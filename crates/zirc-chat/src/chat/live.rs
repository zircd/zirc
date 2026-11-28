//! Which agent sessions are online, away or ended, as their machines say
//! ([`zirc_proto::session::Live`]): one rule every app draws. A session is online strictly
//! while a `zirc watch` of its is open, and away otherwise (the user's choice, 23 Nov 2026),
//! with since when and how many people's messages wait for it; its machine leaving the
//! server makes all its sessions away. One its machine no longer names has ended: away for
//! [`AWAY_FOLD_SECS`]. A session is listed from its first watch, before it says anything,
//! and its row folds once its task lands or it's been away 12 hours ([`super::DirectRow::folded`]),
//! its conversation kept. A machine whose zircd predates `away` says only who's live, with
//! a grace; one predating the lists says nothing, and its sessions fold once quiet as long.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, TimeDelta, Utc};
use zirc_proto::session::{AWAY_FOLD_SECS, LIVE_GRACE_SECS, Live};

use super::{Chat, fold, profile::ago_short};

/// What a machine last said of its sessions.
#[derive(Debug, Clone)]
pub(super) struct MachineLive {
    /// The machine's nick, as it said it.
    nick: String,
    /// What it said.
    live: Live,
    /// When it said so.
    at: DateTime<Utc>,
    /// Since when it's been gone from the server, if it has.
    gone: Option<DateTime<Utc>>,
    /// Since when we've been unsure it's still as it said, as we connected again without
    /// hearing from it yet: it holds for [`LIVE_GRACE_SECS`], as the machine says it again
    /// when we join its channels.
    unsure: Option<DateTime<Utc>>,
    /// The sessions it has stopped naming, each with when it last watched: when its list
    /// said it went away, or else when its list first left it out.
    ended: BTreeMap<String, DateTime<Utc>>,
}

/// Whether an agent session is listening, as its machine says ([`Chat::listening`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listening {
    /// A `zirc watch` of its is open: a message reaches it at once.
    Online,
    /// No watch of its is open, since `since`, with `waiting` messages from people for it.
    Away {
        /// When its last watch closed, or its machine left.
        since: DateTime<Utc>,
        /// How many messages from people wait for it.
        waiting: u32,
    },
    /// Its machine no longer names it: away [`AWAY_FOLD_SECS`], or it stopped long ago.
    Ended {
        /// When it last watched, as far as anyone knows.
        since: Option<DateTime<Utc>>,
    },
}

/// `secs` as a span of time.
fn secs(secs: u64) -> TimeDelta {
    TimeDelta::seconds(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// How long after we connected again a machine's list still holds, unheard.
fn grace() -> TimeDelta {
    secs(LIVE_GRACE_SECS)
}

/// How long away a session's row stays in Messages: 12 hours.
pub(super) fn away_fold() -> TimeDelta {
    secs(AWAY_FOLD_SECS)
}

/// `at`, seconds since the Unix epoch, as a time.
fn epoch(at: u64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(i64::try_from(at).ok()?, 0)
}

impl Chat {
    /// What the machine `nick` said at `at` of its sessions: it replaces what it said
    /// before, unless that was newer.
    pub(super) fn learn_live(&mut self, nick: &str, live: Live, at: DateTime<Utc>) {
        let key = fold(nick);
        if self.live.get(&key).is_some_and(|known| known.at > at) {
            return;
        }
        let named = |live: &Live, session: &str| {
            live.sessions.iter().any(|s| s == session) || live.away.contains_key(session)
        };
        // What it named before and doesn't now ended now, last watching when it said it
        // went away, if it did; what's named again lives on.
        let mut ended = BTreeMap::new();
        if let Some(known) = self.live.get(&key) {
            ended.clone_from(&known.ended);
            let before = known.live.sessions.iter().chain(known.live.away.keys());
            for gone in before.filter(|session| !named(&live, session)) {
                let since = known.live.away.get(gone).and_then(|away| epoch(away.since));
                ended.entry(gone.clone()).or_insert(since.unwrap_or(at));
            }
        }
        ended.retain(|session, _| !named(&live, session));
        let listed: BTreeSet<String> = live
            .sessions
            .iter()
            .chain(live.away.keys())
            .cloned()
            .collect();
        self.live.insert(
            key,
            MachineLive {
                nick: nick.to_owned(),
                live,
                at,
                gone: None,
                unsure: None,
                ended,
            },
        );
        // A session we wrote to before it answered has its conversation once we know it.
        for session in listed {
            self.adopt_agent(nick, &session);
        }
    }

    /// Our own agent sessions their machines name, live or away, by their conversations'
    /// names: each has a row in Messages from its first watch, before it says anything (the
    /// user's choice, 24 Nov 2026); a teammate's only once something's said with it.
    pub(super) fn our_sessions(&self) -> BTreeSet<String> {
        self.live
            .values()
            .filter(|known| self.ours(&known.nick))
            .flat_map(|known| {
                known
                    .live
                    .sessions
                    .iter()
                    .chain(known.live.away.keys())
                    .filter(|session| !session.is_empty() && *session != zirc_proto::NO_SESSION)
                    .map(|session| super::agent_channel(&known.nick, session))
            })
            .collect()
    }

    /// Every agent session any machine names, online or away, as `(machine, session)`, once
    /// each, though a machine names one away within its grace in both lists: the office's
    /// hot desks are those at no coworker's desk ([`Chat::office`]).
    pub(super) fn named_sessions(&self) -> Vec<(String, String)> {
        self.live
            .values()
            .flat_map(|known| {
                known
                    .live
                    .sessions
                    .iter()
                    .chain(known.live.away.keys())
                    .filter(|session| !session.is_empty() && *session != zirc_proto::NO_SESSION)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .map(|session| (known.nick.clone(), session.clone()))
            })
            .collect()
    }

    /// Whether the agent session `session` is known to run on the machine `nick`: the
    /// machine has listed it, live or ended, or it has spoken as from that machine.
    pub(super) fn known_on(&self, nick: &str, session: &str) -> bool {
        self.live
            .get(&fold(nick))
            .is_some_and(|known| known.names(session) || known.ended.contains_key(session))
            || self
                .session_from
                .get(session)
                .is_some_and(|from| from.eq_ignore_ascii_case(nick))
    }

    /// The machine `nick` left the server at `at`: its sessions are away from then, as
    /// nothing reaches them, until it says its list again.
    pub(super) fn live_gone(&mut self, nick: &str, at: DateTime<Utc>) {
        if let Some(known) = self.live.get_mut(&fold(nick)) {
            known.gone.get_or_insert(at);
        }
    }

    /// We connected at `at`: what machines said before holds for [`LIVE_GRACE_SECS`], as
    /// each says it again when we join its channels, and after that they count as gone.
    pub(super) fn live_unsure(&mut self, at: DateTime<Utc>) {
        for known in self.live.values_mut() {
            known.unsure.get_or_insert(at);
        }
    }

    /// Whether the agent session `session` on the machine `nick` is listening, as its
    /// machine says: `None` when its machine has never said, as zircds before the lists
    /// don't, unless it said a message to the session reached no one, as no session of that
    /// name works there ([`zirc_proto::receipt::State::Undelivered`]): then it has ended,
    /// when it was last heard.
    /// Every app's dot, row line and header read it.
    #[must_use]
    pub fn listening(&self, nick: &str, session: &str) -> Option<Listening> {
        let Some(known) = self.live.get(&fold(nick)) else {
            let refused = self
                .receipt(&super::agent_channel(nick, session))
                .is_some_and(|(_, receipt)| {
                    receipt.state == zirc_proto::receipt::State::Undelivered
                });
            return refused.then(|| Listening::Ended {
                since: self.heard(nick, session),
            });
        };
        let now = (self.clock)();
        // Gone from the server, or unheard since we connected again past the grace.
        let gone = known
            .gone
            .into_iter()
            .chain(
                known
                    .unsure
                    .map(|since| since + grace())
                    .filter(|end| *end <= now),
            )
            .min();
        if let Some(away) = known.live.away.get(session) {
            let since = epoch(away.since).unwrap_or(known.at);
            let since = gone.map_or(since, |gone| since.min(gone));
            return Some(Listening::Away {
                since,
                waiting: away.waiting,
            });
        }
        if known.live.sessions.iter().any(|live| live == session) {
            return Some(match gone {
                Some(since) => Listening::Away { since, waiting: 0 },
                None => Listening::Online,
            });
        }
        Some(Listening::Ended {
            since: known.ended.get(session).copied(),
        })
    }

    /// The agent session `session` on the machine `nick`'s dot, the one value its row in
    /// Messages, its header, its card and its profile card all read: here only while a watch
    /// of its is open, away once it closes (the user's rule, 23 Nov 2026); `None` once it has
    /// ended, or when its machine has never said ([`Chat::listening`]).
    #[must_use]
    pub fn session_presence(&self, nick: &str, session: &str) -> Option<super::Presence> {
        match self.listening(nick, session)? {
            Listening::Online => Some(super::Presence::Here),
            Listening::Away { .. } => Some(super::Presence::Away),
            Listening::Ended { .. } => None,
        }
    }

    /// Whether the agent session `session` on the machine `nick` is known to its machine:
    /// `Some(true)` while it's online or away ([`Chat::listening`]), `Some(false)` once it
    /// has ended, and `None` when its machine has never said.
    #[must_use]
    pub fn session_live(&self, nick: &str, session: &str) -> Option<bool> {
        self.listening(nick, session)
            .map(|listening| !matches!(listening, Listening::Ended { .. }))
    }

    /// When the agent session `session` on the machine `nick` last listened, while it
    /// isn't: since when it's been away, or when it last watched before it ended. `None`
    /// while it's online, or when nobody knows.
    #[must_use]
    pub fn listened_at(&self, nick: &str, session: &str) -> Option<DateTime<Utc>> {
        match self.listening(nick, session)? {
            Listening::Online => None,
            Listening::Away { since, .. } => Some(since),
            Listening::Ended { since } => since,
        }
    }

    /// What an agent away with messages waiting says, on its row and its header: `Not
    /// listening for 17 min · 1 message waiting` ([`zirc_proto::session::not_listening`]),
    /// at once (the user's choice, 23 Nov 2026). `None` while it's online, or nothing waits.
    #[must_use]
    pub fn not_listening(&self, nick: &str, session: &str) -> Option<String> {
        self.waiting_away(nick, session)
            .map(|(away, waiting)| zirc_proto::session::not_listening(away, waiting))
    }

    /// The same, short, for its row in Messages: `17m away · 1 waiting`
    /// ([`zirc_proto::session::not_listening_short`]).
    #[must_use]
    pub fn not_listening_short(&self, nick: &str, session: &str) -> Option<String> {
        self.waiting_away(nick, session)
            .map(|(away, waiting)| zirc_proto::session::not_listening_short(away, waiting))
    }

    /// How long, in seconds, the agent session `session` on the machine `nick` has been away
    /// with messages waiting, and how many: `None` while it's online, or nothing waits.
    fn waiting_away(&self, nick: &str, session: &str) -> Option<(u64, u32)> {
        let Some(Listening::Away { since, waiting }) = self.listening(nick, session) else {
            return None;
        };
        let away = u64::try_from(((self.clock)() - since).num_seconds()).unwrap_or(0);
        (waiting > 0).then_some((away, waiting))
    }

    /// The nick of the machine that runs the agent session `session`, for a chip naming it
    /// to open its card: the machine that says it's live, else the one that last said it
    /// was, else the machine whose conversation, or whose agent's, `here` is. `None` when
    /// none of those knows it.
    #[must_use]
    pub fn session_machine(&self, session: &str, here: Option<&str>) -> Option<String> {
        self.live
            .values()
            .filter(|known| known.names(session))
            .max_by_key(|known| {
                let live = self.session_live(&known.nick, session) == Some(true);
                (live, known.at)
            })
            .map(|known| known.nick.clone())
            .or_else(|| {
                let here = here?;
                let nick = super::agent(here).map_or(here, |(nick, _)| nick);
                self.machine(nick).map(|(_, nick)| nick)
            })
    }

    /// Whether the agent session `name`, `<machine>/<session>`, has ended
    /// ([`Chat::session_live`]).
    #[must_use]
    pub fn session_ended(&self, name: &str) -> bool {
        super::agent(name)
            .is_some_and(|(nick, session)| self.session_live(nick, session) == Some(false))
    }

    /// The agent sessions online or away of the machines `keep` keeps, each as its conversation is
    /// named, `<machine>/<session>`.
    pub(super) fn live_sessions(&self, keep: impl Fn(&str) -> bool) -> Vec<String> {
        self.live
            .values()
            .filter(|known| keep(&known.nick))
            .flat_map(|known| {
                known
                    .live
                    .sessions
                    .iter()
                    .chain(known.live.away.keys())
                    .filter(|session| self.session_live(&known.nick, session) == Some(true))
                    .map(|session| super::agent_channel(&known.nick, session))
            })
            .collect()
    }

    /// What an agent's card and header say under its name: `Active now` while it's online
    /// ([`Chat::session_presence`]); away, `Not listening for 17 min · 1 message waiting`
    /// while messages wait, else `Away · last heard 3h ago`; `Ended · last heard 3h ago` once
    /// it has ended; else when it was last heard. With whether it's online, for the here dot.
    /// Where its machine never says whether a watch is open, as zircds before the lists
    /// don't, it's active while `doing` something, as before.
    pub(super) fn heard_line(
        &self,
        nick: &str,
        session: &str,
        doing: bool,
        now: DateTime<Utc>,
    ) -> (String, bool) {
        let last = match self.heard(nick, session) {
            Some(at) if ago_short(at, now) == "now" => Some("last heard just now".to_owned()),
            Some(at) => Some(format!("last heard {} ago", ago_short(at, now))),
            None => None,
        };
        let said = |state: &str| {
            last.clone()
                .map_or_else(|| state.to_owned(), |last| format!("{state} · {last}"))
        };
        match self.listening(nick, session) {
            Some(Listening::Online) => ("Active now".to_owned(), true),
            Some(Listening::Away { .. }) => (
                self.not_listening(nick, session)
                    .unwrap_or_else(|| said(super::AWAY)),
                false,
            ),
            Some(Listening::Ended { .. }) => (said(ENDED), false),
            None if doing => ("Active now".to_owned(), true),
            None => (
                last.map_or_else(
                    || "Not heard yet".to_owned(),
                    |last| {
                        let mut words = last.chars();
                        words
                            .next()
                            .map(|first| first.to_uppercase().chain(words).collect())
                            .unwrap_or_default()
                    },
                ),
                false,
            ),
        }
    }
}

impl MachineLive {
    /// Whether its list names `session`, online or away.
    fn names(&self, session: &str) -> bool {
        self.live.sessions.iter().any(|live| live == session)
            || self.live.away.contains_key(session)
    }
}

/// What an ended session's card and header say it is.
pub const ENDED: &str = "Ended";
