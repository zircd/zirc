//! A circuit breaker on wakes: each wake of an agent's watch costs a turn, so a storm of
//! them, from a bug or a runaway loop, runs up the bill. zircd counts the wakes it lets
//! through in a sliding window, for each session and for the whole machine, and past a
//! limit holds the rest as notes for the session's next wake ([`super::later`]), tells the
//! machine's owner by a direct message once, and lets wakes through again when the window
//! has drained to half the limit, or at once on `zirc wakes resume`.
//!
//! What a session asked to be woken for, a landing it waits on or an answer to its own
//! question, is never held and doesn't count: only wakes nobody asked for make a storm. The
//! owner's own direct messages to a session are never held. The breaker's notice goes
//! to the owner alone, never to a session's watch and not to a channel, so tripping wakes
//! nobody and `#ops` stays for what needs someone there: it was an alert the owner's own
//! phone already had.
//!
//! The user asked for "a conservative low number so if something goes wrong it can't run
//! wild and I'll get notified" (6 Nov 2026).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use zirc_core::chat::{SenderKind, sender_kind};
use zirc_core::ipc::{BreakerState, Response, Why};
use zirc_irc::Line;
use zirc_proto::Event;

use super::{Daemon, WATCHER_SESSION};

/// The window wakes are counted over.
pub(super) const WINDOW: Duration = Duration::from_mins(10);

/// The most wakes of one session in a [`WINDOW`].
pub(crate) const PER_SESSION: usize = 6;

/// The most wakes of all this machine's sessions in a [`WINDOW`].
pub(crate) const MACHINE: usize = 20;

/// How the breaker's notices start, which wake nobody, whoever they name.
const NOTICE: &str = "wakes paused:";

/// How many of the commonest sources a notice names.
const TOP: usize = 3;

/// Whether a wake goes ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    /// It wakes the session.
    Wake,
    /// It's held, the breaker being open.
    Hold,
    /// It's held, and it just opened the breaker: the notice to send the owner.
    Trip(String),
}

/// The wakes counted, and which limits are open.
#[derive(Debug)]
pub(crate) struct Breaker {
    /// The most wakes of one session in a [`WINDOW`], [`PER_SESSION`] unless configured.
    per_session: usize,
    /// The most wakes of the machine in a [`WINDOW`], [`MACHINE`] unless configured.
    machine_limit: usize,
    /// The wakes let through in the window, oldest first: when, whose and what woke it.
    wakes: VecDeque<(Instant, String, String)>,
    /// Whether the machine's limit is open: no session wakes.
    machine: bool,
    /// The sessions whose own limit is open.
    sessions: BTreeSet<String>,
    /// How many wakes were held since the breaker last opened.
    held: usize,
    /// Notices of its opening, to send the owner once the line being told is.
    pending: Vec<String>,
    /// The workflow steps given to a session, as `<session> <run> <step>`, whose first wake
    /// went through as asked for ([`given_step`]); the newest few hundred.
    given: VecDeque<String>,
}

impl Default for Breaker {
    fn default() -> Self {
        Self::with_limits(PER_SESSION, MACHINE)
    }
}

impl Breaker {
    /// A breaker with these limits.
    pub(crate) fn with_limits(per_session: usize, machine_limit: usize) -> Self {
        Self {
            per_session,
            machine_limit,
            wakes: VecDeque::new(),
            machine: false,
            sessions: BTreeSet::new(),
            held: 0,
            pending: Vec::new(),
            given: VecDeque::new(),
        }
    }

    /// Forgets wakes older than the window, and closes each limit that's drained to half.
    fn drain(&mut self, now: Instant) {
        while self
            .wakes
            .front()
            .is_some_and(|(at, ..)| now.duration_since(*at) >= WINDOW)
        {
            self.wakes.pop_front();
        }
        if self.machine && self.wakes.len() <= self.machine_limit / 2 {
            self.machine = false;
            tracing::info!("wakes resume: the machine's window has drained");
        }
        let counts = self.counts();
        self.sessions.retain(|session| {
            let open = counts.get(session).copied().unwrap_or(0) > self.per_session / 2;
            if !open {
                tracing::info!("{session}'s wakes resume: its window has drained");
            }
            open
        });
        if !self.machine && self.sessions.is_empty() {
            self.held = 0;
        }
    }

    /// The wakes in the window, by session.
    fn counts(&self) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for (_, session, _) in &self.wakes {
            *counts.entry(session.clone()).or_default() += 1;
        }
        counts
    }

    /// Whether `session` may wake at `now` for `source`, such as `mention from cc-1`.
    pub(super) fn allow(&mut self, session: &str, source: &str, now: Instant) -> Verdict {
        self.drain(now);
        if self.machine || self.sessions.contains(session) {
            self.held += 1;
            return Verdict::Hold;
        }
        let mine = self.wakes.iter().filter(|(_, it, _)| it == session).count();
        if self.wakes.len() >= self.machine_limit {
            self.machine = true;
            self.held = 1;
            return Verdict::Trip(
                self.notice(&format!("{} wakes on this machine", self.wakes.len())),
            );
        }
        if mine >= self.per_session {
            self.sessions.insert(session.to_owned());
            self.held += 1;
            return Verdict::Trip(self.notice(&format!("{mine} wakes of {session}")));
        }
        tracing::info!("woke {session}: {source}");
        self.wakes
            .push_back((now, session.to_owned(), source.to_owned()));
        Verdict::Wake
    }

    /// Counts a wake that's never held, such as the owner's direct message.
    pub(super) fn count(&mut self, session: &str, source: &str, now: Instant) {
        self.drain(now);
        self.wakes
            .push_back((now, session.to_owned(), source.to_owned()));
    }

    /// Whether any limit is open.
    fn paused(&self) -> bool {
        self.machine || !self.sessions.is_empty()
    }

    /// Closes every limit, and forgets the wakes counted, so it doesn't open again at once.
    pub(super) fn resume(&mut self) {
        self.wakes.clear();
        self.machine = false;
        self.sessions.clear();
        self.held = 0;
    }

    /// The commonest sources in the window, most first, as `mention from cc-1 ×5`.
    fn top(&self) -> Vec<String> {
        let mut by: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, session, source) in &self.wakes {
            let _ = session;
            *by.entry(source.as_str()).or_default() += 1;
        }
        let mut by: Vec<(&str, usize)> = by.into_iter().collect();
        by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        by.into_iter()
            .map(|(source, n)| format!("{source} ×{n}"))
            .collect()
    }

    /// What the notice says, for `what` tripped it.
    fn notice(&self, what: &str) -> String {
        let top: Vec<String> = self.top().into_iter().take(TOP).collect();
        // Read at a glance on a phone: what tripped, then why, then what to do.
        format!(
            "{NOTICE} {what} in {} min\n\
             - top sources: {}\n\
             - held: they wait as notes for each session's next wake\n\
             - needed: reply `resume` to let them through now, or run `zirc wakes resume`",
            WINDOW.as_secs() / 60,
            if top.is_empty() {
                "none".to_owned()
            } else {
                top.join(", ")
            }
        )
    }

    /// How it stands at `now`, for `zirc wakes` and `zirc status`.
    pub(super) fn state(&mut self, now: Instant) -> BreakerState {
        self.drain(now);
        BreakerState {
            window_minutes: WINDOW.as_secs() / 60,
            per_session: self.per_session,
            machine: self.machine_limit,
            wakes: self.wakes.len(),
            paused_machine: self.machine,
            paused_sessions: self.sessions.iter().cloned().collect(),
            held: self.held,
            top: self.top(),
        }
    }
}

/// Whether `response` is one of the breaker's notices, which wake nobody.
fn notice(response: &Response) -> bool {
    matches!(response, Response::Delivery(delivery)
        if matches!(&delivery.message.event, Event::Text { body } if body.starts_with(NOTICE)))
}

/// What woke a session, for the breaker's count: the reason and who said it.
fn source(response: &Response, why: Why) -> String {
    let who = match response {
        Response::Delivery(delivery) if !delivery.message.session.is_empty() => {
            delivery.message.session.clone()
        }
        Response::Delivery(delivery) => delivery.from.clone(),
        Response::Direct(direct) => direct.from.clone(),
        _ => String::new(),
    };
    let why = super::wakes::reason(why);
    if who.is_empty() {
        why
    } else {
        format!("{why} from {who}")
    }
}

/// Whether `response` is a direct message in the owner's own words ([`Daemon::direct`]),
/// which the breaker never holds. Another agent's query, answer or note comes as a direct
/// message too and is held like any other wake: two sessions asking each other in a loop is
/// the storm the breaker is for.
fn owners_word(response: &Response) -> bool {
    matches!(response, Response::Direct(direct)
        if direct.query.is_none() && direct.answer.is_none() && !direct.note)
}

/// Whether `response` wakes a session for `reason` because it asked to be: what it waits on
/// moved (`zirc wait-for`, `zirc stack`), or another agent answered its own `zirc ask`. In two
/// days every one of this machine's eleven trips was mostly these, CI saying how landings
/// the session waited on had gone, and none was the runaway the breaker is for (12 Nov
/// 2026): it held what sessions had asked for, some 75 minutes in all.
fn asked_for(response: &Response, reason: Why) -> bool {
    reason == Why::Awaited
        || matches!(response, Response::Direct(direct) if direct.answer.is_some())
}

/// The run and step a workflow's `response` gives `session`, if it does: its step assigned
/// or escalated to the session, as `machine/session` or the session alone.
fn given_step(response: &Response, session: &str) -> Option<String> {
    let Response::Delivery(delivery) = response else {
        return None;
    };
    let event = zirc_proto::run::RunEvent::of(&delivery.message.event)?;
    let (zirc_proto::run::Change::Assigned { step, to }
    | zirc_proto::run::Change::Escalated { step, to }) = &event.change
    else {
        return None;
    };
    let ours = |name: &String| {
        name == session
            || name
                .rsplit_once('/')
                .is_some_and(|(_, name)| name == session)
    };
    to.iter()
        .any(ours)
        .then(|| format!("{session} {} {step}", event.run))
}

/// Whether `response` is a state said anew or an integration's line that something went
/// wrong, such as CI saying a build failed, which the breaker never holds.
fn urgent(response: &Response) -> bool {
    let Response::Delivery(delivery) = response else {
        return false;
    };
    zirc_proto::state::State::of(&delivery.message.event).is_some()
        || (sender_kind(&delivery.from, None) == SenderKind::Integration
            && !super::routine(&delivery.from, &delivery.message))
}

impl Daemon {
    /// Holds each wake in `reasons` the breaker won't let through, as a note for that
    /// session's next wake, and says so if it just opened.
    pub(super) fn gate_wakes(&mut self, response: &Response, reasons: &mut [Option<Why>]) {
        if notice(response) {
            reasons.fill(None);
            return;
        }
        let now = self.clock.now();
        let mut held = Vec::new();
        for (watcher, why) in self.watchers.iter().zip(reasons.iter_mut()) {
            let (Some(session), Some(reason)) = (watcher.session.as_deref(), *why) else {
                continue;
            };
            let from = source(response, reason);
            // The owner's own direct messages always reach the session, and so do a state
            // said anew and CI's failures: the breaker once paused the operator's session in
            // the middle of master breaking (11 Nov 2026).
            // What the session asked to be woken for isn't a storm, however much of it comes:
            // it neither waits nor counts.
            if asked_for(response, reason) {
                continue;
            }
            // A step a workflow gives the session, one of a role it joined to be given them,
            // as the master fixer is (ops/master_fixer.sh): asked for too, the first time. A
            // flaky build gave it six in a minute, and the breaker held five (14 Nov 2026).
            // The same step said again counts, so a workflow gone wrong still trips it.
            if let Some(given) = given_step(response, session)
                && !self.breaker.given.contains(&given)
            {
                self.breaker.given.push_back(given);
                if self.breaker.given.len() > 500 {
                    self.breaker.given.pop_front();
                }
                continue;
            }
            if owners_word(response) || urgent(response) {
                self.breaker.count(session, &from, now);
                continue;
            }
            match self.breaker.allow(session, &from, now) {
                Verdict::Wake => {}
                Verdict::Hold => {
                    *why = None;
                    held.push((session.to_owned(), reason));
                }
                Verdict::Trip(notice) => {
                    *why = None;
                    held.push((session.to_owned(), reason));
                    self.breaker.pending.push(notice);
                }
            }
        }
        for (session, reason) in held {
            self.keep_for_later(&session, response, reason);
        }
    }

    /// Sends the notices of the breaker's opening, once what tripped it has been told.
    pub(super) fn say_trips(&mut self) {
        for notice in std::mem::take(&mut self.breaker.pending) {
            self.tell_of_trip(&notice);
        }
    }

    /// Whether `session`'s catch-up, a wake of its own, is held: what it missed then
    /// waits for its next wake instead.
    pub(super) fn catch_up_held(&mut self, session: &str, missed: &[Response]) -> bool {
        let now = self.clock.now();
        match self
            .breaker
            .allow(session, "missed while no watch ran", now)
        {
            Verdict::Wake => false,
            verdict => {
                for response in missed {
                    self.keep_for_later(session, response, Why::Mention);
                }
                if let Verdict::Trip(notice) = verdict {
                    self.tell_of_trip(&notice);
                }
                true
            }
        }
    }

    /// Tells the machine's owner that wakes are paused ([`Daemon::tell_owner`]).
    fn tell_of_trip(&mut self, notice: &str) {
        self.tell_owner(notice);
    }

    /// Tells the machine's owner `notice` by a direct message, such as that wakes are
    /// paused: never a session's watch, and no channel (6 Nov 2026: "so I'll get notified";
    /// the first notices went to `#decisions` and then `#ops`, where they were one more line
    /// among the alerts). A machine without an owner has no one to tell, so it only logs.
    pub(super) fn tell_owner(&mut self, notice: &str) {
        tracing::warn!("{}", notice.replace('\n', " "));
        let (Some(client), Some(owner)) = (&self.client, &self.owner) else {
            return;
        };
        let message = zirc_proto::Message {
            level: Some(zirc_proto::Level::Warn),
            ..self.signed(
                WATCHER_SESSION,
                Event::Text {
                    body: notice.to_owned(),
                },
            )
        };
        if let Err(err) = zirc_core::send::send_message(client, owner, &message) {
            tracing::warn!("couldn't tell {owner}: {err}");
        }
    }

    /// Whether `body`, a direct message from `account`, is the machine's owner saying
    /// `resume` (or `wakes resume`) while wakes are paused, which then lets them through.
    /// A notification's Resume button sends it, and so does a reply to the breaker's notice;
    /// at any other time it's a line for the agents like any other.
    pub(super) fn resumed_by_owner(&mut self, account: Option<&str>, body: &str) -> bool {
        let (Some(owner), Some(account)) = (self.owner.as_deref(), account) else {
            return false;
        };
        if !account.eq_ignore_ascii_case(owner) || !self.breaker.paused() {
            return false;
        }
        let text = match zirc_proto::decode(body) {
            Ok(Some(message)) => match message.event {
                Event::Text { body } => body,
                _ => return false,
            },
            Ok(None) if !body.starts_with('\x01') => body.to_owned(),
            _ => return false,
        };
        let text = text.trim().trim_start_matches('/').to_ascii_lowercase();
        if !matches!(text.as_str(), "resume" | "wakes resume") {
            return false;
        }
        self.wakes_resume();
        true
    }

    /// [`Daemon::resumed_by_owner`], and if so, says to `from` that it has.
    pub(super) fn resume_on_owners_word(
        &mut self,
        account: Option<&str>,
        from: &str,
        body: &str,
    ) -> bool {
        if !self.resumed_by_owner(account, body) {
            return false;
        }
        if let Some(client) = &self.client {
            let _ = client.send(Line::new(
                "NOTICE",
                [from, "wakes resumed: held wakes are let through again"],
            ));
        }
        true
    }

    /// [`zirc_core::ipc::Request::WakesResume`].
    pub(super) fn wakes_resume(&mut self) -> BreakerState {
        tracing::info!("wakes resume, as asked");
        self.breaker.resume();
        self.breaker.state(self.clock.now())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Org, Watcher};
    use super::*;
    use tokio::sync::mpsc;
    use zirc_core::ipc::{Delivery, Direct};
    use zirc_proto::Message;

    /// A daemon with one watch, of `cc-1`, and what it hears.
    fn daemon_watched_by_cc1(home: &std::path::Path) -> (Daemon, mpsc::Receiver<Response>) {
        let org = Org {
            name: "zirc".into(),
            first: true,
            state: home.to_owned(),
            several: false,
            guest: None,
        };
        let (inputs, _received) = mpsc::unbounded_channel();
        let mut daemon = Daemon::new(home.to_owned(), org, "irc.example".into(), None, inputs);
        let (heard, receiver) = mpsc::channel(64);
        daemon.watchers.push(Watcher {
            heard,
            session: Some("cc-1".into()),
        });
        (daemon, receiver)
    }

    fn post(text: &str) -> Response {
        Response::Delivery(Delivery::new(
            "#api",
            "beta",
            Message::new("cc-2", Event::Text { body: text.into() }),
        ))
    }

    #[tokio::test]
    async fn past_its_limit_a_session_hears_notes_later_but_its_owners_messages_now() {
        let home = tempfile::tempdir().unwrap();
        let (mut daemon, _hears) = daemon_watched_by_cc1(home.path());
        for n in 0..PER_SESSION {
            let mut reasons = [Some(Why::Mention)];
            daemon.gate_wakes(&post(&format!("cc-1: look at {n}")), &mut reasons);
            assert_eq!(reasons, [Some(Why::Mention)], "wake {n}");
        }
        // The next is held as a note, and the owner is told once.
        let mut reasons = [Some(Why::Mention)];
        daemon.gate_wakes(&post("cc-1: and this"), &mut reasons);
        assert_eq!(reasons, [None]);
        assert_eq!(daemon.later["cc-1"].len(), 1);
        let mut reasons = [Some(Why::Mention)];
        daemon.gate_wakes(&post("cc-1: and that"), &mut reasons);
        assert_eq!(reasons, [None]);
        assert_eq!(daemon.breaker.pending.len(), 1, "one notice per trip");
        // The breaker's own notice wakes nobody, whoever it names.
        let notice = daemon.breaker.pending[0].clone();
        assert!(
            notice.starts_with("wakes paused: 6 wakes of cc-1 in 10 min\n"),
            "{notice:?}"
        );
        let mut reasons = [Some(Why::Mention)];
        daemon.gate_wakes(&post(&notice), &mut reasons);
        assert_eq!(reasons, [None]);
        // The owner's direct message still wakes it.
        let direct = Response::Direct(Direct {
            from: "mattias".into(),
            from_session: None,
            session: Some("cc-1".into()),
            text: "stop and look at this".into(),
            query: None,
            note: false,
            answer: None,
            id: None,
            why: None,
            org: None,
        });
        let mut reasons = [Some(Why::Direct)];
        daemon.gate_wakes(&direct, &mut reasons);
        assert_eq!(reasons, [Some(Why::Direct)]);
        // Nor is what it asked for held, an answer to its own question or a wait of its own,
        // and neither counts.
        let wakes = daemon.breaker.state(daemon.clock.now()).wakes;
        let Response::Direct(owners) = &direct else {
            unreachable!()
        };
        let answer = Response::Direct(Direct {
            from: "mattias-mini".into(),
            from_session: Some("cc-2".into()),
            answer: Some("q2".into()),
            ..owners.clone()
        });
        let mut reasons = [Some(Why::Direct)];
        daemon.gate_wakes(&answer, &mut reasons);
        assert_eq!(reasons, [Some(Why::Direct)]);
        let mut reasons = [Some(Why::Awaited)];
        daemon.gate_wakes(&post("cc-1: landed login as 1a2b3c4"), &mut reasons);
        assert_eq!(reasons, [Some(Why::Awaited)]);
        assert_eq!(daemon.breaker.state(daemon.clock.now()).wakes, wakes);
        // Another agent's query or note is held like any other wake.
        for (query, answer, note) in [(Some("q1"), None::<&str>, false), (None, None, true)] {
            let Response::Direct(owners) = &direct else {
                unreachable!()
            };
            let agents = Response::Direct(Direct {
                from: "mattias-mini".into(),
                from_session: Some("cc-2".into()),
                query: query.map(str::to_owned),
                answer: answer.map(str::to_owned),
                note,
                ..owners.clone()
            });
            let mut reasons = [Some(Why::Direct)];
            daemon.gate_wakes(&agents, &mut reasons);
            assert_eq!(reasons, [None], "{agents:?}");
            assert!(
                daemon.later["cc-1"]
                    .contains(&super::super::with_why(agents.clone(), Some(Why::Direct)))
            );
        }
        // Resumed, it wakes again.
        let _ = daemon.wakes_resume();
        let mut reasons = [Some(Why::Mention)];
        daemon.gate_wakes(&post("cc-1: now"), &mut reasons);
        assert_eq!(reasons, [Some(Why::Mention)]);
    }

    /// A workflow's `run` giving its `step` to `to`, as its nick posts it.
    fn given(run: &str, step: &str, to: &str) -> Response {
        let data = serde_json::json!({
            "run": run,
            "change": {"type": "assigned", "step": step, "to": [to]},
            "card": {"id": run, "title": run},
        });
        Response::Delivery(Delivery::new(
            "#ops",
            "flake_bot",
            Message::new(
                "flake_bot",
                Event::Custom {
                    kind: "x-zirc/run".into(),
                    data,
                    text: format!("`{run}` · `{step}` → @{to}"),
                },
            ),
        ))
    }

    #[tokio::test]
    async fn a_step_a_workflow_gives_the_session_wakes_it_once_past_its_limit() {
        let home = tempfile::tempdir().unwrap();
        let (mut daemon, _hears) = daemon_watched_by_cc1(home.path());
        for n in 0..=PER_SESSION {
            let mut reasons = [Some(Why::Mention)];
            daemon.gate_wakes(&post(&format!("cc-1: look at {n}")), &mut reasons);
        }
        let paused = daemon.breaker.state(daemon.clock.now()).paused_sessions;
        assert_eq!(paused, ["cc-1"]);
        // Each step given to it, to the session alone or as its machine's, still wakes it.
        for (n, to) in [(1, "m3-mac/cc-1"), (2, "cc-1"), (3, "m3-mac/cc-1")] {
            let mut reasons = [Some(Why::Mention)];
            daemon.gate_wakes(&given(&format!("flaky-{n}"), "fix", to), &mut reasons);
            assert_eq!(reasons, [Some(Why::Mention)], "flaky-{n}");
        }
        // The same step said again is held, as is a step given to another session.
        let mut reasons = [Some(Why::Mention)];
        daemon.gate_wakes(&given("flaky-1", "fix", "m3-mac/cc-1"), &mut reasons);
        assert_eq!(reasons, [None]);
        let mut reasons = [Some(Why::Mention)];
        daemon.gate_wakes(&given("flaky-9", "fix", "m3-mac/cc-2"), &mut reasons);
        assert_eq!(reasons, [None]);
    }

    const MIN: Duration = Duration::from_mins(1);

    #[test]
    fn a_storm_trips_it_once_and_it_resumes_when_the_window_drains() {
        let mut breaker = Breaker::default();
        let start = Instant::now();
        // Forty sessions woken once each: the machine's limit.
        for n in 0..MACHINE {
            let at = start + Duration::from_secs(u64::try_from(n).unwrap());
            assert_eq!(
                breaker.allow(&format!("cc-{n}"), "intent from cc-x", at),
                Verdict::Wake
            );
        }
        let at = start + MIN;
        let Verdict::Trip(notice) = breaker.allow("cc-new", "intent from cc-x", at) else {
            panic!("didn't trip")
        };
        assert_eq!(
            notice,
            "wakes paused: 20 wakes on this machine in 10 min\n\
             - top sources: intent from cc-x ×20\n\
             - held: they wait as notes for each session's next wake\n\
             - needed: reply `resume` to let them through now, or run `zirc wakes resume`"
        );
        // Open, it holds every other wake without saying so again.
        assert_eq!(breaker.allow("cc-1", "mention", at), Verdict::Hold);
        assert!(breaker.state(at).paused_machine);
        assert_eq!(breaker.state(at).held, 2);
        // Ten minutes on the window has drained, and wakes go through again.
        let later = start + WINDOW + MIN;
        assert_eq!(breaker.allow("cc-1", "mention", later), Verdict::Wake);
        assert!(!breaker.state(later).paused_machine);
    }

    #[test]
    fn one_session_woken_too_often_is_paused_alone() {
        let mut breaker = Breaker::default();
        let start = Instant::now();
        for n in 0..PER_SESSION {
            let at = start + Duration::from_secs(u64::try_from(n).unwrap() * 30);
            assert_eq!(breaker.allow("cc-1", "topic from ci", at), Verdict::Wake);
        }
        let at = start + 6 * MIN;
        // Every trip is said to the owner directly, which pushes to their phone.
        assert!(matches!(
            breaker.allow("cc-1", "topic from ci", at),
            Verdict::Trip(_)
        ));
        assert_eq!(breaker.allow("cc-1", "mention", at), Verdict::Hold);
        assert_eq!(breaker.allow("cc-2", "mention", at), Verdict::Wake);
        assert_eq!(breaker.state(at).paused_sessions, ["cc-1"]);
        // Resumed by hand, at once, and counted afresh.
        breaker.resume();
        assert_eq!(breaker.allow("cc-1", "mention", at), Verdict::Wake);
        for _ in 1..PER_SESSION {
            assert_eq!(breaker.allow("cc-1", "mention", at), Verdict::Wake);
        }
        assert!(matches!(
            breaker.allow("cc-1", "mention", at),
            Verdict::Trip(_)
        ));
    }

    #[test]
    fn a_busy_hour_never_trips_it() {
        let mut breaker = Breaker::default();
        let start = Instant::now();
        // Eight sessions, each woken every six minutes for an hour: 2 per window each,
        // about 16 per window across the machine.
        for minute in 0..60_u64 {
            for n in 0..8_u64 {
                if (minute + n) % 6 == 0 {
                    let at = start + Duration::from_secs(minute * 60 + n);
                    assert_eq!(
                        breaker.allow(&format!("cc-{n}"), "mention", at),
                        Verdict::Wake,
                        "minute {minute}, cc-{n}"
                    );
                }
            }
        }
    }
}
