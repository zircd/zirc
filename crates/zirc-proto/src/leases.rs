//! Leases on things a team's machines share, such as an emulator, a simulator or the CI
//! Mac: as many holders at a time as it has [`slots`] (three builds, one emulator), the others
//! queued in order, each lease ending by itself.
//!
//! There's no server to decide who holds what. Each machine's zircd says what its sessions
//! want, release and extend as [`LEASE_KIND`] events in the organisation's channel, and
//! every zircd works out the same holders from the same events: [`Leases::state`] depends
//! on the set of events and the time, never on the order they arrived in. Requests are
//! queued by when their machine made them, and one becomes the holder only [`SETTLE`]
//! after it was made, so a request made at the same moment elsewhere has reached every
//! machine before either is told the lease is theirs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The kind of the events leases are agreed through, in the organisation's channel: sent
/// out of its history ([`crate::live`]), as they say only how things stand.
pub const LEASE_KIND: &str = crate::LEASE_KIND;

/// How long after it's made a request can become the holder, in milliseconds: long enough
/// for a request made at the same moment on another machine to have arrived.
pub const SETTLE: u64 = 2_000;

/// How long a lease lasts when nobody says, in seconds.
pub const DEFAULT_SECONDS: u64 = 20 * 60;

/// The longest a lease may last at once, in seconds: extending it is how to hold it longer.
pub const MAX_SECONDS: u64 = 4 * 3600;

/// How long a queued request waits for its turn before it's dropped, in milliseconds, so a
/// machine that went away doesn't hold up the queue forever.
pub const WAIT_LIMIT: u64 = 8 * 3600 * 1000;

/// How many sessions may hold the lease `name` at once: three builds, as this machine has
/// room for, and one of anything else, such as an emulator. Every machine must agree, so
/// it's said here, once, and not in each one's config.
#[must_use]
pub fn slots(name: &str) -> usize {
    if name == "build" { 3 } else { 1 }
}

/// The longest a session holds a lease in a row by asking for it again while others wait
/// for it, in seconds, unless its own request asked for longer: past that it goes to the
/// back of the queue ([`renewal`]). A holder that asked again before every command never
/// let go (5 Nov 2026).
pub const HOLD_CAP: u64 = 30 * 60;

/// How far a holder's `turn` runs once it asks at `now` to hold it `seconds` longer: as
/// asked while nobody else waits, and while others do, no later than [`HOLD_CAP`] (or its
/// own request's length, if longer) after its turn started. With whether that cut it short,
/// when its session goes to the back of the queue. It's never earlier than the turn's end.
#[must_use]
pub fn renewal(turn: &Turn, others_wait: bool, now: u64, seconds: u64) -> (u64, bool) {
    let asked = now + seconds.min(MAX_SECONDS) * 1000;
    let cap = turn.start + turn.want.seconds.max(HOLD_CAP) * 1000;
    let until = if others_wait { asked.min(cap) } else { asked };
    (until.max(turn.end), others_wait && asked > cap)
}

/// What a lease event says, as the data of a [`LEASE_KIND`] event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum LeaseEvent {
    /// A session wants the lease `name`.
    Want(Want),
    /// The request `id` is done with: given back, or no longer waited for.
    Release {
        /// The request.
        id: String,
        /// When, in milliseconds since the Unix epoch, by the releasing machine's clock.
        at: u64,
    },
    /// The request `id`, holding its lease, holds it until `until`.
    Extend {
        /// The request.
        id: String,
        /// Until when, in milliseconds since the Unix epoch.
        until: u64,
    },
}

/// A session's request for a lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Want {
    /// The request's id, unique across machines: its machine's nick, the time and a count.
    pub id: String,
    /// The lease, such as `emulator`.
    pub name: String,
    /// The machine that asked, by nick.
    pub nick: String,
    /// The session that asked.
    pub session: String,
    /// When it was made, in milliseconds since the Unix epoch, by its machine's clock.
    pub at: u64,
    /// How long it holds the lease once it's theirs, in seconds.
    pub seconds: u64,
}

/// A lease as it is: who holds it and who's waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Lease {
    /// Its name.
    pub name: String,
    /// Who holds it, if anyone does: the first of [`Lease::holders`], for readers from
    /// before a lease had [`slots`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<Holder>,
    /// Everyone holding it, at most [`slots`] of them, the one holding it longest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holders: Vec<Holder>,
    /// Who's waiting for it, next first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<Holder>,
    /// This machine's sessions whose requests it holds back until it has room, as its
    /// `config.toml` says (`[leases.<name>]`): they've not asked the others yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<Held>,
}

/// A session's request this machine holds back until it has room for the lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Held {
    /// The session.
    pub session: String,
    /// Why, such as `load 52 is over 40`.
    pub why: String,
}

/// A session holding or waiting for a lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Holder {
    /// Its machine, by nick.
    pub nick: String,
    /// The session.
    pub session: String,
    /// When it holds the lease until, or when waiting, until when it'd hold it if its turn
    /// came now: in seconds since the Unix epoch.
    pub until: u64,
    /// Whether it's this machine's.
    #[serde(default)]
    pub mine: bool,
}

/// What's been said about leases, from which who holds what follows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leases {
    /// Every request heard and not yet forgotten, by id.
    wants: BTreeMap<String, Want>,
    /// When each released request was released.
    released: BTreeMap<String, u64>,
    /// Until when each extended request holds its lease.
    extended: BTreeMap<String, u64>,
}

/// One request's turn, as worked out from everything heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// The request.
    pub want: Want,
    /// When it starts, or started, holding the lease, in milliseconds.
    pub start: u64,
    /// When it stops, in milliseconds.
    pub end: u64,
}

impl Leases {
    /// Takes in `event`, from this machine or another.
    pub fn apply(&mut self, event: LeaseEvent) {
        match event {
            LeaseEvent::Want(want) => {
                self.wants.insert(want.id.clone(), want);
            }
            LeaseEvent::Release { id, at } => {
                let earliest = self.released.get(&id).map_or(at, |&before| before.min(at));
                self.released.insert(id, earliest);
            }
            LeaseEvent::Extend { id, until } => {
                let latest = self
                    .extended
                    .get(&id)
                    .map_or(until, |&before| before.max(until));
                self.extended.insert(id, latest);
            }
        }
    }

    /// What the machine `nick` said that's still remembered: each of its requests, and
    /// their releases and extensions. Taking them in again changes nothing, so they can be
    /// said again to a machine that missed them.
    #[must_use]
    pub fn events_of(&self, nick: &str) -> Vec<LeaseEvent> {
        let mut events = Vec::new();
        for want in self.wants.values().filter(|want| want.nick == nick) {
            events.push(LeaseEvent::Want(want.clone()));
            if let Some(&at) = self.released.get(&want.id) {
                events.push(LeaseEvent::Release {
                    id: want.id.clone(),
                    at,
                });
            }
            if let Some(&until) = self.extended.get(&want.id) {
                events.push(LeaseEvent::Extend {
                    id: want.id.clone(),
                    until,
                });
            }
        }
        events
    }

    /// Each request's turn at the lease `name`, in order, leaving out those released before
    /// their turn came or that waited too long.
    fn turns(&self, name: &str) -> Vec<Turn> {
        let mut wants: Vec<&Want> = self.wants.values().filter(|it| it.name == name).collect();
        wants.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        // When each of its slots is free from: a request takes the one free soonest, and
        // never while its own session holds another.
        let mut free_from = vec![0; slots(name).max(1)];
        let mut turns: Vec<Turn> = Vec::new();
        // When each session's latest turn ends: looked up, not searched for among the turns
        // so far, which made this quadratic in a lease's requests.
        let mut own_until: HashMap<(&str, &str), u64> = HashMap::new();
        for want in wants {
            let (slot, &free) = free_from
                .iter()
                .enumerate()
                .min_by_key(|(slot, free)| (**free, *slot))
                .expect("a lease has a slot");
            let session = (want.nick.as_str(), want.session.as_str());
            let own = own_until.get(&session).copied().unwrap_or(0);
            let start = (want.at + SETTLE).max(free).max(own);
            if start > want.at + WAIT_LIMIT {
                continue;
            }
            let released = self.released.get(&want.id).copied();
            if released.is_some_and(|at| at <= start) {
                continue;
            }
            let seconds = want.seconds.min(MAX_SECONDS);
            let mut end = start + seconds * 1000;
            if let Some(&until) = self.extended.get(&want.id) {
                end = end.max(until);
            }
            if let Some(at) = released {
                end = end.min(at);
            }
            free_from[slot] = end;
            let until = own_until.entry(session).or_insert(0);
            *until = (*until).max(end);
            turns.push(Turn {
                want: want.clone(),
                start,
                end,
            });
        }
        turns
    }

    /// Whether the machine `nick` has a request that still counts at `now`, in milliseconds:
    /// one holding a lease or waiting for its turn. Only then is it worth saying its events
    /// again to one who missed them.
    #[must_use]
    pub fn has_current(&self, nick: &str, now: u64) -> bool {
        let mut names: Vec<&str> = self
            .wants
            .values()
            .filter(|want| want.nick == nick)
            .map(|want| want.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        names.into_iter().any(|name| {
            self.turns(name)
                .iter()
                .any(|turn| turn.want.nick == nick && turn.end > now)
        })
    }

    /// Every lease as it is at `now`, in milliseconds since the Unix epoch, by name, with
    /// `me` the nick of this machine.
    #[must_use]
    pub fn state(&self, now: u64, me: &str) -> Vec<Lease> {
        let names = self.names();
        names
            .into_iter()
            .filter_map(|name| {
                let lease = self.lease(name, now, me);
                (!lease.holders.is_empty() || !lease.waiting.is_empty()).then_some(lease)
            })
            .collect()
    }

    /// The lease `name` as it is at `now`, with `me` this machine's nick.
    #[must_use]
    pub fn lease(&self, name: &str, now: u64, me: &str) -> Lease {
        let mut lease = Lease {
            name: name.to_owned(),
            holder: None,
            holders: Vec::new(),
            waiting: Vec::new(),
            held: Vec::new(),
        };
        let mut turns = self.turns(name);
        turns.sort_by_key(|turn| turn.start);
        for turn in turns {
            if turn.end <= now {
                continue;
            }
            let who = Holder {
                nick: turn.want.nick.clone(),
                session: turn.want.session.clone(),
                until: turn.end / 1000,
                mine: turn.want.nick == me,
            };
            if turn.start <= now {
                lease.holders.push(who);
            } else {
                lease.waiting.push(Holder {
                    until: (now.max(turn.want.at + SETTLE) + turn.end - turn.start) / 1000,
                    ..who
                });
            }
        }
        lease.holder = lease.holders.first().cloned();
        lease
    }

    /// The turn at `now` of the request this machine, `me`, made for `session` at the lease
    /// `name`: holding or waiting.
    #[must_use]
    pub fn turn_of(&self, name: &str, me: &str, session: &str, now: u64) -> Option<Turn> {
        self.turns(name)
            .into_iter()
            .find(|turn| turn.end > now && turn.want.nick == me && turn.want.session == session)
    }

    /// Whether another session than `turn`'s waits for its lease after it at `now`.
    #[must_use]
    pub fn others_wait(&self, turn: &Turn, now: u64) -> bool {
        self.turns(&turn.want.name).iter().any(|other| {
            other.start > now
                && other.end > now
                && (other.want.nick != turn.want.nick || other.want.session != turn.want.session)
        })
    }

    /// Whether the session of `turn` has asked again, after it, for its lease.
    #[must_use]
    pub fn asked_again(&self, turn: &Turn) -> bool {
        self.turns(&turn.want.name).iter().any(|other| {
            other.start > turn.start
                && other.want.nick == turn.want.nick
                && other.want.session == turn.want.session
        })
    }

    /// The requests this machine, `me`, made that are still holding or waiting at `now`.
    #[must_use]
    pub fn mine(&self, me: &str, now: u64) -> Vec<Turn> {
        let names = self.names();
        names
            .into_iter()
            .flat_map(|name| self.turns(name))
            .filter(|turn| turn.end > now && turn.want.nick == me)
            .collect()
    }

    /// The names of the leases there are requests for, each once, in order.
    fn names(&self) -> BTreeSet<&str> {
        self.wants.values().map(|it| it.name.as_str()).collect()
    }

    /// Forgets what no longer matters at `now`: requests long over. Whether any went.
    ///
    /// zircd runs this every second. It once worked every lease's turns out again for each
    /// request there was, and looked each request up in a list: with the few hundred that a
    /// day of builds leaves (each is kept [`WAIT_LIMIT`]), that was a core of this Mac, all
    /// the time (12 Nov 2026).
    pub fn prune(&mut self, now: u64) -> bool {
        let live: HashSet<String> = self
            .names()
            .into_iter()
            .flat_map(|name| self.turns(name))
            .filter(|turn| turn.end > now)
            .map(|turn| turn.want.id)
            .collect();
        // Kept a while after its turn, so a late copy of its events changes nothing.
        let recent = |want: &Want| want.at + WAIT_LIMIT > now;
        let before = (self.wants.len(), self.released.len(), self.extended.len());
        self.wants
            .retain(|id, want| live.contains(id) || recent(want));
        let wants = &self.wants;
        self.released.retain(|id, _| wants.contains_key(id));
        self.extended.retain(|id, _| wants.contains_key(id));
        before != (self.wants.len(), self.released.len(), self.extended.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn want(id: &str, name: &str, nick: &str, session: &str, at: u64, minutes: u64) -> LeaseEvent {
        LeaseEvent::Want(Want {
            id: id.into(),
            name: name.into(),
            nick: nick.into(),
            session: session.into(),
            at,
            seconds: minutes * 60,
        })
    }

    /// A day of builds leaves hundreds of requests, each kept for hours, and zircd prunes
    /// every second. With a few thousand this ends at once; it once took minutes, working
    /// every turn out again for each request, so a slip back shows as a test that never ends.
    #[test]
    fn pruning_thousands_of_requests_is_no_work_to_speak_of() {
        let mut leases = Leases::default();
        let many = 4000;
        for n in 0..many {
            let at = n * MIN;
            let id = format!("alpha-{n}");
            leases.apply(want(
                &id,
                "build",
                "alpha",
                &format!("cc-{}", n % 7),
                at,
                10,
            ));
            leases.apply(LeaseEvent::Release {
                id,
                at: at + MIN / 2,
            });
        }
        let now = many * MIN;
        // Each pruned as before: all but the ones still within their wait are gone.
        assert!(leases.prune(now));
        let kept = usize::try_from(WAIT_LIMIT / MIN).unwrap();
        assert!(
            leases.wants.len() <= kept + 1,
            "{} kept",
            leases.wants.len()
        );
        assert_eq!(leases.released.len(), leases.wants.len());
        // Nothing more to forget: it says so, and zircd saves nothing.
        assert!(!leases.prune(now));
        assert!(leases.mine("alpha", now).is_empty());
    }

    #[test]
    fn a_machine_has_current_requests_while_one_holds_or_waits() {
        let mut leases = Leases::default();
        assert!(!leases.has_current("alpha", 0));
        leases.apply(want("a1", "emulator", "alpha", "cc-1", 0, 10));
        leases.apply(want("b1", "emulator", "beta", "cc-2", 1, 10));
        // Alpha holds it, beta waits: both count.
        assert!(leases.has_current("alpha", 5_000));
        assert!(leases.has_current("beta", 5_000));
        // Alpha gives it back: only beta's counts.
        leases.apply(LeaseEvent::Release {
            id: "a1".into(),
            at: 60_000,
        });
        assert!(!leases.has_current("alpha", 60_001));
        assert!(leases.has_current("beta", 60_001));
        // Beta's turn ends by itself.
        assert!(!leases.has_current("beta", 60_000 + 10 * 60_000 + 1));
    }

    fn holder(leases: &Leases, now: u64) -> Option<String> {
        let lease = leases.lease("emulator", now, "alpha");
        lease.holder.map(|it| format!("{}/{}", it.nick, it.session))
    }

    #[test]
    fn a_machines_events_said_again_change_nothing() {
        let mut leases = Leases::default();
        leases.apply(want("a1", "emulator", "alpha", "cc-1", 1_000, 20));
        leases.apply(want("b1", "emulator", "beta", "cc-2", 2_000, 20));
        leases.apply(LeaseEvent::Extend {
            id: "a1".into(),
            until: 9_000_000,
        });
        leases.apply(LeaseEvent::Release {
            id: "a1".into(),
            at: 50_000,
        });
        let alphas = leases.events_of("alpha");
        assert_eq!(alphas.len(), 3, "{alphas:?}");
        // Another machine that heard only beta's hears alpha's again: the same state.
        let mut other = Leases::default();
        other.apply(want("b1", "emulator", "beta", "cc-2", 2_000, 20));
        for event in alphas.clone() {
            other.apply(event);
        }
        assert_eq!(other, leases);
        // Twice is the same as once.
        for event in alphas {
            other.apply(event);
        }
        assert_eq!(other, leases);
    }

    #[test]
    fn one_holds_at_a_time_and_the_rest_wait_in_order() {
        let mut leases = Leases::default();
        leases.apply(want("a", "emulator", "alpha", "cc-1", 0, 20));
        leases.apply(want("b", "emulator", "beta", "cc-2", 1_000, 10));
        // Nobody holds it until it's settled.
        assert_eq!(holder(&leases, 1_500), None);
        assert_eq!(holder(&leases, SETTLE), Some("alpha/cc-1".into()));
        let lease = leases.lease("emulator", SETTLE, "alpha");
        assert!(lease.holder.unwrap().mine);
        assert_eq!(lease.waiting.len(), 1);
        assert_eq!(lease.waiting[0].session, "cc-2");
        // It ends by itself, and the next one's turn starts.
        assert_eq!(holder(&leases, SETTLE + 20 * MIN), Some("beta/cc-2".into()));
        assert_eq!(holder(&leases, SETTLE + 30 * MIN), None);
        assert!(leases.state(SETTLE + 30 * MIN, "alpha").is_empty());
    }

    #[test]
    fn every_machine_agrees_whatever_order_it_heard_things_in() {
        let events = [
            want("b", "emulator", "beta", "cc-2", 1_000, 10),
            want("a", "emulator", "alpha", "cc-1", 0, 20),
            LeaseEvent::Release {
                id: "a".into(),
                at: 5 * MIN,
            },
            LeaseEvent::Extend {
                id: "b".into(),
                until: 40 * MIN,
            },
        ];
        let mut forwards = Leases::default();
        let mut backwards = Leases::default();
        for event in &events {
            forwards.apply(event.clone());
        }
        for event in events.iter().rev() {
            backwards.apply(event.clone());
        }
        for now in [0, SETTLE, 4 * MIN, 6 * MIN, 30 * MIN, 39 * MIN, 41 * MIN] {
            assert_eq!(forwards.state(now, "x"), backwards.state(now, "x"), "{now}");
        }
        // Released early, the next starts then, and holds it as long as it was extended.
        assert_eq!(holder(&forwards, 5 * MIN + 1), Some("beta/cc-2".into()));
        assert_eq!(holder(&forwards, 39 * MIN), Some("beta/cc-2".into()));
        assert_eq!(holder(&forwards, 41 * MIN), None);
    }

    #[test]
    fn a_request_released_before_its_turn_never_holds_it() {
        let mut leases = Leases::default();
        leases.apply(want("a", "emulator", "alpha", "cc-1", 0, 20));
        leases.apply(want("b", "emulator", "beta", "cc-2", 1_000, 10));
        leases.apply(want("c", "emulator", "gamma", "cc-3", 2_000, 10));
        leases.apply(LeaseEvent::Release {
            id: "b".into(),
            at: MIN,
        });
        assert_eq!(
            holder(&leases, SETTLE + 20 * MIN),
            Some("gamma/cc-3".into())
        );
        // Other leases are their own.
        leases.apply(want("d", "simulator", "beta", "cc-2", 0, 5));
        assert_eq!(leases.state(SETTLE, "alpha").len(), 2);
    }

    #[test]
    fn a_holder_asking_again_while_others_wait_holds_it_no_more_than_the_cap() {
        let mut leases = Leases::default();
        leases.apply(want("a", "emulator", "alpha", "cc-1", 0, 20));
        let turn = leases.turn_of("emulator", "alpha", "cc-1", SETTLE).unwrap();
        // Nobody waits: as long as asked, again and again.
        assert!(!leases.others_wait(&turn, 10 * MIN));
        assert_eq!(renewal(&turn, false, 50 * MIN, 20 * 60), (70 * MIN, false));
        // Beta waits: alpha holds it 30 minutes in a row at most.
        leases.apply(want("b", "emulator", "beta", "cc-2", MIN, 20));
        assert!(leases.others_wait(&turn, 10 * MIN));
        assert_eq!(renewal(&turn, true, 5 * MIN, 20 * 60), (25 * MIN, false));
        assert_eq!(
            renewal(&turn, true, 15 * MIN, 20 * 60),
            (SETTLE + 30 * MIN, true)
        );
        // Never earlier than it already ends.
        let long = Turn {
            end: 40 * MIN,
            ..turn.clone()
        };
        assert_eq!(renewal(&long, true, 15 * MIN, 20 * 60), (40 * MIN, true));
        // Its own request's length, when that's longer than the cap.
        let hour = Turn {
            want: Want {
                seconds: 3600,
                ..turn.want.clone()
            },
            ..turn.clone()
        };
        assert_eq!(renewal(&hour, true, 15 * MIN, 20 * 60), (35 * MIN, false));
        // Asked again after it, alpha waits behind beta.
        assert!(!leases.asked_again(&turn));
        leases.apply(want("a2", "emulator", "alpha", "cc-1", 16 * MIN, 20));
        assert!(leases.asked_again(&turn));
        let after = leases
            .turn_of("emulator", "beta", "cc-2", 21 * MIN)
            .unwrap();
        assert!(leases.others_wait(&after, 21 * MIN));
    }

    #[test]
    fn three_hold_a_build_at_once_and_the_fourth_takes_whichever_frees_first() {
        let mut leases = Leases::default();
        leases.apply(want("a", "build", "alpha", "cc-1", 0, 20));
        leases.apply(want("b", "build", "beta", "cc-2", 1_000, 10));
        leases.apply(want("c", "build", "gamma", "cc-3", 2_000, 15));
        leases.apply(want("d", "build", "delta", "cc-4", 3_000, 10));
        let who = |lease: &Lease| {
            let names = |list: &[Holder]| list.iter().map(|it| it.session.clone()).collect();
            (names(&lease.holders), names(&lease.waiting))
        };
        let lease = leases.lease("build", 5_000, "alpha");
        assert_eq!(
            who(&lease),
            (
                vec!["cc-1".into(), "cc-2".into(), "cc-3".into()],
                vec!["cc-4".into()]
            )
        );
        assert_eq!(lease.holder.unwrap().session, "cc-1");
        // Beta's ends first: delta gets its slot, while alpha and gamma still hold theirs.
        let lease = leases.lease("build", 1_000 + SETTLE + 10 * MIN, "alpha");
        assert_eq!(
            who(&lease),
            (
                vec!["cc-1".into(), "cc-3".into(), "cc-4".into()],
                Vec::new()
            )
        );
        // A session never holds both: alpha asking again waits for its own to end.
        leases.apply(want("a2", "build", "alpha", "cc-1", 4_000, 10));
        let again = leases
            .turns("build")
            .into_iter()
            .find(|it| it.want.id == "a2");
        assert_eq!(again.unwrap().start, SETTLE + 20 * MIN);
        // An emulator is still one at a time.
        assert_eq!(slots("emulator"), 1);
    }

    #[test]
    fn says_what_this_machine_holds_or_waits_for_and_forgets_whats_over() {
        let mut leases = Leases::default();
        leases.apply(want("a", "emulator", "alpha", "cc-1", 0, 20));
        leases.apply(want("b", "emulator", "beta", "cc-2", 1_000, 10));
        let turn = leases.turn_of("emulator", "beta", "cc-2", SETTLE).unwrap();
        assert_eq!(turn.start, SETTLE + 20 * MIN);
        assert_eq!(leases.mine("alpha", SETTLE).len(), 1);
        assert!(leases.turn_of("emulator", "beta", "cc-9", SETTLE).is_none());
        leases.prune(WAIT_LIMIT + 31 * MIN);
        assert_eq!(leases, Leases::default());
    }
}
