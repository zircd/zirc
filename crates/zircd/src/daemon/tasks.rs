//! The task each session is on, so its posts, intents, commits, pushes and landing say which
//! task they're part of ([`zirc_proto::Message::task`]): a session's `taking:` post starts
//! one, with a new id, and its `done:` or `dropped:` post, the last to carry it, ends it. A
//! session's intent, while it's on none, starts one too, which its landing ends. A landing
//! (`//ops:land`, [`zirc_core::ipc::Request::Task`]) claims the task the session is on, else
//! the one it finished last, as sessions often post `done:` before they land, else a new
//! one of its own, so every landing says a task: 101 of 234 landings' events said none on
//! 14-15 Nov, the fixer's (which takes workflow steps, never `taking:`) and those landed
//! well after a `done:`, and a landing no task names can't be counted per task. When that
//! landing lands while the session is still on the task it claimed, zircd says `done:` for
//! it, in the channel its `taking:` went to: the task is finished everywhere without the
//! session waking to say so, and `zirc intend` posts the `taking:` for a session on none, so
//! a session needn't post either. Kept across restarts, as names are, the `done:` to say
//! too, as zircd updates itself while landings run. Only ids and channels: never what a
//! task is.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use zirc_core::{ipc::LANDING_CLAIMS, paths};
use zirc_proto::Event;

use super::book::Field;
use super::{Daemon, read_saved, save_saved};

/// A task a session finished, for its landing to claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Finished {
    /// The task's id.
    task: String,
    /// When it was finished, in seconds since the Unix epoch.
    at: u64,
}

/// What zircd needs to say `done:` for a session whose landing lands.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Landing {
    /// Session → the channel its `taking:` went to.
    #[serde(default)]
    taken_in: BTreeMap<String, String>,
    /// Session → the task its queued landing claimed while on it, which that landing
    /// landing ends.
    #[serde(default)]
    claimed: BTreeMap<String, String>,
}

/// Each session's task: the one it's on, whether an intent started it, the one it finished
/// last, and what its landing landing finishes. An organisation's daemon keeps its own, in its own directory: a session's
/// task is said where it works.
#[derive(Debug, Default)]
pub(super) struct Tasks {
    /// Session → the task it's on, on what it sends.
    on: BTreeMap<String, String>,
    /// The sessions whose task an intent started, not a `taking:`: their landing ends it.
    /// Not kept across restarts, where such a task runs on to `done:` instead.
    implied: BTreeSet<String>,
    /// Session → the task it finished last, for its landing to claim.
    finished: BTreeMap<String, Finished>,
    /// For zircd's `done:` when a session's landing lands.
    landing: Landing,
}

impl Tasks {
    /// The tasks saved in `state`, an organisation's directory.
    pub(super) fn load(state: &Path) -> Self {
        Self {
            on: read_saved(&paths::session_tasks(state)).unwrap_or_default(),
            implied: BTreeSet::new(),
            finished: read_saved(&paths::session_tasks_finished(state)).unwrap_or_default(),
            landing: read_saved(&paths::session_tasks_landing(state)).unwrap_or_default(),
        }
    }

    fn save(&self, state: &Path) -> std::io::Result<()> {
        save_saved(&paths::session_tasks(state), &self.on)?;
        save_saved(&paths::session_tasks_finished(state), &self.finished)?;
        save_saved(&paths::session_tasks_landing(state), &self.landing)
    }

    /// The task `session` is on.
    pub(super) fn on(&self, session: &str) -> Option<&String> {
        self.on.get(session)
    }

    /// Forgets the tasks of every session `keep` doesn't say to. Whether any went.
    pub(super) fn retain(&mut self, keep: impl Fn(&str) -> bool) -> bool {
        let count = |tasks: &Self| {
            tasks.on.len()
                + tasks.finished.len()
                + tasks.landing.taken_in.len()
                + tasks.landing.claimed.len()
        };
        let before = count(self);
        self.on.retain(|session, _| keep(session));
        self.implied.retain(|session| keep(session));
        self.finished.retain(|session, _| keep(session));
        self.landing.taken_in.retain(|session, _| keep(session));
        self.landing.claimed.retain(|session, _| keep(session));
        count(self) < before
    }

    /// `session` takes on a new task (`taking:`), or, with `implied`, states an intent:
    /// which starts one only while it's on none. Whether a task started.
    fn start(&mut self, session: &str, implied: bool) -> bool {
        if implied && self.on.contains_key(session) {
            return false;
        }
        self.on.insert(session.to_owned(), new_id(session));
        self.finished.remove(session);
        if implied {
            self.implied.insert(session.to_owned());
        } else {
            self.implied.remove(session);
        }
        true
    }

    /// `session` finished its task (`done:` or `dropped:`) at `now`. Whether it was on one.
    fn finish(&mut self, session: &str, now: u64) -> bool {
        self.implied.remove(session);
        let Some(task) = self.on.remove(session) else {
            return false;
        };
        self.finished
            .insert(session.to_owned(), Finished { task, at: now });
        true
    }

    /// The task `session`'s landing at `now` claims: the one it's on, which ends if an intent
    /// started it, else the one it finished in the last [`LANDING_CLAIMS`] seconds, which
    /// no later landing claims again, else a new one, which is the landing's alone.
    fn land(&mut self, session: &str, now: u64) -> String {
        if let Some(task) = self.on.get(session).cloned() {
            if self.implied.remove(session) {
                self.on.remove(session);
            }
            return task;
        }
        match self.finished.remove(session) {
            Some(finished) if now.saturating_sub(finished.at) <= LANDING_CLAIMS => finished.task,
            _ => new_id(session),
        }
    }
}

/// What zircd says for a session when the landing of the task it took on lands.
pub(super) const DONE_ON_LANDING: &str = "done: landed (said by zircd)";

impl Daemon {
    /// Before `session`'s post `text` goes to `channel`: a `taking:` starts a task,
    /// replacing the one it was on, and the session hears of others' that look like it.
    pub(super) fn task_said(&mut self, session: &str, text: &str, channel: &str) {
        if zirc_proto::says_taking(text) {
            self.same_work_taken(session, text, channel);
        }
        if zirc_proto::says_taking(text) && self.tasks.start(session, false) {
            self.tasks
                .landing
                .taken_in
                .insert(session.to_owned(), channel.to_owned());
            self.save_tasks();
        }
    }

    /// Before `session`'s intent goes: it starts a task, while the session is on none.
    pub(super) fn task_intended(&mut self, session: &str) {
        if self.tasks.start(session, true) {
            self.save_tasks();
        }
    }

    /// After `session`'s post `text` went: a `done:` or `dropped:` ends its task.
    pub(super) fn task_ended(&mut self, session: &str, text: &str) {
        // Its roles end with it: a workflow's next step goes to a member still at work.
        if zirc_proto::says_finished(text)
            && !self.roles_of(session).is_empty()
            && let Err(err) = self.book.set(Field::Roles, session, String::new())
        {
            tracing::warn!("couldn't clear a session's roles: {err}");
        }
        if zirc_proto::says_finished(text) {
            self.same_work_finished(session);
            // What it asked its owner and still waits for: nobody acts on the answer now.
            self.withdraw_requests_of(session);
        }
        if zirc_proto::says_finished(text) && self.tasks.finish(session, super::now()) {
            self.tasks.landing.taken_in.remove(session);
            self.save_tasks();
        }
    }

    /// The task `session`'s landing claims ([`Tasks::land`]).
    pub(super) fn task_landed(&mut self, session: &str) -> String {
        // The intent it declared before is for what this landing carries: its landing ends it.
        self.auto_intents.landing_queued(session, super::now());
        let on = self.tasks.on(session).cloned();
        let task = self.tasks.land(session, super::now());
        // Claimed while still on it, by a `taking:`: this landing landing finishes it.
        match self.tasks.on(session) {
            Some(still) if on.as_ref() == Some(&task) && *still == task => {
                self.tasks
                    .landing
                    .claimed
                    .insert(session.to_owned(), task.clone());
            }
            _ => {
                self.tasks.landing.claimed.remove(session);
            }
        }
        self.save_tasks();
        task
    }

    /// `session`'s landing landed: if it's still on the task that landing claimed, zircd
    /// says `done:` for it where its `taking:` went, and the task ends.
    pub(super) fn task_landing_landed(&mut self, session: &str) {
        let Some(task) = self.tasks.landing.claimed.remove(session) else {
            return;
        };
        let channel = self.tasks.landing.taken_in.get(session).cloned();
        let (Some(channel), true) = (channel, self.tasks.on(session) == Some(&task)) else {
            self.save_tasks();
            return;
        };
        let text = DONE_ON_LANDING.to_owned();
        let message = self.signed(session, Event::Text { body: text.clone() });
        self.publish_message(&channel, message);
        self.task_ended(session, &text);
    }

    /// Forgets the tasks of the sessions the book no longer knows.
    pub(super) fn forget_tasks(&mut self) {
        let book = self.book.clone();
        if self.tasks.retain(|session| book.knows(session)) {
            self.save_tasks();
        }
    }

    fn save_tasks(&self) {
        if let Err(err) = self.tasks.save(&self.org.state) {
            tracing::warn!("couldn't save sessions' tasks: {err}");
        }
    }
}

/// A new task's id for `session`: `t-`, the time in milliseconds and two characters of
/// the session's, in base 36, so two sessions starting one at once differ.
fn new_id(session: &str) -> String {
    let millis = u128::from(crate::clock::unix_ms());
    let salt = session.bytes().fold(0u32, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(u32::from(byte))
    }) % (36 * 36);
    format!("t-{}{:0>2}", base36(millis), base36(u128::from(salt)))
}

fn base36(mut n: u128) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[usize::try_from(n % 36).unwrap_or_default()]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_a_task_id() {
        let id = new_id("cc-f43c76");
        assert_eq!(zirc_proto::task_id(&id), Some(id.clone()), "{id}");
        assert_ne!(new_id("cc-f43c76"), new_id("cc-b30819"));
        assert_eq!(base36(0), "0");
        assert_eq!(base36(36 * 36 - 1), "zz");
    }

    #[test]
    fn each_organisation_keeps_its_own_and_forgets_sessions_no_longer_known() {
        let home = tempfile::tempdir().unwrap();
        let (first, other) = (home.path().to_owned(), home.path().join("orgs/acme"));
        let mut ours = Tasks::load(&first);
        let mut theirs = Tasks::load(&other);
        ours.start("cc-a", false);
        ours.save(&first).unwrap();
        theirs.start("cc-b", false);
        theirs.finish("cc-b", 100);
        theirs
            .landing
            .taken_in
            .insert("cc-c".into(), "#api/agents".into());
        theirs.landing.claimed.insert("cc-c".into(), "t-1".into());
        theirs.save(&other).unwrap();
        // Neither saved over the other's.
        let ours = Tasks::load(&first);
        assert!(ours.on("cc-a").is_some() && ours.on("cc-b").is_none());
        let mut theirs = Tasks::load(&other);
        assert!(theirs.finished.contains_key("cc-b") && theirs.on("cc-a").is_none());
        // What its landing's done: needs, kept across restarts too.
        assert_eq!(theirs.landing.claimed["cc-c"], "t-1");
        assert!(theirs.retain(|session| session != "cc-b"));
        assert!(theirs.finished.is_empty());
        assert!(theirs.retain(|session| session != "cc-c"));
        assert!(theirs.landing.taken_in.is_empty() && theirs.landing.claimed.is_empty());
        assert!(!theirs.retain(|_| true));
    }

    #[test]
    fn a_landing_after_done_claims_the_task_once() {
        let mut kept = Tasks::default();
        assert!(kept.start("cc-a", false));
        let task = kept.on["cc-a"].clone();
        assert!(kept.finish("cc-a", 100));
        assert_eq!(kept.on.get("cc-a"), None, "done: ends it");
        assert_eq!(kept.land("cc-a", 200), task);
        // Ids go by the millisecond.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let next = kept.land("cc-a", 300);
        assert_ne!(next, task, "claimed once: the next landing's is its own");
        assert_eq!(zirc_proto::task_id(&next), Some(next.clone()));
        assert_eq!(kept.on.get("cc-a"), None, "and ends with it");
    }

    #[test]
    fn a_finished_task_is_claimed_only_for_a_while() {
        let mut kept = Tasks::default();
        kept.start("cc-a", false);
        kept.finish("cc-a", 100);
        let task = kept.finished["cc-a"].task.clone();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_ne!(kept.land("cc-a", 101 + LANDING_CLAIMS), task);
    }

    #[test]
    fn a_landing_leaves_a_taken_task_on() {
        let mut kept = Tasks::default();
        kept.start("cc-a", false);
        let task = kept.on["cc-a"].clone();
        assert_eq!(kept.land("cc-a", 1), task);
        assert_eq!(kept.land("cc-a", 2), task, "on until done:");
    }

    #[test]
    fn an_intent_starts_a_task_its_landing_ends() {
        let mut kept = Tasks::default();
        assert!(kept.start("cc-a", true));
        let task = kept.on["cc-a"].clone();
        assert!(!kept.start("cc-a", true), "the next intent keeps it");
        assert_eq!(kept.on["cc-a"], task);
        assert_eq!(kept.land("cc-a", 1), task);
        assert_eq!(kept.on.get("cc-a"), None);
        // Ids go by the millisecond.
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(kept.start("cc-a", true), "the next intent starts the next");
        assert_ne!(kept.on["cc-a"], task);
    }

    #[test]
    fn an_intent_keeps_a_taken_task_and_taking_replaces_an_implied_one() {
        let mut kept = Tasks::default();
        kept.start("cc-a", false);
        let taken = kept.on["cc-a"].clone();
        assert!(!kept.start("cc-a", true));
        assert_eq!(kept.on["cc-a"], taken);
        kept.finish("cc-a", 1);
        kept.start("cc-b", true);
        kept.start("cc-b", false);
        assert!(
            !kept.implied.contains("cc-b"),
            "a taking: is no implied task"
        );
        kept.land("cc-b", 2);
        assert!(kept.on.contains_key("cc-b"), "so its landing leaves it on");
    }
}
