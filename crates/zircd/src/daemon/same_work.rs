//! A session taking on work another has taken on already. Two people asked for one thing in
//! their own words, and the second session built it again: nothing told it the first had
//! posted its `taking:` (the agent evals' C5, 16 Nov 2026).
//!
//! zircd keeps the `taking:` of each session it hears, this machine's and other machines',
//! by the repository's scope it went to, until that session's `done:` or `dropped:` or for
//! [`OPEN_FOR`]. When a session here posts `taking:`, each other session's open task in the
//! same repository that looks like the same work ([`zirc_core::same_work::alike`]) is told to
//! it at once, in a line of zircd's own naming that session and what it took on, with how to
//! ask it before building the same thing. Kept only while zircd runs, as what a task is never
//! is ([`super::tasks`]); history replayed after a short gap brings back what was said
//! meanwhile.

use std::collections::{BTreeMap, BTreeSet};

use zirc_core::ipc::Delivery;
use zirc_core::same_work;
use zirc_proto::Event;

use super::{Daemon, WATCHER_SESSION, now};

/// How long a `taking:` with no `done:` or `dropped:` after it counts as open: a day, as
/// a task left unfinished overnight is most likely abandoned.
const OPEN_FOR: u64 = 24 * 60 * 60;

/// A task some session took on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    /// Its `taking:` post, as it said it.
    text: String,
    /// When, in seconds since the Unix epoch.
    at: u64,
}

/// The tasks sessions have taken on and not finished, by the repository channel their
/// `taking:` went to, machine (lowercase) and session.
#[derive(Debug, Default)]
pub(super) struct OpenTasks {
    open: BTreeMap<(String, String, String), Open>,
    /// What each session here has been told, by it, the other session and its task, so a
    /// `taking:` said again doesn't tell it again.
    told: BTreeSet<(String, String, String)>,
}

/// Another session's open task that looks like the same work.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Alike {
    machine: String,
    session: String,
    text: String,
}

impl OpenTasks {
    /// `machine`'s `session` took on `text` in `channel` at `at`, replacing what it was on.
    fn taken(&mut self, channel: &str, machine: &str, session: &str, text: &str, at: u64) {
        self.finished(machine, session);
        self.open
            .retain(|_, open| at.saturating_sub(open.at) < OPEN_FOR);
        let text = text.trim().to_owned();
        let key = (
            channel.to_owned(),
            machine.to_ascii_lowercase(),
            session.to_owned(),
        );
        self.open.insert(key, Open { text, at });
    }

    /// `machine`'s `session` finished its task.
    fn finished(&mut self, machine: &str, session: &str) {
        let machine = machine.to_ascii_lowercase();
        self.open
            .retain(|(_, m, s), _| !(*m == machine && s == session));
    }

    /// The other sessions' tasks open in `channel` at `now` that look like `text`, the
    /// `taking:` of `machine`'s `session`, not told to it before.
    fn alike(
        &mut self,
        channel: &str,
        (machine, session): (&str, &str),
        text: &str,
        now: u64,
    ) -> Vec<Alike> {
        let machine = machine.to_ascii_lowercase();
        let found: Vec<Alike> = self
            .open
            .iter()
            .filter(|((c, m, s), open)| {
                c == channel
                    && !(*m == machine && s == session)
                    && now.saturating_sub(open.at) < OPEN_FOR
                    && same_work::alike(text, &open.text)
            })
            .map(|((_, m, s), open)| Alike {
                machine: m.clone(),
                session: s.clone(),
                text: open.text.clone(),
            })
            .collect();
        found
            .into_iter()
            .filter(|alike| {
                self.told.insert((
                    session.to_owned(),
                    format!("{}/{}", alike.machine, alike.session),
                    alike.text.clone(),
                ))
            })
            .collect()
    }
}

impl Daemon {
    /// Before `session`'s `taking:` post `text` goes to `channel`: tells it of each other
    /// session's open task there that looks like the same work, then keeps its own.
    pub(super) fn same_work_taken(&mut self, session: &str, text: &str, channel: &str) {
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return;
        };
        let at = now();
        let found = self.open_tasks.alike(channel, (&me, session), text, at);
        for alike in found {
            tracing::info!(
                "{session}'s task looks like {}/{}'s",
                alike.machine,
                alike.session
            );
            self.tell_dropped(channel, same_work_line(session, &alike));
        }
        self.open_tasks.taken(channel, &me, session, text, at);
    }

    /// After this machine's `session` posted `done:` or `dropped:`.
    pub(super) fn same_work_finished(&mut self, session: &str) {
        if let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) {
            self.open_tasks.finished(&me, session);
        }
    }

    /// Another machine's post, heard live or replayed: a `taking:` keeps its task, a
    /// `done:` or `dropped:` ends it.
    pub(super) fn same_work_heard(&mut self, delivery: &Delivery) {
        let Event::Text { body } = &delivery.message.event else {
            return;
        };
        let session = delivery.message.session.as_str();
        if session.is_empty() || session == WATCHER_SESSION {
            return;
        }
        if zirc_proto::says_taking(body) {
            let channel = delivery.scope();
            let at = delivery
                .time
                .as_deref()
                .and_then(zirc_core::time::seconds_at)
                .unwrap_or_else(now);
            self.open_tasks
                .taken(&channel, &delivery.from, session, body, at);
        } else if zirc_proto::says_finished(body) {
            self.open_tasks.finished(&delivery.from, session);
        }
    }
}

/// What `session`, taking on work that looks like `alike`'s, hears.
fn same_work_line(session: &str, alike: &Alike) -> String {
    let other = format!("{}/{}", alike.machine, alike.session);
    format!(
        "{session}: {other} may be on the same work already, and hasn't said done: \"{}\". \
         Before you build it too, ask it: `zirc ask --session {session} --to {other}`, saying \
         what you were asked and asking what its task covers and how far it is; then leave it \
         to them, build on theirs or split the work, and tell the user which",
        alike.text
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "taking: add a CSV export of orders";
    const SPREADSHEET: &str = "taking: let staff download orders as a spreadsheet file (csv)";

    #[test]
    fn tells_another_sessions_open_alike_task_once() {
        let mut open = OpenTasks::default();
        open.taken("#api", "Alpha", "cc-a", CSV, 100);
        open.taken("#api", "beta", "cc-c", "taking: fix tax rounding", 100);
        let found = open.alike("#api", ("beta", "cc-b"), SPREADSHEET, 200);
        assert_eq!(
            found,
            [Alike {
                machine: "alpha".into(),
                session: "cc-a".into(),
                text: CSV.into(),
            }]
        );
        assert!(
            open.alike("#api", ("beta", "cc-b"), SPREADSHEET, 201)
                .is_empty()
        );
        // Not its own, nor another repository's.
        assert!(
            open.alike("#api", ("ALPHA", "cc-a"), SPREADSHEET, 200)
                .is_empty()
        );
        assert!(
            open.alike("#web", ("beta", "cc-d"), SPREADSHEET, 200)
                .is_empty()
        );
    }

    #[test]
    fn a_finished_or_old_task_or_one_replaced_is_not_open() {
        let mut open = OpenTasks::default();
        open.taken("#api", "alpha", "cc-a", CSV, 100);
        open.finished("ALPHA", "cc-a");
        assert!(
            open.alike("#api", ("beta", "cc-b"), SPREADSHEET, 200)
                .is_empty()
        );
        open.taken("#api", "alpha", "cc-a", CSV, 100);
        assert!(
            open.alike("#api", ("beta", "cc-b"), SPREADSHEET, 100 + OPEN_FOR)
                .is_empty()
        );
        open.finished("alpha", "cc-a");
        open.taken("#api", "alpha", "cc-e", CSV, 100);
        open.taken("#api", "alpha", "cc-e", "taking: the README", 150);
        assert!(
            open.alike("#api", ("beta", "cc-f"), SPREADSHEET, 200)
                .is_empty()
        );
    }

    #[test]
    fn says_whose_task_it_is_and_to_ask_it() {
        let line = same_work_line(
            "cc-b",
            &Alike {
                machine: "alpha".into(),
                session: "cc-a".into(),
                text: CSV.into(),
            },
        );
        assert!(line.starts_with("cc-b: alpha/cc-a may be on the same work already"));
        assert!(line.contains(CSV));
        assert!(line.contains("zirc ask --session cc-b --to alpha/cc-a"));
    }
}
