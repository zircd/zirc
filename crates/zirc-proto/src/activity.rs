//! What only a session's machine knows of how a team works together, for measuring it:
//! which lines woke a session's watch ([`Wake`]), what the session did after
//! ([`WakeOutcome`]), where each of its prompts came from and what it cost ([`Prompt`]), and
//! what zircd told it of others changing what it changes ([`Notice`]). Each goes as a [`crate::Event::Custom`] of one of [`KINDS`], at debug: only
//! true now ([`crate::live`]), so none is a line of any channel's history or chat, and
//! nothing wakes on one. The tenant's recorder (its sidecar, nick [`RECORDER`]) hears them
//! as they're sent and keeps a row of each beside the history, as long as the
//! organisation keeps its messages.
//!
//! Metadata only: ids, kinds, levels, counts and times. Never what anyone said, a path or
//! a prompt's words.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::telemetry::OverlapKind;
use crate::{Event, Level};

/// A session's watch woke on a line ([`Wake`]).
pub const WAKE_KIND: &str = "x-zirc/wake";

/// What a session did after a wake ([`WakeOutcome`]).
pub const WAKE_OUTCOME_KIND: &str = "x-zirc/wake-outcome";

/// One of a session's prompts ([`Prompt`]).
pub const PROMPT_KIND: &str = "x-zirc/prompt";

/// zircd told a session that another changes what it changes ([`Notice`]).
pub const NOTICE_KIND: &str = "x-zirc/notice";

/// Every kind here: each is live ([`crate::live`]).
pub const KINDS: [&str; 4] = [WAKE_KIND, WAKE_OUTCOME_KIND, PROMPT_KIND, NOTICE_KIND];

/// The nick a tenant's recorder goes by, to which machines send these events directly
/// rather than to a channel, so no one else's client is sent them.
pub const RECORDER: &str = "zirc-recorder";

/// A session's watch woke on a line: what the line was, never what it said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Wake {
    /// This wake's id, which its [`WakeOutcome`] names.
    pub id: String,
    /// Why it concerned the session, as `zirc watch` says it: `mention`, `overlap`,
    /// `paths`, `landing` and so on.
    pub why: String,
    /// The line's kind: its event's (`text`, `intent`, `commit`) or, for a custom event,
    /// its custom kind (`x-zirc/landing`).
    pub kind: String,
    /// The line's level ([`crate::Message::level`]), when it says one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    /// The line's id on the server (`msgid`), when it has one, so the recorder can join
    /// the wake to its row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// For an intent, that zircd said it for a session by itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub automatic: bool,
}

/// What a session did first after a wake, within the time zircd waits
/// (`zirc_core::ipc::WAKE_ACTED_SECS`), or that it did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Acted {
    /// It posted, answered or wrote to someone.
    Posted,
    /// It committed.
    Committed,
    /// It pushed.
    Pushed,
    /// It said what it's about to change.
    Intended,
    /// It reacted to a message.
    Reacted,
    /// It asked zircd for something else (a read, a lease, a landing).
    Asked,
    /// Nothing, in time.
    Nothing,
    /// One a newer zircd says that this one doesn't know.
    #[serde(other)]
    Other,
}

/// What a session did after the wake [`WakeOutcome::wake`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WakeOutcome {
    /// The wake's id ([`Wake::id`]).
    pub wake: String,
    /// What the session did first.
    pub acted: Acted,
    /// How many seconds after the wake, when it acted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
}

/// Where a session's prompt came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The person, typing.
    Person,
    /// Another agent session, such as one that started it or relays to it.
    Session,
    /// Something automatic: a watch waking it, a background task ending, a schedule.
    Automated,
    /// One a newer client says that this one doesn't know.
    #[serde(other)]
    Other,
}

/// One prompt to a session, and what answering it cost, as its agent tool's hook tells
/// zircd: counts, never words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Prompt {
    /// Where it came from.
    pub source: Source,
    /// Tokens read fresh, written to and read from the cache, and written out, answering
    /// it, when the tool says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// See [`Prompt::input_tokens`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// See [`Prompt::input_tokens`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// See [`Prompt::input_tokens`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// The model that answered, such as `claude-opus-5-5`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// zircd told a session that another changes what it changes: counted with what it amounts
/// to, never the paths or lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Notice {
    /// What it was told of.
    pub what: Noticed,
    /// For an overlap, what it amounts to: the worst of its paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap: Option<OverlapKind>,
    /// Whether the other side is on this machine, rather than another.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// Whether it woke the session. An overlap in different parts of the files isn't news
    /// to a session, so it doesn't.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub woke: bool,
    /// Whether it follows an overlap in different parts of a file that now is in the same
    /// lines.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub escalated: bool,
    /// The other side: its session when it's on this machine and has one, or else the other
    /// machine's nick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<String>,
}

/// What a [`Notice`] told a session of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Noticed {
    /// Both change the same files.
    Overlap,
    /// A branch someone pushed changes the same lines it's changing.
    ConflictAhead,
    /// One this build doesn't know.
    #[serde(other)]
    Other,
}

/// One of these records as the event it goes as.
pub trait Record: Serialize + for<'de> Deserialize<'de> {
    /// Its kind, one of [`KINDS`].
    const KIND: &'static str;

    /// The event that carries it, with a few words for anyone reading raw IRC.
    ///
    /// # Panics
    ///
    /// Never: each record serializes to JSON.
    #[must_use]
    fn event(&self) -> Event {
        Event::Custom {
            kind: Self::KIND.to_owned(),
            data: serde_json::to_value(self).expect("an activity record serializes"),
            text: Self::KIND.trim_start_matches("x-zirc/").to_owned(),
        }
    }

    /// The record `event` carries, if it's one of this kind.
    #[must_use]
    fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == Self::KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }
}

impl Record for Wake {
    const KIND: &'static str = WAKE_KIND;
}

impl Record for WakeOutcome {
    const KIND: &'static str = WAKE_OUTCOME_KIND;
}

impl Record for Prompt {
    const KIND: &'static str = PROMPT_KIND;
}

impl Record for Notice {
    const KIND: &'static str = NOTICE_KIND;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[test]
    fn each_kind_is_zircs_and_live() {
        for kind in KINDS {
            assert_eq!(crate::check_kind(kind), None, "{kind}");
        }
        let wake = Wake {
            id: "w1".into(),
            why: "mention".into(),
            kind: "text".into(),
            level: Some(Level::Info),
            line: Some("m1".into()),
            automatic: false,
        };
        let message = Message::new("cc-1", wake.event());
        assert!(message.is_live(), "a record never enters history");
        assert_eq!(Wake::of(&message.event), Some(wake));
        assert_eq!(WakeOutcome::of(&message.event), None);
    }

    #[test]
    fn reads_what_a_newer_sender_says() {
        let outcome: WakeOutcome =
            serde_json::from_value(serde_json::json!({"wake": "w1", "acted": "sang", "later": 1}))
                .unwrap();
        assert_eq!(outcome.acted, Acted::Other);
        let prompt: Prompt =
            serde_json::from_value(serde_json::json!({"source": "person", "output_tokens": 12}))
                .unwrap();
        assert_eq!(prompt.source, Source::Person);
        assert_eq!(prompt.output_tokens, Some(12));
        let notice: Notice =
            serde_json::from_value(serde_json::json!({"what": "rumour", "woke": true})).unwrap();
        assert_eq!(notice.what, Noticed::Other);
        assert!(notice.woke && !notice.local && notice.overlap.is_none());
    }

    #[test]
    fn a_notice_says_what_an_overlap_amounts_to_and_never_a_path() {
        let notice = Notice {
            what: Noticed::Overlap,
            overlap: Some(OverlapKind::SameLines),
            local: true,
            woke: true,
            escalated: false,
            with: Some("cc-2".into()),
        };
        assert_eq!(
            serde_json::to_value(&notice).unwrap(),
            serde_json::json!({"what": "overlap", "overlap": "same_lines", "local": true,
                "woke": true, "with": "cc-2"})
        );
        assert_eq!(Notice::of(&notice.event()), Some(notice));
    }
}
