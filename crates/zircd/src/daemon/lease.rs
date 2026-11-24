//! The daemon's part in leases ([`zirc_core::leases`]): it says what this machine's
//! sessions want, release and extend as [`LEASE_KIND`] events in the organisation's channel,
//! takes in other machines' alike, tells a session's watch when a lease is theirs, and gives
//! back a session's leases once its watch has been gone a while.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use zirc_core::ipc::{Delivery, Response};
use zirc_core::leases::{self, LEASE_KIND, LeaseEvent, Leases, Want};
use zirc_core::paths;
use zirc_core::project::ORG_CHANNEL;
use zirc_proto::{Event, Message};

use super::{Daemon, WATCHER_SESSION, read_saved};
use crate::clock::unix_ms as now_ms;
use crate::procs::Procs;
use crate::{config, room};

/// How long a session's watch may be gone before its leases are given back, for one that
/// watched: as long as a watch takes to be started again, and then some.
pub(super) const WATCH_GONE: Duration = Duration::from_mins(3);

/// How often, at most, this machine says its lease events again for those joining the
/// organisation's channel.
pub(super) const SAY_AGAIN_EVERY: Duration = Duration::from_mins(1);

/// How often, at most, a session without a watch is looked at for building ([`Procs::read`]
/// runs `ps`).
pub(super) const BUILDING_EVERY: Duration = Duration::from_secs(10);

/// How long a lease's holder may run nothing heavy under it ([`Procs::heavy_under`]) before
/// it's given back, however busy the machine.
pub(super) const IDLE_FOR: Duration = Duration::from_mins(10);

/// How often, at most, holders are looked at for running nothing.
const IDLE_EVERY: Duration = Duration::from_mins(1);

/// The leases on a device, which runs under the system rather than under whoever holds
/// it, so `ps` can't say whether its holder still uses it: they're never given back for
/// running nothing, only when their time is up or their holder's watch has gone.
const DEVICES: &[&str] = &["simulator", "emulator"];

/// Whether lease `name` is given back once its holder has run nothing heavy for
/// [`IDLE_FOR`]: any but a device's ([`DEVICES`]).
fn released_when_idle(name: &str) -> bool {
    // A role's seat is held while its session's watch lives, busy or not (`super::roles`).
    !DEVICES.contains(&name) && zirc_proto::roles::of_lease(name).is_none()
}

/// The longest a lease's name may be.
const MAX_NAME: usize = 64;

/// What this machine says about leases, and what it's heard.
#[derive(Debug, Default)]
pub(super) struct LeaseState {
    pub(super) leases: Leases,
    /// The requests whose session has been told the lease is theirs.
    told: std::collections::BTreeSet<String>,
    /// The requests made when their lease was free, held and waited for by nobody: `zirc
    /// lease` says they're the session's, so its watch needn't.
    free_when_asked: std::collections::BTreeSet<String>,
    /// The `zirc lease --wait` still waiting, by lease and session: its process id. While it
    /// runs it says the lease is the session's, so the session's watch needn't.
    waiters: std::collections::BTreeMap<(String, String), u32>,
    /// Since when each session holding or waiting for a lease has had no watch open.
    unwatched: std::collections::BTreeMap<String, Instant>,
    /// Counts the requests made, to name each apart.
    made: u64,
    /// Requests held back until this machine has room ([`crate::room`]), by lease and
    /// session: how long each would hold it, and why it waits.
    held: std::collections::BTreeMap<(String, String), Holding>,
    /// When the held requests' room was last looked at.
    room_checked: Option<Instant>,
    /// When the waiting commands were last looked at for having ended.
    waiters_checked: Option<Instant>,
    /// When this machine's lease events were last said again, for one who joined.
    said_again: Option<Instant>,
    /// Whether one joined too soon after that to be told, and is told once it's time.
    say_owed: bool,
    /// Who ran each session's last `zirc lease`, by process id: its agent, say.
    runners: std::collections::BTreeMap<String, u32>,
    /// Whether a session without a watch was building, and when that was last looked at.
    building: std::collections::BTreeMap<String, (Instant, bool)>,
    /// Since when each holding request's runner has run nothing heavy.
    idle: std::collections::BTreeMap<String, Instant>,
    /// When holders were last looked at for running nothing.
    idle_checked: Option<Instant>,
    /// How many times each request has been held longer.
    renewals: std::collections::BTreeMap<String, u32>,
}

impl LeaseState {
    /// Forgets what's kept of requests and sessions that no longer count: all but `mine`,
    /// this machine's requests still holding or waiting, and those held for room. Every
    /// `zirc lease` is a new request, and `tools/bazel` makes one a build: what was kept of
    /// each was never dropped.
    fn forget_ended(&mut self, mine: &[leases::Turn]) {
        let ids: std::collections::BTreeSet<&str> =
            mine.iter().map(|turn| turn.want.id.as_str()).collect();
        self.told.retain(|id| ids.contains(id.as_str()));
        self.free_when_asked.retain(|id| ids.contains(id.as_str()));
        self.renewals.retain(|id, _| ids.contains(id.as_str()));
        let sessions: std::collections::BTreeSet<&str> = mine
            .iter()
            .map(|turn| turn.want.session.as_str())
            .chain(self.held.keys().map(|(_, session)| session.as_str()))
            .collect();
        self.runners
            .retain(|session, _| sessions.contains(session.as_str()));
        self.building
            .retain(|session, _| sessions.contains(session.as_str()));
        self.unwatched
            .retain(|session, _| sessions.contains(session.as_str()));
        self.waiters
            .retain(|(_, session), _| sessions.contains(session.as_str()));
    }
}

/// A request held back until this machine has room for the lease.
#[derive(Debug, Clone)]
struct Holding {
    seconds: u64,
    why: String,
    /// Since when its session has had no watch open.
    unwatched: Option<Instant>,
}

/// How often the `zirc lease --wait` commands still waiting are looked at for having ended.
const WAITERS_EVERY: Duration = Duration::from_secs(10);

/// How often held requests look again whether the machine has room.
const ROOM_EVERY: Duration = Duration::from_secs(10);

/// Why `session` can't give back lease `name`, with this machine's `mine`: who here holds
/// or waits for it instead, and how to give theirs back. A session that asked with
/// `--session` and gave back without it was told only that it held nothing, and kept its
/// slot for hours (6 Nov 2026).
fn not_yours(mine: &[leases::Turn], name: &str, session: &str, now: u64) -> String {
    let here: Vec<String> = mine
        .iter()
        .filter(|turn| turn.want.name == name)
        .map(|turn| {
            let how = if turn.start <= now {
                "holds it"
            } else {
                "waits for it"
            };
            format!(
                "{} {how} (zirc lease release {name} --session {})",
                turn.want.session, turn.want.session
            )
        })
        .collect();
    if here.is_empty() {
        format!("{session} neither holds nor waits for lease {name}, nor does any session here")
    } else {
        format!(
            "{session} neither holds nor waits for lease {name}; here, {}",
            here.join("; ")
        )
    }
}

impl Daemon {
    /// Takes in the leases saved before zircd last stopped.
    pub(super) fn restore_leases(&mut self) {
        if let Some(leases) = read_saved::<Leases>(&paths::leases(&self.org.state)) {
            self.leases.leases = leases;
        }
    }

    fn save_leases(&self) {
        let json = serde_json::to_string_pretty(&self.leases.leases).expect("leases serialize");
        if let Err(err) = paths::write_private(&paths::leases(&self.org.state), &json) {
            tracing::warn!("couldn't save the leases: {err}");
        }
    }

    /// This machine's nick, while it's in the organisation's channel, where leases are agreed:
    /// they're said in its name.
    pub(super) fn lease_nick(&self) -> Result<String> {
        let Some(client) = &self.client else {
            bail!("not connected to the server: leases are agreed there");
        };
        if !self.joined.contains(ORG_CHANNEL) {
            bail!(
                "this machine isn't in {ORG_CHANNEL}, where leases are agreed: it's there while \
                 it watches a repository (zirc repo add)"
            );
        }
        Ok(client.nick().to_owned())
    }

    /// Says `event` in the organisation's channel, as `session`, and takes it in.
    pub(super) fn say_lease(&mut self, session: &str, event: LeaseEvent, text: String) {
        let data = serde_json::to_value(&event).expect("a lease event serializes");
        self.leases.leases.apply(event);
        self.save_leases();
        self.publish(
            ORG_CHANNEL,
            session,
            Event::Custom {
                kind: LEASE_KIND.to_owned(),
                data,
                text,
            },
        );
    }

    /// Says again, as they were, the lease events of this machine's that are still
    /// remembered, for one joining the organisation's channel: they go out of its history,
    /// and taking one in twice changes nothing.
    ///
    /// Only while this machine has a request that still counts, holding or waiting, and at
    /// most once every [`SAY_AGAIN_EVERY`]: apps join and leave the channel all the time,
    /// and saying every remembered event to each of them flooded it (5 Nov 2026). One who
    /// joins sooner is told once that much time has passed (`lease_tick`), not
    /// never: a machine joining just after another was told never heard of a lease
    /// (`zircd_restarts_test`, build 3366).
    pub(super) fn say_leases_again(&mut self) {
        let Some(client) = &self.client else { return };
        let me = client.nick().to_owned();
        if !self.leases.leases.has_current(&me, now_ms()) {
            self.leases.say_owed = false;
            return;
        }
        if self
            .leases
            .said_again
            .is_some_and(|at| self.clock.now().duration_since(at) < SAY_AGAIN_EVERY)
        {
            self.leases.say_owed = true;
            return;
        }
        self.leases.said_again = Some(self.clock.now());
        self.leases.say_owed = false;
        for event in self.leases.leases.events_of(&me) {
            let session = match &event {
                LeaseEvent::Want(want) => want.session.clone(),
                _ => WATCHER_SESSION.to_owned(),
            };
            let data = serde_json::to_value(&event).expect("a lease event serializes");
            // Signed as everything this machine sends, with the session's name.
            let message = self.signed(
                &session,
                Event::Custom {
                    kind: LEASE_KIND.to_owned(),
                    data,
                    text: String::new(),
                },
            );
            if let Some(line) = zirc_core::presence::working_line(ORG_CHANNEL, &message)
                && let Err(err) = client.send(line)
            {
                tracing::warn!("couldn't say the leases again: {err}");
            }
        }
    }

    /// [`zirc_core::ipc::Request::Lease`].
    pub(super) async fn lease(
        &mut self,
        name: &str,
        session: &str,
        seconds: Option<u64>,
        pid: Option<u32>,
        wait: bool,
    ) -> Result<Response> {
        let name = name.trim();
        if name.is_empty() || name.len() > MAX_NAME || name.contains(char::is_whitespace) {
            bail!("a lease's name is one word of at most {MAX_NAME} bytes, such as emulator");
        }
        let me = self.lease_nick()?;
        let now = now_ms();
        let seconds = seconds
            .unwrap_or(leases::DEFAULT_SECONDS)
            .clamp(1, leases::MAX_SECONDS);
        let runner = match pid {
            Some(pid) => Procs::read().await.and_then(|procs| procs.runner_of(pid)),
            None => None,
        };
        if let Some(runner) = runner {
            self.leases.runners.insert(session.to_owned(), runner);
        }
        let key = (name.to_owned(), session.to_owned());
        match pid.filter(|_| wait) {
            Some(pid) => self.leases.waiters.insert(key, pid),
            None => self.leases.waiters.remove(&key),
        };
        match self.leases.leases.turn_of(name, &me, session, now) {
            // Holding it: for `seconds` from now, or while others wait, to the cap.
            Some(turn) if turn.start <= now => self.renew(&me, &turn, seconds, now),
            // Waiting for it already.
            Some(_) => {}
            // Not until this machine has room: asked for then (lease_tick).
            None => {
                if let Some(why) = self.short_of(name).await {
                    let key = (name.to_owned(), session.to_owned());
                    self.leases.held.insert(
                        key,
                        Holding {
                            seconds,
                            why,
                            unwatched: None,
                        },
                    );
                } else {
                    let lease = self.leases.leases.lease(name, now, &me);
                    let free =
                        lease.holders.len() < leases::slots(name) && lease.waiting.is_empty();
                    let id = self.want(&me, name, session, seconds, now);
                    if free {
                        self.leases.free_when_asked.insert(id);
                    }
                }
            }
        }
        Ok(Response::Leases {
            leases: vec![self.with_held(self.leases.leases.lease(name, now, &me))],
        })
    }

    /// Holds `turn`'s lease longer, asked at `now` for `seconds` more ([`leases::renewal`]):
    /// past the cap while others wait, its session asks again behind them.
    pub(super) fn renew(&mut self, me: &str, turn: &leases::Turn, seconds: u64, now: u64) {
        let (name, session) = (&turn.want.name, &turn.want.session);
        let others = self.leases.leases.others_wait(turn, now);
        let (until, capped) = leases::renewal(turn, others, now, seconds);
        let times = self
            .leases
            .renewals
            .entry(turn.want.id.clone())
            .or_default();
        *times += 1;
        tracing::info!(
            "{session} asks again for lease {name}, held {}m so far, renewal {times}{}",
            now.saturating_sub(turn.start) / 60_000,
            if others { ", others waiting" } else { "" },
        );
        if until > turn.end {
            let event = LeaseEvent::Extend {
                id: turn.want.id.clone(),
                until,
            };
            let text = format!("{session} holds lease {name} longer");
            self.say_lease(session, event, text);
        }
        if capped && !self.leases.leases.asked_again(turn) {
            tracing::info!("{session} has held lease {name} its most in a row: queued again");
            let (name, session) = (name.clone(), session.clone());
            let _ = self.want(me, &name, &session, seconds, now);
        }
    }

    /// Asks the organisation's machines for lease `name` for `session`, for `seconds` once
    /// it's theirs.
    pub(super) fn want(
        &mut self,
        me: &str,
        name: &str,
        session: &str,
        seconds: u64,
        now: u64,
    ) -> String {
        self.leases.made += 1;
        let want = Want {
            id: format!("{me}-{now}-{}", self.leases.made),
            name: name.to_owned(),
            nick: me.to_owned(),
            session: session.to_owned(),
            at: now,
            seconds,
        };
        let text = format!("{session} wants lease {name}");
        let id = want.id.clone();
        self.say_lease(session, LeaseEvent::Want(want), text);
        id
    }

    /// Why this machine hasn't room for lease `name` yet, as its `config.toml` says
    /// (`[leases.<name>]`); `None` when it has, or says nothing about it.
    async fn short_of(&self, name: &str) -> Option<String> {
        let room = self
            .config
            .ask(|settings| config::lease_room(settings, name))?;
        room::short_now(&room).await
    }

    /// `lease` with this machine's requests for it that wait for room.
    fn with_held(&self, mut lease: leases::Lease) -> leases::Lease {
        lease.held = self
            .leases
            .held
            .iter()
            .filter(|((name, _), _)| *name == lease.name)
            .map(|((_, session), holding)| leases::Held {
                session: session.clone(),
                why: holding.why.clone(),
            })
            .collect();
        lease
    }

    /// [`zirc_core::ipc::Request::LeaseRelease`].
    pub(super) fn lease_release(&mut self, name: &str, session: &str) -> Result<Response> {
        let me = self.lease_nick()?;
        let now = now_ms();
        // One still waiting for room was never asked for: nothing to give back.
        if self
            .leases
            .held
            .remove(&(name.to_owned(), session.to_owned()))
            .is_some()
        {
            return Ok(Response::Leases {
                leases: vec![self.with_held(self.leases.leases.lease(name, now, &me))],
            });
        }
        let Some(turn) = self.leases.leases.turn_of(name, &me, session, now) else {
            bail!(
                "{}",
                not_yours(&self.leases.leases.mine(&me, now), name, session, now)
            );
        };
        let event = LeaseEvent::Release {
            id: turn.want.id,
            at: now,
        };
        self.say_lease(session, event, format!("{session} released lease {name}"));
        Ok(Response::Leases {
            leases: vec![self.leases.leases.lease(name, now, &me)],
        })
    }

    /// [`zirc_core::ipc::Request::Leases`].
    pub(super) fn leases(&self) -> Response {
        let me = self.client.as_ref().map_or("", |client| client.nick());
        let now = now_ms();
        let mut leases: Vec<leases::Lease> = self
            .leases
            .leases
            .state(now, me)
            .into_iter()
            .map(|lease| self.with_held(lease))
            .collect();
        // Those only held here, which nobody holds or waits for yet.
        for (name, _) in self.leases.held.keys() {
            if !leases.iter().any(|lease| &lease.name == name) {
                leases.push(self.with_held(self.leases.leases.lease(name, now, me)));
            }
        }
        Response::Leases { leases }
    }

    /// Asks for the held requests whose lease this machine now has room for, at most every
    /// [`ROOM_EVERY`], and drops those whose session's watch has been gone [`WATCH_GONE`].
    async fn ask_held(&mut self, me: &str, watching: &std::collections::BTreeSet<String>) {
        if self.leases.held.is_empty()
            || self
                .leases
                .room_checked
                .is_some_and(|at| self.clock.now().duration_since(at) < ROOM_EVERY)
        {
            return;
        }
        self.leases.room_checked = Some(self.clock.now());
        let now = now_ms();
        let held: Vec<((String, String), Holding)> = self
            .leases
            .held
            .iter()
            .map(|(key, holding)| (key.clone(), holding.clone()))
            .collect();
        let at = self.clock.now();
        for ((name, session), holding) in held {
            let key = (name.clone(), session.clone());
            let unwatched = (!watching.contains(&session) && self.missed.contains_key(&session))
                .then(|| holding.unwatched.unwrap_or(at));
            if unwatched.is_some_and(|since| at.duration_since(since) >= WATCH_GONE) {
                self.leases.held.remove(&key);
                continue;
            }
            let Some(why) = self.short_of(&name).await else {
                self.leases.held.remove(&key);
                let _ = self.want(me, &name, &session, holding.seconds, now);
                continue;
            };
            if let Some(it) = self.leases.held.get_mut(&key) {
                it.why = why;
                it.unwatched = unwatched;
            }
        }
    }

    /// Whether a `zirc lease --wait` says `session` that lease `name` is its own, so its
    /// watch needn't wake for that too: one was waiting for it, and was still running when
    /// last looked at ([`Daemon::forget_dead_waiters`]). It isn't asked whether it runs now:
    /// it ends the moment it sees the lease is the session's, which is often just before
    /// this asks, and the watch was then told what the command had just said, with "release
    /// it when you're done" for a lease `tools/bazel` gives back itself.
    fn waiter_says_it(&mut self, name: &str, session: &str) -> bool {
        self.leases
            .waiters
            .remove(&(name.to_owned(), session.to_owned()))
            .is_some()
    }

    /// Forgets the `zirc lease --wait` commands that have ended without their lease, as one
    /// interrupted has, looking at most every [`WAITERS_EVERY`]: their sessions' watches are
    /// then told when the lease is theirs, as nothing else will.
    async fn forget_dead_waiters(&mut self) {
        let at = self.clock.now();
        if self.leases.waiters.is_empty()
            || self
                .leases
                .waiters_checked
                .is_some_and(|last| at.duration_since(last) < WAITERS_EVERY)
        {
            return;
        }
        self.leases.waiters_checked = Some(at);
        let waiting: Vec<((String, String), u32)> = self
            .leases
            .waiters
            .iter()
            .map(|(key, pid)| (key.clone(), *pid))
            .collect();
        for (key, pid) in waiting {
            // Only one known to be gone: when the machine can't say, it's taken to run.
            if Procs::alive(pid).await == Some(false) {
                self.leases.waiters.remove(&key);
            }
        }
    }

    /// Whether `response` says a lease is a session's, of a request this machine no longer
    /// holds: news no more by the time a watch would hear it.
    pub(super) fn stale_lease_notice(&self, response: &Response) -> bool {
        let Response::Delivery(delivery) = response else {
            return false;
        };
        let Event::Custom { kind, data, .. } = &delivery.message.event else {
            return false;
        };
        let Some(id) = data.get("id").and_then(serde_json::Value::as_str) else {
            return false;
        };
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return false;
        };
        kind == LEASE_KIND
            && !self
                .leases
                .leases
                .mine(&me, now_ms())
                .iter()
                .any(|turn| turn.want.id == id)
    }

    /// Tells the watch of the session `text` names, in a lease line of zircd's own, which
    /// concerns only whom it names.
    pub(super) fn tell_session(&mut self, text: String, id: Option<&str>) {
        let from = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        self.tell_watchers(&Response::Delivery(Delivery::new(
            ORG_CHANNEL,
            from,
            Message::new(
                WATCHER_SESSION,
                Event::Custom {
                    kind: LEASE_KIND.to_owned(),
                    // The request it's about, when a release makes it stale (`catch_up`).
                    data: id.map_or(
                        serde_json::Value::Null,
                        |id| serde_json::json!({ "id": id }),
                    ),
                    text,
                },
            ),
        )));
    }

    /// Gives back, telling its session's watch, each lease held here whose
    /// runner has run nothing heavy for [`IDLE_FOR`], or is gone: no device's
    /// ([`DEVICES`]). Looked at every [`IDLE_EVERY`], and only for sessions whose runner is known.
    async fn release_idle(&mut self, mine: &[leases::Turn], now: u64) {
        if self
            .leases
            .idle_checked
            .is_some_and(|at| self.clock.now().duration_since(at) < IDLE_EVERY)
        {
            return;
        }
        let holding: Vec<(leases::Turn, u32)> = mine
            .iter()
            .filter(|turn| turn.start <= now && released_when_idle(&turn.want.name))
            .filter_map(|turn| {
                let runner = *self.leases.runners.get(&turn.want.session)?;
                Some((turn.clone(), runner))
            })
            .collect();
        let ids: std::collections::BTreeSet<&String> =
            holding.iter().map(|(turn, _)| &turn.want.id).collect();
        self.leases.idle.retain(|id, _| ids.contains(id));
        if holding.is_empty() {
            return;
        }
        self.leases.idle_checked = Some(self.clock.now());
        let Some(procs) = Procs::read().await else {
            return;
        };
        for (turn, runner) in holding {
            let id = turn.want.id.clone();
            let running = procs.has(runner);
            // Whatever the load: a busy machine kept holders that had stopped for hours.
            if running && procs.heavy_under(runner).is_some() {
                self.leases.idle.remove(&id);
                continue;
            }
            let at = self.clock.now();
            let since = *self.leases.idle.entry(id.clone()).or_insert(at);
            if running && at.duration_since(since) < IDLE_FOR {
                continue;
            }
            self.leases.idle.remove(&id);
            let (session, name) = (turn.want.session.clone(), turn.want.name.clone());
            let why = if running {
                format!(
                    "nothing has run under it for {} minutes",
                    IDLE_FOR.as_secs() / 60
                )
            } else {
                "what asked for it has stopped".to_owned()
            };
            tracing::info!("{session} holds lease {name} and {why}: giving it back");
            let event = LeaseEvent::Release { id, at: now };
            self.say_lease(
                &session,
                event,
                format!("{session} released lease {name}, {why}"),
            );
            self.tell_session(
                format!(
                    "{session}: gave back lease {name}, as {why}; zirc lease {name} --session \
                 {session} to ask again"
                ),
                None,
            );
        }
    }

    /// Takes in what another machine, `from`, said in `channel`, if it's about leases there:
    /// only about its own requests, so no machine can give back or extend another's.
    pub(super) fn lease_heard(&mut self, channel: &str, from: &str, event: &Event) {
        let Event::Custom { kind, data, .. } = event else {
            return;
        };
        if channel != ORG_CHANNEL || kind != LEASE_KIND {
            return;
        }
        let Ok(event) = serde_json::from_value::<LeaseEvent>(data.clone()) else {
            tracing::debug!("{from} said something about leases zircd can't read");
            return;
        };
        let theirs = match &event {
            LeaseEvent::Want(want) => want.nick == from,
            LeaseEvent::Release { id, .. } | LeaseEvent::Extend { id, .. } => {
                id.starts_with(&format!("{from}-"))
            }
        };
        if !theirs {
            tracing::debug!("{from} spoke for another machine's lease: ignored");
            return;
        }
        self.leases.leases.apply(event);
        self.save_leases();
    }

    /// Whether something heavy (Bazel, a compiler) runs under the process the session last
    /// asked for a lease from: it's building, whatever its watch says. Reads the processes
    /// at most every [`BUILDING_EVERY`], and says what it last found between.
    async fn builds_under(&mut self, session: &str) -> bool {
        let Some(&runner) = self.leases.runners.get(session) else {
            return false;
        };
        if let Some((at, found)) = self.leases.building.get(session)
            && self.clock.now().duration_since(*at) < BUILDING_EVERY
        {
            return *found;
        }
        let found = Procs::read()
            .await
            .is_some_and(|procs| procs.has(runner) && procs.heavy_under(runner).is_some());
        self.leases
            .building
            .insert(session.to_owned(), (self.clock.now(), found));
        found
    }

    /// Once a second: tells each session here whose turn at a lease has come, and gives back
    /// the leases of sessions whose watch has been gone [`WATCH_GONE`].
    pub(super) async fn lease_tick(&mut self) {
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return;
        };
        if self.leases.say_owed {
            self.say_leases_again();
        }
        let now = now_ms();
        let mine = self.leases.leases.mine(&me, now);
        let watching: std::collections::BTreeSet<String> = self
            .watchers
            .iter()
            .filter(|watcher| !watcher.heard.is_closed())
            .filter_map(|watcher| watcher.session.clone())
            .collect();
        self.ask_held(&me, &watching).await;
        self.forget_dead_waiters().await;
        let mut gone = Vec::new();
        for turn in &mine {
            let session = &turn.want.session;
            if watching.contains(session) || !self.missed.contains_key(session) {
                self.leases.unwatched.remove(session);
            } else {
                let at = self.clock.now();
                let since = *self.leases.unwatched.entry(session.clone()).or_insert(at);
                if at.duration_since(since) >= WATCH_GONE {
                    // A holder still building keeps its lease: its watch ended when
                    // something woke it, and the agent starts it again only once its
                    // command returns, which for a 20-minute `bazel test` is long after
                    // WATCH_GONE. Giving the slot away mid-build put five Bazel servers
                    // on a machine whose lease said two (9 Nov).
                    if !(turn.start <= now && self.builds_under(session).await) {
                        gone.push(turn.clone());
                        continue;
                    }
                }
            }
            // A lease free when it was asked for, and the session's once settled, is what
            // `zirc lease` said: telling its watch too woke it for nothing.
            let at_once = self.leases.free_when_asked.contains(&turn.want.id)
                && turn.start <= turn.want.at + leases::SETTLE;
            // A role's seat: `zirc role take` said so, and one that came second is told apart.
            let role = zirc_proto::roles::of_lease(&turn.want.name).is_some();
            if turn.start <= now
                && !role
                && self.leases.told.insert(turn.want.id.clone())
                && !at_once
                && !self.waiter_says_it(&turn.want.name, session)
            {
                let minutes = (turn.end.saturating_sub(now) / 60_000).max(1);
                let text = format!(
                    "{session}: lease {} is yours, for {minutes} minutes; zirc lease release {} \
                     --session {session} when you're done",
                    turn.want.name, turn.want.name
                );
                self.tell_session(text, Some(&turn.want.id));
            }
        }
        for turn in gone {
            let session = turn.want.session.clone();
            let name = turn.want.name.clone();
            tracing::info!("{session}'s watch has been gone a while: giving back lease {name}");
            let event = LeaseEvent::Release {
                id: turn.want.id,
                at: now,
            };
            let text = format!("{session} released lease {name}, its watch gone");
            self.say_lease(&session, event, text);
            self.leases.unwatched.remove(&session);
        }
        self.release_idle(&mine, now).await;
        self.roles_tick();
        // Read again: a request held for room may have been asked for since `mine` was.
        let mine = self.leases.leases.mine(&me, now_ms());
        self.leases.forget_ended(&mine);
        if self.leases.leases.prune(now) {
            self.save_leases();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_lease_is_never_given_back_for_running_nothing() {
        assert!(released_when_idle("build"));
        assert!(!released_when_idle("simulator") && !released_when_idle("emulator"));
    }

    #[tokio::test]
    async fn a_waiting_command_speaks_for_the_watch_unless_it_ended_without_its_lease() {
        let home = tempfile::tempdir().unwrap();
        let org = super::super::Org {
            name: "zirc".into(),
            first: true,
            state: home.path().to_owned(),
            several: false,
        };
        let (inputs, _received) = tokio::sync::mpsc::unbounded_channel();
        let mut daemon = Daemon::new(
            home.path().to_owned(),
            org,
            "irc.example".into(),
            None,
            inputs,
        );
        let key = |session: &str| ("build".to_owned(), session.to_owned());
        // One that ended a while ago, as when interrupted, and one still running: this test.
        let mut ended = std::process::Command::new("true").spawn().unwrap();
        let gone = ended.id();
        ended.wait().unwrap();
        daemon.leases.waiters.insert(key("cc-gone"), gone);
        daemon
            .leases
            .waiters
            .insert(key("cc-waits"), std::process::id());
        daemon.forget_dead_waiters().await;
        // The one that ended says nothing, so the watch is told; the other speaks for it,
        // once, though it's not asked again whether it runs.
        assert!(!daemon.waiter_says_it("build", "cc-gone"));
        assert!(daemon.waiter_says_it("build", "cc-waits"));
        assert!(!daemon.waiter_says_it("build", "cc-waits"));
        // Looked at only now and then: one that ends is still taken to run till the next look.
        daemon.leases.waiters.insert(key("cc-late"), gone);
        daemon.forget_dead_waiters().await;
        assert!(daemon.leases.waiters.contains_key(&key("cc-late")));
        daemon.clock.advance(WAITERS_EVERY);
        daemon.forget_dead_waiters().await;
        assert!(!daemon.leases.waiters.contains_key(&key("cc-late")));
    }

    #[test]
    fn what_is_kept_of_ended_requests_is_forgotten_and_of_current_ones_kept() {
        let turn = |session: &str| leases::Turn {
            want: Want {
                id: format!("me-{session}"),
                name: "build".into(),
                nick: "me".into(),
                session: session.into(),
                at: 0,
                seconds: 1200,
            },
            start: 0,
            end: 1_200_000,
        };
        let mut state = LeaseState::default();
        for session in ["cc-holds", "cc-ended", "cc-held"] {
            let id = format!("me-{session}");
            state.told.insert(id.clone());
            state.free_when_asked.insert(id.clone());
            state.renewals.insert(id, 2);
            state.runners.insert(session.to_owned(), 7);
            state
                .building
                .insert(session.to_owned(), (Instant::now(), true));
            state.unwatched.insert(session.to_owned(), Instant::now());
            state
                .waiters
                .insert(("build".to_owned(), session.to_owned()), 7);
        }
        // Held for room: no request yet, but its runner is who'll hold the lease.
        state.held.insert(
            ("build".to_owned(), "cc-held".to_owned()),
            Holding {
                seconds: 1200,
                why: "load 52 is over 40".to_owned(),
                unwatched: None,
            },
        );
        state.forget_ended(&[turn("cc-holds")]);
        let ids = |ids: &std::collections::BTreeSet<String>| -> Vec<String> {
            ids.iter().cloned().collect()
        };
        assert_eq!(ids(&state.told), ["me-cc-holds"]);
        assert_eq!(ids(&state.free_when_asked), ["me-cc-holds"]);
        assert_eq!(state.renewals.keys().collect::<Vec<_>>(), ["me-cc-holds"]);
        let kept = ["cc-held", "cc-holds"];
        assert_eq!(state.runners.keys().collect::<Vec<_>>(), kept);
        assert_eq!(state.building.keys().collect::<Vec<_>>(), kept);
        assert_eq!(state.unwatched.keys().collect::<Vec<_>>(), kept);
        let waiting: Vec<&str> = state
            .waiters
            .keys()
            .map(|(_, session)| session.as_str())
            .collect();
        assert_eq!(waiting, kept);
    }

    #[test]
    fn giving_back_another_sessions_lease_says_whose_it_is_and_how() {
        let turn = |session: &str, start: u64| leases::Turn {
            want: Want {
                id: format!("me-{session}"),
                name: "build".into(),
                nick: "me".into(),
                session: session.into(),
                at: 0,
                seconds: 1200,
            },
            start,
            end: 1_200_000,
        };
        assert_eq!(
            not_yours(
                &[turn("cc-1", 0), turn("cc-2", 5_000)],
                "build",
                "mattias",
                1_000
            ),
            "mattias neither holds nor waits for lease build; here, cc-1 holds it (zirc lease \
             release build --session cc-1); cc-2 waits for it (zirc lease release build \
             --session cc-2)"
        );
        assert_eq!(
            not_yours(&[], "build", "mattias", 0),
            "mattias neither holds nor waits for lease build, nor does any session here"
        );
    }
}
