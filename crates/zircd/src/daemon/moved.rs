//! What a session hears of the default branch at its next wake, and never wakes for: that
//! landings have gone onto it since the session's work was built, and that one of the
//! repository's states is broken, or fine again.
//!
//! A landing changes files a session depends on far more often than its own: the session's
//! tests then run against old code, and it finds out when it lands. Whether that matters is
//! the session's to judge, when it's doing something anyway, so each landing is counted for
//! every session working in the repository and said as one note with the next line that
//! wakes it ([`super::later`]), until its work is built on something newer. A state said
//! broken names whoever is to fix it, whom it wakes; everyone else working there hears it
//! the same way, and that it's fine again only if they heard it broken.

use std::collections::{BTreeMap, BTreeSet};

use zirc_core::ipc::{Delivery, Response, Why};
use zirc_core::sessions::LANDING_KIND;
use zirc_proto::state::{Level, State};
use zirc_proto::{Event, Message};

use super::{Daemon, WATCHER_SESSION, in_repo};

/// How many landings are counted for a session at most.
const COUNTED: usize = 500;

/// What's kept to say these notes.
#[derive(Debug, Default)]
pub(super) struct Moved {
    /// The commits landed since each session's work was built, oldest first, by session.
    landed: BTreeMap<String, Vec<String>>,
    /// The states a session has a note of as broken, or heard so: session, repository's
    /// channel and state's name.
    broken: BTreeSet<(String, String, String)>,
}

/// A landing CI says landed: its commit, the session that queued it and what it's about.
struct Landed {
    commit: String,
    session: Option<String>,
    subject: Option<String>,
}

impl Landed {
    /// The landing `response` says landed, if it's CI saying one did.
    fn of(response: &Response) -> Option<(&str, Self)> {
        let Response::Delivery(delivery) = response else {
            return None;
        };
        let Event::Custom { kind, data, text } = &delivery.message.event else {
            return None;
        };
        let ci = zirc_proto::ci::Data::of(data);
        if kind != LANDING_KIND
            || ci.state.as_deref() != Some("passed")
            || !text.contains(" landed ")
        {
            return None;
        }
        let landed = Self {
            commit: ci.commit?,
            session: ci.session,
            subject: ci.subject,
        };
        Some((&delivery.channel, landed))
    }
}

/// A commit as people say it: its first seven characters.
fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// What a session is told of `count` landings on `branch` since its work was built on
/// `base`, the newest of them `commit`, about `subject`.
fn moved_line(
    session: &str,
    branch: &str,
    count: usize,
    base: Option<&str>,
    commit: &str,
    subject: Option<&str>,
) -> String {
    let landings = if count == 1 { "landing" } else { "landings" };
    let since = base.map_or_else(String::new, |base| {
        format!(" since {}, which your work is built on", short(base))
    });
    let about = subject.map_or_else(String::new, |subject| format!(" \"{subject}\""));
    format!(
        "{session}: {branch} moved: {count} {landings}{since}; the newest is {}{about}",
        short(commit)
    )
}

/// Whether `response` is a note that the default branch moved.
fn is_moved(response: &Response) -> bool {
    matches!(response, Response::Delivery(delivery) if delivery.why == Some(Why::DefaultMoved))
}

/// Whether `response` is `state` said broken.
fn says_broken(response: &Response, state: &State) -> bool {
    matches!(response, Response::Delivery(delivery)
    if State::of(&delivery.message.event).is_some_and(|said| {
        said.repo == state.repo && said.name == state.name && said.level == Level::Broken
    }))
}

impl Daemon {
    /// The sessions working in the repository `channel` is of that have watched, each with
    /// its default branch and the commit its work is built on.
    fn working_in(&self, channel: &str) -> Vec<(String, String, Option<String>)> {
        let known: BTreeSet<&str> = self
            .watchers
            .iter()
            .filter_map(|watcher| watcher.session.as_deref())
            .chain(self.missed.keys().map(String::as_str))
            .collect();
        self.repos
            .values()
            .filter(|watched| in_repo(channel, &watched.channel))
            .flat_map(|watched| {
                watched.sessions.names().map(|session| {
                    (
                        session.to_owned(),
                        watched.default_branch.clone(),
                        watched.snapshot.base.clone(),
                    )
                })
            })
            .filter(|(session, _, _)| known.contains(session.as_str()))
            .collect()
    }

    /// Keeps what `response` says of the default branch for the sessions it doesn't wake
    /// (`woken` are those it does), for their next wake: see the module's doc.
    pub(super) fn note_quietly(&mut self, response: &Response, woken: &BTreeSet<String>) {
        if let Some((channel, landed)) = Landed::of(response) {
            let channel = channel.to_owned();
            self.note_landed(&channel, &landed, woken);
        } else if let Response::Delivery(delivery) = response
            && let Some(state) = State::of(&delivery.message.event)
        {
            self.note_state(response, &state, woken);
        }
    }

    fn note_landed(&mut self, channel: &str, landed: &Landed, woken: &BTreeSet<String>) {
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        for (session, branch, base) in self.working_in(channel) {
            // Its own landing, or one its work has already.
            if landed.session.as_deref() == Some(session.as_str())
                || base.as_deref() == Some(landed.commit.as_str())
            {
                continue;
            }
            let counted = self.moved.landed.entry(session.clone()).or_default();
            if counted.contains(&landed.commit) {
                continue;
            }
            counted.push(landed.commit.clone());
            if counted.len() > COUNTED {
                counted.remove(0);
            }
            let count = counted.len();
            // The landing's own line, as one that changes the session's files, says it now.
            if woken.contains(&session) {
                continue;
            }
            let body = moved_line(
                &session,
                &branch,
                count,
                base.as_deref(),
                &landed.commit,
                landed.subject.as_deref(),
            );
            let note = Response::Delivery(Delivery::new(
                channel.to_owned(),
                me.clone(),
                Message::new(WATCHER_SESSION, Event::Text { body }),
            ));
            // One note, of all of them: the one before it is no longer kept.
            if let Some(kept) = self.later.get_mut(&session) {
                kept.retain(|response| !is_moved(response));
            }
            self.keep_for_later(&session, &note, Why::DefaultMoved);
        }
    }

    fn note_state(&mut self, response: &Response, state: &State, woken: &BTreeSet<String>) {
        for (session, _, _) in self.working_in(&state.repo) {
            let key = (session.clone(), state.repo.clone(), state.name.clone());
            match state.level {
                Level::Broken => {
                    if self.moved.broken.insert(key) && !woken.contains(&session) {
                        self.keep_for_later(&session, response, Why::StateChanged);
                    }
                }
                Level::Ok => {
                    if !self.moved.broken.remove(&key) {
                        continue;
                    }
                    // Broken and fine again before the session heard either: neither is said.
                    let kept = self.later.entry(session.clone()).or_default();
                    let before = kept.len();
                    kept.retain(|response| !says_broken(response, state));
                    let unheard = kept.len() < before;
                    if kept.is_empty() {
                        self.later.remove(&session);
                    }
                    if !unheard && !woken.contains(&session) {
                        self.keep_for_later(&session, response, Why::StateChanged);
                    }
                }
                Level::Warning => {}
            }
        }
    }

    /// Notes that the work of `sessions` is built on another commit now, as after a rebase:
    /// the landings counted for them are behind it, and a note of them not yet heard is
    /// dropped.
    pub(super) fn rebuilt(&mut self, sessions: &[String]) {
        for session in sessions {
            self.moved.landed.remove(session);
            if let Some(kept) = self.later.get_mut(session) {
                kept.retain(|response| !is_moved(response));
                if kept.is_empty() {
                    self.later.remove(session);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_many_landings_since_what_the_work_is_built_on() {
        let base = "b".repeat(40);
        let commit = "1a2b3c4d5e".repeat(4);
        assert_eq!(
            moved_line("cc-1", "main", 1, Some(&base), &commit, Some("Fix login")),
            "cc-1: main moved: 1 landing since bbbbbbb, which your work is built on; the newest is 1a2b3c4 \"Fix login\""
        );
        assert_eq!(
            moved_line("cc-1", "main", 3, None, &commit, None),
            "cc-1: main moved: 3 landings; the newest is 1a2b3c4"
        );
    }

    fn landing(state: &str, text: &str) -> Response {
        Response::Delivery(Delivery::new(
            "#api/landings",
            "zirc-ci",
            Message::new(
                "ci",
                Event::Custom {
                    kind: LANDING_KIND.into(),
                    data: serde_json::json!({
                        "state": state, "session": "cc-2", "commit": "c".repeat(40),
                        "subject": "Fix login",
                    }),
                    text: text.into(),
                },
            ),
        ))
    }

    #[test]
    fn only_a_landing_that_landed_counts() {
        let said = landing("passed", "cc-2: landed login as ccccccc (build 7)");
        let (channel, landed) = Landed::of(&said).unwrap();
        assert_eq!(channel, "#api/landings");
        assert_eq!(landed.commit, "c".repeat(40));
        assert_eq!(landed.session.as_deref(), Some("cc-2"));
        assert_eq!(landed.subject.as_deref(), Some("Fix login"));
        // Queued, failed, or the Apple apps' word on it: not a landing landed.
        for (state, text) in [
            ("pending", "cc-2: queued login (build 7)"),
            ("failed", "cc-2: build 7 failed"),
            (
                "passed",
                "cc-2 (apple/login): build 7's Apple apps compile on ccccccc",
            ),
        ] {
            assert!(Landed::of(&landing(state, text)).is_none(), "{text}");
        }
    }
}
