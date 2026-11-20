//! Which of others' intents each session has been woken for, so an intent wakes a session
//! only when it says something new to it: a path of the session's it hadn't touched, or one
//! whose last intent has run out. zircd renews its automatic intents every 20 minutes, and
//! sessions say intents again as their plans change; without this, each of those woke every
//! session they touched again, with nothing new to say.

use std::collections::{BTreeMap, BTreeSet};

use zirc_core::ipc::Response;
use zirc_proto::Event;

use super::{Daemon, auto_intents, now};

/// By whom it's for and from (see [`Daemon::stale_intent`]): each path told, and until when.
#[derive(Debug, Default)]
pub(super) struct IntentsTold {
    told: BTreeMap<String, BTreeMap<String, u64>>,
}

impl IntentsTold {
    /// Whether an intent touching `shared`, of the paths of whoever `key` names, is news to
    /// them at `now`, and notes it told until `until`. One that `ends` (`done:` or
    /// `dropped:`) is news when it ends what they were told, which it forgets.
    pub(super) fn news(
        &mut self,
        key: &str,
        shared: &[String],
        ends: bool,
        now: u64,
        until: u64,
    ) -> bool {
        self.told.retain(|_, paths| {
            paths.retain(|_, until| *until > now);
            !paths.is_empty()
        });
        let told = self.told.entry(key.to_owned()).or_default();
        let news = if ends {
            // Each told path is forgotten, not just the first.
            shared
                .iter()
                .filter(|path| told.remove(*path).is_some())
                .count()
                > 0
        } else {
            let news = shared.iter().any(|path| !told.contains_key(path));
            for path in shared {
                let at = told.entry(path.clone()).or_default();
                *at = (*at).max(until);
            }
            news
        };
        if told.is_empty() {
            self.told.remove(key);
        }
        news
    }
}

impl Daemon {
    /// The sessions on this machine to whom `response`, if it's someone's intent, says
    /// nothing new, by [`IntentsTold::news`]; it's noted as told to the rest it touches.
    pub(super) fn stale_intent(&mut self, response: &Response) -> BTreeSet<String> {
        let mut stale = BTreeSet::new();
        let Response::Delivery(delivery) = response else {
            return stale;
        };
        let Event::Intent {
            summary,
            paths,
            expires,
            lasts,
        } = &delivery.message.event
        else {
            return stale;
        };
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        let sessions: BTreeSet<String> = self
            .watchers
            .iter()
            .filter_map(|watcher| watcher.session.clone())
            .chain(self.missed.keys().cloned())
            .collect();
        let now = now();
        let until = lasts
            .map(|lasts| now + lasts)
            .or(*expires)
            .unwrap_or(now + auto_intents::LASTS);
        let ends = zirc_proto::says_finished(summary);
        // An agent room's intents are its repository's, where each session's paths and its
        // own intents are kept, as [`Daemon::why`] reads them: looked for in the room itself,
        // a session's intent shared nothing, so it was never noted as told, and each time
        // zircd said another's again for the same directory, it woke the session anew.
        let home = delivery.scope();
        let channel = home.as_str();
        for session in sessions {
            let mine = self.session_paths(channel, &session);
            let mine: BTreeSet<&str> = mine.iter().map(String::as_str).collect();
            let shared = self.shared_with_mine(channel, &me, Some(&session), &mine, paths);
            if shared.is_empty() {
                continue;
            }
            let key = format!(
                "{session}\n{channel}\n{}/{}",
                delivery.from, delivery.message.session
            );
            if !self.intents_told.news(&key, &shared, ends, now, until) {
                stale.insert(session);
            }
        }
        stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|&path| path.to_owned()).collect()
    }

    #[test]
    fn an_intent_is_news_once_until_it_runs_out_or_touches_more() {
        let mut told = IntentsTold::default();
        let src = paths(&["src"]);
        assert!(told.news("cc-1/cc-2", &src, false, 1000, 2800));
        // Renewed, or said again, while the first lasts: nothing new.
        assert!(!told.news("cc-1/cc-2", &src, false, 2200, 4000));
        // The renewal kept it going past the first's end.
        assert!(!told.news("cc-1/cc-2", &src, false, 3000, 4800));
        // It touches another of the session's paths: news.
        assert!(told.news("cc-1/cc-2", &paths(&["src", "tests"]), false, 3100, 4900));
        // The same from another session, or to another, is news to them.
        assert!(told.news("cc-1/cc-3", &src, false, 3100, 4900));
        assert!(told.news("cc-4/cc-2", &src, false, 3100, 4900));
        // Once it's run out, it's news again.
        assert!(told.news("cc-1/cc-2", &src, false, 4900, 6700));
    }

    #[test]
    fn a_done_is_news_only_when_it_ends_what_was_told() {
        let mut told = IntentsTold::default();
        let src = paths(&["src"]);
        assert!(!told.news("cc-1/cc-2", &src, true, 1000, 2800));
        assert!(told.news("cc-1/cc-2", &src, false, 1000, 2800));
        assert!(told.news("cc-1/cc-2", &src, true, 1100, 2900));
        assert!(!told.news("cc-1/cc-2", &src, true, 1200, 3000));
        assert!(told.news("cc-1/cc-2", &src, false, 1300, 3100));
    }
}
