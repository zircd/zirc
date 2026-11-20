//! Which of this machine's agent sessions are live ([`zirc_proto::session::Live`]): those
//! with a `zirc watch` open, or one closed less than [`LIVE_GRACE_SECS`] ago, as a session
//! watching with `--once` closes its watch each time it wakes and opens it again. Said in
//! the repositories' channels and the organisation's whenever the list changes, and again
//! to whoever joins, so apps show a session from its first watch and fold it away once it
//! has ended. When each session last watched is kept across restarts, so a zircd that
//! restarts doesn't say its sessions ended while their watches come back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use zirc_core::paths;
use zirc_core::project::ORG_CHANNEL;
use zirc_proto::session::{self, LIVE_GRACE_SECS};
use zirc_proto::{AGENTS_CHANNEL, Level, Message};

use super::{Daemon, WATCHER_SESSION, read_saved};

/// Every machine's live sessions, as each last said its list in the organisation's channels,
/// and every session seen live since zircd started: whose watch is gone ([`Request::LiveSessions`]).
///
/// [`Request::LiveSessions`]: zirc_core::ipc::Request::LiveSessions
#[derive(Debug, Default)]
pub(super) struct Elsewhere {
    /// Machine's nick → its live sessions.
    lists: BTreeMap<String, BTreeSet<String>>,
    /// Every session seen live, here or on another machine.
    ever: BTreeSet<String>,
}

impl Elsewhere {
    /// `machine` said its live sessions are `sessions`.
    pub(super) fn said(&mut self, machine: &str, sessions: &[String]) {
        self.ever.extend(sessions.iter().cloned());
        self.lists
            .insert(machine.to_owned(), sessions.iter().cloned().collect());
    }

    /// Those live now, with `here`, this machine's, and those seen that no longer are.
    pub(super) fn now(&mut self, here: &[String]) -> (Vec<String>, Vec<String>) {
        self.ever.extend(here.iter().cloned());
        let live: BTreeSet<String> = self.lists.values().flatten().chain(here).cloned().collect();
        let gone = self.ever.difference(&live).cloned().collect();
        (live.into_iter().collect(), gone)
    }
}

/// How often, at most, the times are saved while the list stays the same, in seconds.
const SAVE_EVERY: u64 = 60;

/// Which sessions are live, and what was said where.
#[derive(Debug, Default)]
pub(super) struct Live {
    /// Where the times are kept.
    path: PathBuf,
    /// Session → when it last had a watch open, in seconds since the epoch.
    seen: BTreeMap<String, u64>,
    /// Channel → the list last said there, since joining it.
    said: BTreeMap<String, session::Live>,
    /// When the times were last saved.
    saved_at: u64,
}

impl Live {
    /// What was kept in the organisation's state directory `state`, or nothing.
    pub(super) fn read(state: &Path) -> Self {
        let path = paths::live_sessions(state);
        let seen = read_saved(&path).unwrap_or_default();
        Self {
            path,
            seen,
            ..Self::default()
        }
    }

    /// Notes that `watching` have a watch open at `now`, and forgets those whose last
    /// closed [`LIVE_GRACE_SECS`] ago or more. Returns the live sessions, sorted, and
    /// whether that list changed.
    pub(super) fn update(
        &mut self,
        watching: impl IntoIterator<Item = String>,
        now: u64,
    ) -> (Vec<String>, bool) {
        let before: Vec<String> = self.seen.keys().cloned().collect();
        for session in watching {
            self.seen.insert(session, now);
        }
        self.seen
            .retain(|_, at| now.saturating_sub(*at) < LIVE_GRACE_SECS);
        let live: Vec<String> = self.seen.keys().cloned().collect();
        let changed = live != before;
        (live, changed)
    }

    /// The sessions live here, sorted.
    pub(super) fn sessions(&self) -> Vec<String> {
        self.seen.keys().cloned().collect()
    }

    /// Someone joined `channel`: they hear the list again.
    pub(super) fn say_again(&mut self, channel: &str) {
        self.said.remove(channel);
    }

    /// Saves when each session last watched.
    fn save(&mut self, now: u64) {
        let json = serde_json::to_string(&self.seen).expect("a map of times serializes");
        if let Err(err) = paths::write_private(&self.path, &json) {
            tracing::warn!("couldn't save which sessions are live: {err}");
        }
        self.saved_at = now;
    }
}

impl Daemon {
    /// Notes which sessions watch now, and says the live list where it hasn't been said
    /// as it is. Called every second.
    pub(super) fn live_tick(&mut self, now: u64) {
        let watching: Vec<String> = self
            .watching()
            .into_iter()
            .flatten()
            .filter(|session| !session.is_empty() && session != WATCHER_SESSION)
            .collect();
        let (live, changed) = self.live.update(watching, now);
        if changed || now.saturating_sub(self.live.saved_at) >= SAVE_EVERY {
            self.live.save(now);
        }
        // A guest's terminal says nothing outside its guest's room.
        if self.org.guest.is_some() {
            return;
        }
        let Some(client) = &self.client else {
            self.live.said.clear();
            return;
        };
        // Said in the organisation's channels a machine's in: never a repository's scope, which
        // is no channel ([`Daemon::is_scope`]).
        let channels: Vec<String> = self
            .joined
            .iter()
            .filter(|channel| [ORG_CHANNEL, AGENTS_CHANNEL].contains(&channel.as_str()))
            .cloned()
            .collect();
        self.live
            .said
            .retain(|channel, _| channels.contains(channel));
        // Each live session's roles ride along, for the workflows that give out their steps.
        let roles: BTreeMap<String, Vec<String>> = live
            .iter()
            .map(|session| {
                (
                    session.clone(),
                    self.roles_of(session).into_iter().collect(),
                )
            })
            .filter(|(_, roles): &(String, Vec<String>)| !roles.is_empty())
            .collect();
        let list = session::Live {
            sessions: live,
            roles,
        };
        for channel in channels {
            if self.live.said.get(&channel) == Some(&list) {
                continue;
            }
            // Debug, said outright, so apps from before the kind was known take it as
            // only true now too, and show no line for it.
            let message = Message {
                level: Some(Level::Debug),
                ..self.signed(WATCHER_SESSION, list.event())
            };
            match zirc_core::send::send_message(client, &channel, &message) {
                Ok(()) => {
                    self.live.said.insert(channel, list.clone());
                }
                Err(err) => tracing::warn!("couldn't say the live sessions in {channel}: {err}"),
            }
        }
    }

    /// Saves when each session last watched, as zircd stops.
    pub(super) fn save_live(&mut self, now: u64) {
        let watching: Vec<String> = self.watching().into_iter().flatten().collect();
        self.live.update(watching, now);
        self.live.save(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_seen_live_elsewhere_is_gone_once_its_machine_leaves_it_out() {
        let mut elsewhere = Elsewhere::default();
        elsewhere.said("ada-mbp", &["cc-1".into(), "cc-2".into()]);
        assert_eq!(
            elsewhere.now(&["cc-3".into()]),
            (vec!["cc-1".into(), "cc-2".into(), "cc-3".into()], vec![])
        );
        elsewhere.said("ada-mbp", &["cc-2".into()]);
        assert_eq!(
            elsewhere.now(&[]),
            (vec!["cc-2".into()], vec!["cc-1".into(), "cc-3".into()])
        );
    }

    #[test]
    fn a_session_is_live_from_its_first_watch_until_the_grace_after_its_last() {
        let mut live = Live::default();
        assert_eq!(
            live.update(["cc-1".to_owned()], 100),
            (vec!["cc-1".to_owned()], true)
        );
        // Its --once watch woke and is being started again: still live.
        assert_eq!(
            live.update([], 100 + LIVE_GRACE_SECS - 1),
            (vec!["cc-1".to_owned()], false)
        );
        assert_eq!(live.update([], 100 + LIVE_GRACE_SECS), (vec![], true));
    }

    #[test]
    fn a_restart_keeps_sessions_live_while_their_watches_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut live = Live::read(dir.path());
        live.update(["cc-1".to_owned()], 1_000);
        live.save(1_000);
        let mut again = Live::read(dir.path());
        assert_eq!(again.update([], 1_010), (vec!["cc-1".to_owned()], false));
        assert_eq!(again.update([], 1_000 + LIVE_GRACE_SECS), (vec![], true));
    }
}
