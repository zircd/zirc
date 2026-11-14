//! Which agent sessions are live, as their machines say ([`zirc_proto::session::Live`]):
//! one rule every app draws. A session is live while its machine's latest list names it
//! and the machine is here, or left less than [`LIVE_GRACE_SECS`] ago. A live session is
//! listed from its first watch, before it says anything; one that ended folds away under
//! the list's `N ended`, its conversation kept. A machine whose zircd predates the lists
//! says none, and its sessions fold as they always did, once quiet for a day.

use std::collections::BTreeSet;

use chrono::{DateTime, TimeDelta, Utc};
use zirc_proto::session::{LIVE_GRACE_SECS, Live};

use super::{Chat, SESSION_MARK, fold, profile::ago_short};

/// What a machine last said of its sessions.
#[derive(Debug, Clone)]
pub(super) struct MachineLive {
    /// The machine's nick, as it said it.
    nick: String,
    /// Its live sessions.
    sessions: BTreeSet<String>,
    /// When it said so.
    at: DateTime<Utc>,
    /// Since when it's been gone from the server, or unknown to be here, if it has.
    gone: Option<DateTime<Utc>>,
}

/// How long after its machine left, or since we connected without hearing from it, its
/// sessions still count as live.
fn grace() -> TimeDelta {
    TimeDelta::seconds(i64::try_from(LIVE_GRACE_SECS).unwrap_or(i64::MAX))
}

impl Chat {
    /// What the machine `nick` said at `at` of its live sessions: it replaces what it said
    /// before, unless that was newer.
    pub(super) fn learn_live(&mut self, nick: &str, live: Live, at: DateTime<Utc>) {
        let key = fold(nick);
        if self.live.get(&key).is_some_and(|known| known.at > at) {
            return;
        }
        self.live.insert(
            key,
            MachineLive {
                nick: nick.to_owned(),
                sessions: live.sessions.into_iter().collect(),
                at,
                gone: None,
            },
        );
    }

    /// The machine `nick` left the server at `at`: its sessions end once it's been gone
    /// [`LIVE_GRACE_SECS`], unless it says its list again.
    pub(super) fn live_gone(&mut self, nick: &str, at: DateTime<Utc>) {
        if let Some(known) = self.live.get_mut(&fold(nick)) {
            known.gone.get_or_insert(at);
        }
    }

    /// We connected at `at`: what machines said before is unknown to hold until they say it
    /// again, as each does when we join its channels.
    pub(super) fn live_unsure(&mut self, at: DateTime<Utc>) {
        for known in self.live.values_mut() {
            known.gone.get_or_insert(at);
        }
    }

    /// Whether the agent session `session` on the machine `nick` is live: `Some(true)` while
    /// its machine says so, `Some(false)` once it has ended, and `None` when its machine has
    /// never said, as zircds before the lists don't.
    #[must_use]
    pub fn session_live(&self, nick: &str, session: &str) -> Option<bool> {
        let known = self.live.get(&fold(nick))?;
        let here = known
            .gone
            .is_none_or(|since| (self.clock)() - since < grace());
        Some(here && known.sessions.contains(session))
    }

    /// The nick of the machine that runs the agent session `session`, for a chip naming it
    /// to open its card: the machine that says it's live, else the one that last said it
    /// was, else the machine whose conversation, or whose agent's, `here` is. `None` when
    /// none of those knows it.
    #[must_use]
    pub fn session_machine(&self, session: &str, here: Option<&str>) -> Option<String> {
        self.live
            .values()
            .filter(|known| known.sessions.contains(session))
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

    /// The live agent sessions of the machines `keep` keeps, each as its conversation is
    /// named, `<machine>/<session>`.
    pub(super) fn live_sessions(&self, keep: impl Fn(&str) -> bool) -> Vec<String> {
        self.live
            .values()
            .filter(|known| keep(&known.nick))
            .flat_map(|known| {
                known
                    .sessions
                    .iter()
                    .filter(|session| self.session_live(&known.nick, session) == Some(true))
                    .map(|session| format!("{}{SESSION_MARK}{session}", known.nick))
            })
            .collect()
    }

    /// What an agent's card and header say under its name: `Active now` while it's at work
    /// or live, `Ended · last heard 3h ago` once it has ended, else when it was last heard.
    /// With whether it's active, for the here dot.
    pub(super) fn heard_line(
        &self,
        nick: &str,
        session: &str,
        doing: bool,
        now: DateTime<Utc>,
    ) -> (String, bool) {
        let live = self.session_live(nick, session);
        let last = match self.heard(nick, session) {
            Some(at) if ago_short(at, now) == "now" => Some("last heard just now".to_owned()),
            Some(at) => Some(format!("last heard {} ago", ago_short(at, now))),
            None => None,
        };
        match live {
            Some(false) => (
                last.map_or_else(|| ENDED.to_owned(), |last| format!("{ENDED} · {last}")),
                false,
            ),
            Some(true) => ("Active now".to_owned(), true),
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

/// What an ended session's card and header say it is.
pub const ENDED: &str = "Ended";
