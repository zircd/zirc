//! Which of this machine's agent sessions are online and which away
//! ([`zirc_proto::session::Live`]): online while a `zirc watch` of its is open, away
//! otherwise, with since when and how many people's messages wait for it, for
//! [`AWAY_FOLD_SECS`] after its last watch. Said in the organisation's channels whenever it
//! changes, and again to whoever joins, so apps show a session from its first watch, say
//! when it isn't listening, and fold it away 12 hours after. [`Live::sessions`] keeps its
//! grace ([`LIVE_GRACE_SECS`]) for apps from before `away`. When each
//! session last watched is kept across restarts, so a zircd that restarts says how long
//! its sessions have been away. A session away [`NOT_LISTENING_TELL_SECS`] with a message
//! waiting is told to the machine's owner, once for each time it's away, as from the
//! session by what it's called.
//!
//! [`Live::sessions`]: zirc_proto::session::Live::sessions

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use zirc_core::chat::{SenderKind, sender_kind};
use zirc_core::ipc::{Response, Why};
use zirc_core::paths;
use zirc_core::project::ORG_CHANNEL;
use zirc_proto::session::{self, AWAY_FOLD_SECS, Away, LIVE_GRACE_SECS, NOT_LISTENING_TELL_SECS};
use zirc_proto::{AGENTS_CHANNEL, Level, Message};

use super::{Daemon, WATCHER_SESSION, read_saved};
use crate::activity;

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
    /// The list last told the tenant's recorder, and when: told again when it changes, and
    /// every [`zirc_proto::working::TELL_EVERY_SECS`], as a recorder that started since
    /// heard nothing.
    recorded: Option<(session::Live, u64)>,
    /// When the times were last saved.
    saved_at: u64,
    /// The sessions whose owner was told they aren't listening, each with since when it's
    /// been away then: told once for each time it's away.
    told: BTreeMap<String, u64>,
    /// The live sessions last said to have joined, in zircd's own lines ([`crate::activity`]).
    joined: BTreeSet<String>,
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
    /// closed [`AWAY_FOLD_SECS`] ago or more. Returns whether the sessions it knows changed.
    pub(super) fn update(&mut self, watching: &BTreeSet<String>, now: u64) -> bool {
        let before: Vec<String> = self.seen.keys().cloned().collect();
        for session in watching {
            self.seen.insert(session.clone(), now);
        }
        self.seen
            .retain(|_, at| now.saturating_sub(*at) < AWAY_FOLD_SECS);
        self.told
            .retain(|session, _| self.seen.contains_key(session) && !watching.contains(session));
        !self.seen.keys().eq(before.iter())
    }

    /// Whether `session` had a watch open in the last [`AWAY_FOLD_SECS`]: online, or away
    /// and still listed.
    pub(super) fn knows(&self, session: &str) -> bool {
        self.seen.contains_key(session)
    }

    /// The sessions live here, as [`session::Live::sessions`] says them: watching, or
    /// last watched less than [`LIVE_GRACE_SECS`] before `now`, sorted.
    pub(super) fn sessions(&self, now: u64) -> Vec<String> {
        self.seen
            .iter()
            .filter(|(_, at)| now.saturating_sub(**at) < LIVE_GRACE_SECS)
            .map(|(session, _)| session.clone())
            .collect()
    }

    /// The sessions here with no watch among `watching`, each with when it last watched and
    /// how many messages wait for it, as `waiting` counts them.
    pub(super) fn away(
        &self,
        watching: &BTreeSet<String>,
        waiting: impl Fn(&str) -> u32,
    ) -> BTreeMap<String, Away> {
        self.seen
            .iter()
            .filter(|(session, _)| !watching.contains(*session))
            .map(|(session, since)| {
                let away = Away {
                    since: *since,
                    waiting: waiting(session),
                };
                (session.clone(), away)
            })
            .collect()
    }

    /// Of `away`, the sessions to tell the owner about at `now`: away
    /// [`NOT_LISTENING_TELL_SECS`] or longer with a message waiting, and not told yet this
    /// time they're away. Noted as told.
    pub(super) fn due_to_tell(
        &mut self,
        away: &BTreeMap<String, Away>,
        now: u64,
    ) -> Vec<(String, Away)> {
        let due: Vec<(String, Away)> = away
            .iter()
            .filter(|(session, away)| {
                away.waiting > 0
                    && now.saturating_sub(away.since) >= NOT_LISTENING_TELL_SECS
                    && self.told.get(*session) != Some(&away.since)
            })
            .map(|(session, away)| (session.clone(), *away))
            .collect();
        for (session, away) in &due {
            self.told.insert(session.clone(), away.since);
        }
        due
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
    /// as it is; tells the owner of a session that has stopped listening with messages
    /// waiting. Called every second.
    pub(super) fn live_tick(&mut self, now: u64) {
        let watching: BTreeSet<String> = self
            .watching()
            .into_iter()
            .flatten()
            .filter(|session| !session.is_empty() && session != WATCHER_SESSION)
            .collect();
        let changed = self.live.update(&watching, now);
        if changed || now.saturating_sub(self.live.saved_at) >= SAVE_EVERY {
            self.live.save(now);
        }
        let live = self.live.sessions(now);
        self.say_joined(&live);
        let away = self.live.away(&watching, |session| self.waiting(session));
        for (session, away) in self.live.due_to_tell(&away, now) {
            self.tell_not_listening(&session, now.saturating_sub(away.since), away.waiting);
        }
        let Some(client) = &self.client else {
            self.live.said.clear();
            self.live.recorded = None;
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
        let list = session::Live {
            sessions: live,
            away,
        };
        // The tenant's recorder hears it too, for zirc.dev's dashboard, which is in no channel
        // ([`zirc_proto::working`]): best effort, as every record to it is.
        let told = self.live.recorded.as_ref().is_some_and(|(said, at)| {
            *said == list && now.saturating_sub(*at) < zirc_proto::working::TELL_EVERY_SECS
        });
        if !told && self.record(WATCHER_SESSION, list.event()) {
            self.live.recorded = Some((list.clone(), now));
            // With each session's card, so a recorder that started since knows what it's called.
            let named: BTreeSet<&String> = list.sessions.iter().chain(list.away.keys()).collect();
            for session in named {
                if let Some(card) = self.card_of(session) {
                    self.record(session, card.event());
                }
            }
        }
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

    /// Says which sessions joined since the last tick and which left, as `live` has them.
    fn say_joined(&mut self, live: &[String]) {
        let live: BTreeSet<String> = live.iter().cloned().collect();
        if live == self.live.joined {
            return;
        }
        let joined: Vec<String> = live.difference(&self.live.joined).cloned().collect();
        let left: Vec<String> = self.live.joined.difference(&live).cloned().collect();
        for session in joined {
            let who = activity::who(&session, self.name_of(&session).as_deref());
            self.activity.say(&activity::joined(&who));
        }
        for session in left {
            let who = activity::who(&session, self.name_of(&session).as_deref());
            self.activity.say(&activity::left(&who));
        }
        self.live.joined = live;
    }

    /// Saves when each session last watched, as zircd stops.
    pub(super) fn save_live(&mut self, now: u64) {
        let watching: BTreeSet<String> = self.watching().into_iter().flatten().collect();
        self.live.update(&watching, now);
        self.live.save(now);
    }

    /// Tells the machine's owner that `session` has been away `away_secs` with `waiting`
    /// messages for it, as from the session itself, by what it's called: their apps file it
    /// in its conversation, which a tap on the push opens.
    fn tell_not_listening(&mut self, session: &str, away_secs: u64, waiting: u32) {
        let name = self.name_of(session);
        let task = self.tasks.on(session).cloned();
        let card = self.book.title(session);
        let title =
            session::agent_title(card.as_deref(), task.as_deref(), name.as_deref(), session);
        let words = session::not_listening_notice(&title, away_secs, waiting);
        tracing::info!("{session}: {words}");
        if let Err(err) = self.dm(session, &words, None) {
            tracing::warn!("couldn't tell the owner {session} isn't listening: {err:#}");
        }
    }

    /// How many messages from people wait for `session`, kept while no watch of its is
    /// open ([`Daemon::keep_missed`]): what a person wrote to it, or about it by name.
    /// Another agent's note, query or answer, and what systems say, aren't counted.
    fn waiting(&self, session: &str) -> u32 {
        let count = self.missed.get(session).map_or(0, |kept| {
            kept.iter()
                .filter(|(_, response)| from_a_person(response))
                .count()
        });
        u32::try_from(count).unwrap_or(u32::MAX)
    }
}

/// Whether `response`, kept for a session away, is a person's message to it: a direct
/// message (the owner's, or a colleague's reply), or a person's line naming it.
fn from_a_person(response: &Response) -> bool {
    match response {
        Response::Direct(direct) => {
            !direct.note && direct.query.is_none() && direct.answer.is_none()
        }
        Response::Delivery(delivery) => {
            delivery.why == Some(Why::Mention)
                && delivery.message.session.is_empty()
                && sender_kind(&delivery.from, delivery.hosted_by.as_deref()) == SenderKind::Person
        }
        _ => false,
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

    fn watching(sessions: &[&str]) -> BTreeSet<String> {
        sessions.iter().map(|&session| session.to_owned()).collect()
    }

    #[test]
    fn a_session_is_away_once_its_watch_closes_and_live_for_older_apps_a_grace_longer() {
        let mut live = Live::default();
        assert!(live.update(&watching(&["cc-1"]), 100));
        assert_eq!(live.sessions(100), vec!["cc-1".to_owned()]);
        assert!(live.away(&watching(&["cc-1"]), |_| 0).is_empty());
        // Its --once watch woke and is being started again: away at once, since its last
        // watch, though older apps still see it live.
        assert!(!live.update(&watching(&[]), 105));
        assert_eq!(
            live.away(&watching(&[]), |_| 2),
            BTreeMap::from([(
                "cc-1".to_owned(),
                Away {
                    since: 100,
                    waiting: 2
                }
            )])
        );
        assert_eq!(live.sessions(105), vec!["cc-1".to_owned()]);
        assert!(live.sessions(100 + LIVE_GRACE_SECS).is_empty());
        // Away until 12 hours after its last watch: then no longer said.
        assert!(!live.update(&watching(&[]), 100 + AWAY_FOLD_SECS - 1));
        assert_eq!(live.away(&watching(&[]), |_| 0).len(), 1);
        assert!(live.update(&watching(&[]), 100 + AWAY_FOLD_SECS));
        assert!(live.away(&watching(&[]), |_| 0).is_empty());
    }

    #[test]
    fn the_owner_hears_once_each_time_a_session_stops_listening_with_messages_waiting() {
        let mut live = Live::default();
        live.update(&watching(&["cc-1", "cc-2"]), 100);
        live.update(&watching(&[]), 101);
        let (none, one, three) = (
            live_away(&live, 0),
            live_away(&live, 1),
            live_away(&live, 3),
        );
        let late = 100 + NOT_LISTENING_TELL_SECS;
        // Nothing waiting: nothing to tell.
        assert!(live.due_to_tell(&none, late).is_empty());
        // Too soon.
        assert!(live.due_to_tell(&one, late - 1).is_empty());
        let told = live.due_to_tell(&one, late);
        assert_eq!(told.len(), 2);
        assert!(live.due_to_tell(&three, late + 60).is_empty());
        // Back, then away again: told again once it's been long enough.
        live.update(&watching(&["cc-1"]), late + 100);
        live.update(&watching(&[]), late + 101);
        let again = late + 100 + NOT_LISTENING_TELL_SECS;
        let one = live_away(&live, 1);
        let told = live.due_to_tell(&one, again);
        assert_eq!(
            told.iter()
                .map(|(session, _)| session.as_str())
                .collect::<Vec<_>>(),
            vec!["cc-1"]
        );
    }

    fn live_away(live: &Live, waiting: u32) -> BTreeMap<String, Away> {
        live.away(&watching(&[]), |_| waiting)
    }

    #[test]
    fn a_restart_keeps_when_each_session_last_watched() {
        let dir = tempfile::tempdir().unwrap();
        let mut live = Live::read(dir.path());
        live.update(&watching(&["cc-1"]), 1_000);
        live.save(1_000);
        let mut again = Live::read(dir.path());
        assert!(!again.update(&watching(&[]), 1_010));
        assert_eq!(again.sessions(1_010), vec!["cc-1".to_owned()]);
        assert_eq!(again.away(&watching(&[]), |_| 0)["cc-1"].since, 1_000);
    }
}
