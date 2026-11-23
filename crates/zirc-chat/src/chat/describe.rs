//! Conversations put into words: zirc's events as a kind and a headline, who sent them,
//! each entry as one line of plain text, and which entries read as one run. Plain words
//! only; each UI picks its own colours and layout.

use std::fmt::Write as _;

use chrono::TimeDelta;
use zirc_proto::Event;

use super::body::strip_formatting;
use super::conversation::{Entry, EntryKind};

/// Messages from one sender this close together read as one run, as in Slack and Discord.
pub const RUN_GAP: TimeDelta = TimeDelta::minutes(5);

/// What sort of event, for its badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// What a session means to work on.
    Intent,
    /// Uncommitted changes.
    Working,
    /// A commit.
    Commit,
    /// A push.
    Push,
    /// Free text from a person or agent.
    Post,
    /// A file shared.
    File,
    /// A question for agents ([`Event::Query`]).
    Query,
    /// An answer to one.
    Answer,
}

impl EventKind {
    /// The badge's word.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Working => "working",
            Self::Commit => "commit",
            Self::Push => "push",
            Self::Post => "post",
            Self::File => "file",
            Self::Query => "query",
            Self::Answer => "answer",
        }
    }
}

/// An event put into words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    /// Its kind.
    pub kind: EventKind,
    /// One line saying what happened.
    pub headline: String,
    /// The paths it's about, in full, for a working set, an intent or a push.
    pub paths: Vec<String>,
}

/// `event` put into words, for a view that shows its kind and paths apart from its
/// headline. [`zirc_proto::Event::human_text`] is the same event as one sentence.
#[must_use]
pub fn describe(event: &Event) -> Described {
    let (kind, headline, paths) = match event {
        Event::Intent { summary, paths, .. } => {
            (EventKind::Intent, strip_formatting(summary), paths.clone())
        }
        Event::Working {
            paths,
            continued,
            branch,
            ..
        } => {
            // A part for one worktree of several says its branch; an older zircd's parts
            // only continue the list.
            let on = branch
                .as_ref()
                .map(|branch| format!(" on {}", strip_formatting(branch)))
                .unwrap_or_default();
            let headline = match (paths.len(), continued, branch.is_some()) {
                (n, true, false) => format!("and {}", files(n)),
                (0, false, _) => format!("committed or dropped all changes{on}"),
                (n, _, _) => format!("has uncommitted changes in {}{on}", files(n)),
            };
            (EventKind::Working, headline, paths.clone())
        }
        Event::Commit {
            branch,
            sha,
            subject,
            ..
        } => (
            EventKind::Commit,
            format!("{}  ({branch} {})", strip_formatting(subject), short(sha)),
            Vec::new(),
        ),
        Event::Push {
            branch,
            head,
            forced,
            subjects,
            paths,
            ..
        } => {
            let subjects: Vec<String> = subjects.iter().map(|s| strip_formatting(s)).collect();
            (
                EventKind::Push,
                zirc_proto::push_line(branch, head, *forced, &subjects),
                paths.clone(),
            )
        }
        Event::File(file) => (
            EventKind::File,
            format!(
                "shared {} ({})",
                strip_formatting(&file.name),
                super::file::size_text(file.size)
            ),
            Vec::new(),
        ),
        Event::Query { to, text, .. } => (
            EventKind::Query,
            match to {
                Some(session) => format!("to {session}: {}", strip_formatting(text)),
                None => strip_formatting(text),
            },
            Vec::new(),
        ),
        Event::Answer { text, .. } => (EventKind::Answer, strip_formatting(text), Vec::new()),
        // A note for one session, which asks no answer: a post to it.
        Event::Note { to, text } => (
            EventKind::Post,
            format!("to {to}: {}", strip_formatting(text)),
            Vec::new(),
        ),
        // A team's own event reads as a post of its text, until the apps draw cards for them.
        Event::Text { body } | Event::Custom { text: body, .. } => {
            (EventKind::Post, strip_formatting(body), Vec::new())
        }
    };
    Described {
        kind,
        headline,
        paths,
    }
}

/// Who sent a message: `nick/session`, or the nick alone when the session adds nothing, as
/// for a person.
#[must_use]
pub fn sender(nick: &str, session: &str) -> String {
    if session.is_empty() || session == nick {
        nick.to_owned()
    } else {
        format!("{nick}/{session}")
    }
}

/// What a notification of a zirc message says, and Activity's row of one: a text post's
/// words, any other event's headline; `None` for an agent saying it's working, which is
/// never one.
#[must_use]
pub fn notice_text(event: &Event) -> Option<String> {
    match event {
        Event::Working { .. } => None,
        Event::Text { body } => Some(body.clone()),
        other => Some(describe(other).headline),
    }
}

/// An entry as one line of plain text, as a log, a script or a screen reader reads it.
#[must_use]
pub fn entry_text(entry: &Entry) -> String {
    let from = &entry.from;
    let with_reason = |text: String, reason: &Option<String>| match reason {
        Some(reason) => format!("{text} ({reason})"),
        None => text,
    };
    match &entry.kind {
        EntryKind::Message(text) => format!("<{from}> {text}"),
        EntryKind::Action(text) | EntryKind::Unreadable(text) => format!("* {from} {text}"),
        EntryKind::Notice(text) => format!("-{from}- {text}"),
        EntryKind::Zirc(message) => {
            let described = describe(&message.event);
            let mut line = format!(
                "[{}] {}: {}",
                described.kind.label(),
                super::sender_text(
                    from,
                    entry.owner.as_deref(),
                    Some(message),
                    &str::to_owned,
                    &str::to_owned
                ),
                described.headline
            );
            if !described.paths.is_empty() {
                let _ = write!(line, " ({})", described.paths.join(", "));
            }
            line
        }
        EntryKind::Join => format!("{from} joined"),
        EntryKind::Part(reason) => with_reason(format!("{from} left"), reason),
        EntryKind::Quit(reason) => with_reason(format!("{from} quit"), reason),
        EntryKind::Kick { nick, reason } => with_reason(format!("{from} removed {nick}"), reason),
        EntryKind::Nick(new) => format!("{from} is now {new}"),
        EntryKind::Topic(topic) => format!("{from} set the topic: {topic}"),
        EntryKind::Info(text) => text.clone(),
    }
}

/// What copying an entry copies: what was said, without who said it, for messages,
/// actions and notices, and otherwise the entry as [`entry_text`] puts it.
#[must_use]
pub fn copy_text(entry: &Entry) -> String {
    match &entry.kind {
        EntryKind::Message(text) | EntryKind::Action(text) | EntryKind::Notice(text) => {
            text.clone()
        }
        _ => entry_text(entry),
    }
}

/// The first web link in `text`, without the punctuation that ends a sentence after it.
#[must_use]
pub fn first_link(text: &str) -> Option<&str> {
    text.split_whitespace()
        .map(|word| word.trim_start_matches(['(', '<', '"', '\'']))
        .find(|word| word.starts_with("https://") || word.starts_with("http://"))
        .map(|word| word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '>', '"', '\'']))
}

/// How many files, in words: `1 file`, `14 files`.
pub(super) fn files(n: usize) -> String {
    match n {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    }
}

pub(super) fn short(sha: &str) -> &str {
    zirc_proto::short_sha(sha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use zirc_proto::Message;

    fn entry(minute: u32, from: &str, kind: EntryKind) -> Entry {
        Entry {
            from: from.into(),
            owner: None,
            kind,
            ..Entry::info(
                Utc.with_ymd_and_hms(2026, 10, 30, 12, minute, 0).unwrap(),
                String::new(),
            )
        }
    }

    #[test]
    fn puts_events_into_words() {
        let push = describe(&Event::Push {
            branch: "login".into(),
            head: "0123456789".into(),
            forced: false,
            subjects: vec!["One".into(), "Two".into()],
            paths: vec!["src/login.rs".into()],
            base: None,
            lines: std::collections::BTreeMap::new(),
            repo: None,
            on: None,
        });
        assert_eq!(push.kind, EventKind::Push);
        assert_eq!(
            push.headline,
            "pushed login to 0123456 (2 commits): One; Two"
        );
        assert_eq!(push.paths, ["src/login.rs"]);
        let working = describe(&Event::Working {
            paths: vec!["a.rs".into()],
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        });
        assert_eq!(working.headline, "has uncommitted changes in 1 file");
        assert_eq!(working.paths, ["a.rs"]);
        let part = describe(&Event::Working {
            paths: vec!["b.rs".into(), "c.rs".into()],
            continued: true,
            worktree: Some("api-hooks".into()),
            branch: Some("retry-hooks".into()),
            part: Some([2, 2]),
            base: None,
            committed: Vec::new(),
        });
        assert_eq!(
            part.headline,
            "has uncommitted changes in 2 files on retry-hooks"
        );
    }

    #[test]
    fn writes_each_entry_as_a_line() {
        let commit = Message::new(
            "cc-1",
            Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: "abcdef12345".into(),
                subject: "Fix".into(),
                repo: None,
            },
        );
        assert_eq!(
            entry_text(&entry(0, "mbp", EntryKind::Zirc(commit))),
            "[commit] mbp: Fix  (main abcdef1)"
        );
        assert_eq!(
            entry_text(&entry(0, "bob", EntryKind::Part(Some("bye".into())))),
            "bob left (bye)"
        );
    }

    #[test]
    fn copies_what_was_said_without_who_said_it() {
        let said = entry(
            0,
            "alice",
            EntryKind::Message("see https://zirc.dev.".into()),
        );
        assert_eq!(copy_text(&said), "see https://zirc.dev.");
        let joined = entry(0, "alice", EntryKind::Join);
        assert_eq!(copy_text(&joined), "alice joined");
    }

    #[test]
    fn finds_the_first_link() {
        assert_eq!(
            first_link("see (https://zirc.dev/m/a?w=1), or http://b.test."),
            Some("https://zirc.dev/m/a?w=1")
        );
        assert_eq!(first_link("<http://a.test/x>"), Some("http://a.test/x"));
        assert_eq!(first_link("no links, httpx://nope"), None);
    }
}
