//! Activity, as every app lists it: what concerns us, newest first. The phones show it in a
//! tab of its own, the desktops and web in the header's inbox (the user's choice, 11 Nov
//! 2026). It lists what was aimed at us and nothing else (the user's choice, 13 Nov 2026):
//! mentions of us in channels, replies to our messages there, and reactions to what we said
//! anywhere, each saying what happened in words every app shares. Direct messages aren't
//! Activity, a reply in one included: the sidebar's own rows and badges say those, as they
//! say an agent's post or a channel set to notify everything.

use std::cmp::Reverse;
use std::collections::HashSet;

use serde::Serialize;
use ts_rs::TS;

use super::conversation::{Conversation, Entry, EntryKind, Kind};
use super::describe::notice_text;
use super::lines::Keys;
use super::words::ACTIVITY;
use super::{Chat, LineSpeaker};

/// What a mention's row says it is.
pub const MENTIONED: &str = "mentioned you";
/// What a reply's row says it is.
pub const REPLIED: &str = "replied to you";
/// What a reaction's row says it is, before its emoji: `reacted 👍`.
pub const REACTED: &str = "reacted";
/// After a count, on the inbox's button: `3 unread`.
pub const UNREAD: &str = "unread";

/// What happened, on an Activity row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ActivityKind {
    /// Someone named us in a channel.
    Mention,
    /// Someone answered one of our messages.
    Reply,
    /// Someone reacted to one of our messages.
    Reaction,
}

/// One row of Activity, ready to draw and send as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ActivityRow {
    /// What happened.
    pub kind: ActivityKind,
    /// The conversation it happened in, to open.
    pub conversation: String,
    /// That conversation as the sidebar names it: `#web`, `Leo Okafor`.
    pub place: String,
    /// The message to open it at, as its line's key: theirs for a mention or a reply, ours
    /// for a reaction.
    pub key: String,
    /// Who: who named us, answered or reacted, as a line's head shows them.
    pub speaker: LineSpeaker,
    /// What they did, in every app's words: `mentioned you`, `replied to you`, `reacted 👍`.
    pub what: String,
    /// What they did and where, as a row says it after who: `replied to you in #web`; in a
    /// conversation with them alone, what they did: `reacted 👍`.
    pub headline: String,
    /// For a reaction, its emoji.
    pub emoji: Option<String>,
    /// The words: theirs for a mention or a reply, ours for what they reacted to.
    pub text: String,
    /// Those words as a row shows them ([`Chat::shown_text`]): plain, each mention by the
    /// name it shows as.
    pub excerpt: String,
    /// When it happened, in milliseconds since 1970: a reaction's when it was given.
    pub time_ms: i64,
    /// Whether it's after what the person has read of its conversation: Activity's badge
    /// counts these ([`super::Badges::activity`]).
    pub unread: bool,
}

/// The inbox's button in a header, as the web, the desktop and the Mac draw it (the user's
/// choice, 12 Nov 2026): a bell that opens Activity in a panel under it, with a dot while
/// anything there is unread. The phones have Activity's tab instead, with its count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct InboxButton {
    /// Whether anything in Activity is unread ([`Chat::activity_unread`]): the bell's dot.
    pub dot: bool,
    /// What it's called, on hover and to a screen reader: `Activity`, or with what's
    /// unread, `Activity, 3 unread`.
    pub label: String,
}

/// One of Activity's rows before it's worded: what [`Chat::activity`] words and
/// [`Chat::activity_unread`] counts.
struct Hit<'a> {
    kind: ActivityKind,
    key: String,
    /// Who: the entry they said, or for a reaction one made up for them.
    who: Entry,
    said: String,
    emoji: Option<&'a str>,
    at: i64,
    unread: bool,
}

impl Chat {
    /// Activity, newest first: mentions of us and replies to our messages in channels, and
    /// reactions to our messages in every conversation we have, never our own.
    #[must_use]
    pub fn activity(&self) -> Vec<ActivityRow> {
        let mut rows: Vec<ActivityRow> = self
            .conversations
            .iter()
            .flat_map(|conversation| {
                let place = self.title(&conversation.name);
                self.activity_in(conversation)
                    .into_iter()
                    .map(move |hit| self.activity_row(conversation, &place, hit))
                    .collect::<Vec<_>>()
            })
            .collect();
        rows.sort_by_key(|row| Reverse(row.time_ms));
        rows
    }

    /// The header's inbox button ([`InboxButton`]).
    #[must_use]
    pub fn inbox_button(&self) -> InboxButton {
        let unread = self.activity_unread();
        InboxButton {
            dot: unread > 0,
            label: if unread > 0 {
                format!("{ACTIVITY}, {unread} {UNREAD}")
            } else {
                ACTIVITY.to_owned()
            },
        }
    }

    /// How many of Activity's rows are unread ([`ActivityRow::unread`]): its badge.
    #[must_use]
    pub fn activity_unread(&self) -> usize {
        self.conversations
            .iter()
            .map(|conversation| {
                self.activity_in(conversation)
                    .iter()
                    .filter(|hit| hit.unread)
                    .count()
            })
            .sum()
    }

    /// `hit`, in `conversation`, called `place` in the sidebar, as Activity's row of it.
    fn activity_row(&self, conversation: &Conversation, place: &str, hit: Hit<'_>) -> ActivityRow {
        let alone = conversation.kind == Kind::Query;
        let what = match (hit.kind, hit.emoji) {
            (ActivityKind::Reaction, Some(emoji)) => format!("{REACTED} {emoji}"),
            (ActivityKind::Reaction, None) => REACTED.to_owned(),
            (ActivityKind::Reply, _) => REPLIED.to_owned(),
            (ActivityKind::Mention, _) => MENTIONED.to_owned(),
        };
        ActivityRow {
            kind: hit.kind,
            conversation: conversation.name.clone(),
            place: place.to_owned(),
            key: hit.key,
            speaker: self.line_speaker(&hit.who),
            headline: if alone {
                what.clone()
            } else {
                format!("{what} in {place}")
            },
            what,
            emoji: hit.emoji.map(str::to_owned),
            excerpt: self.shown_text(&hit.said),
            text: hit.said,
            time_ms: hit.at,
            unread: hit.unread,
        }
    }

    /// `conversation`'s Activity, unworded.
    fn activity_in<'a>(&self, conversation: &'a Conversation) -> Vec<Hit<'a>> {
        let mut hits = Vec::new();
        let mine = |entry: &Entry| entry.from.eq_ignore_ascii_case(&self.me);
        // After what's read: since the server's read marker, or without one, what arrived
        // live while it's unread.
        let unread = |at: chrono::DateTime<chrono::Utc>, history: bool| match conversation.read {
            Some(read) => at > read,
            None => !history && conversation.unread > 0,
        };
        let mut keys = Keys::default();
        let keyed: Vec<(String, &Entry)> = conversation
            .entries
            .iter()
            .map(|entry| (keys.of(entry), entry))
            .collect();
        let ours: HashSet<&str> = conversation
            .entries
            .iter()
            .filter(|entry| mine(entry))
            .filter_map(|entry| entry.id.as_deref())
            .collect();
        // What's said in a conversation with one nick is a direct message, which the
        // sidebar says: only a reaction there is Activity.
        let channel = conversation.kind == Kind::Channel;
        for (key, entry) in keyed.iter().filter(|_| channel) {
            if entry.from.is_empty() || mine(entry) {
                continue;
            }
            let Some(said) = words(entry) else {
                continue;
            };
            // A reply that names us too is a reply.
            let kind = if entry
                .reply_to
                .as_deref()
                .is_some_and(|id| ours.contains(id))
            {
                ActivityKind::Reply
            } else if entry.mention {
                ActivityKind::Mention
            } else {
                continue;
            };
            hits.push(Hit {
                kind,
                key: key.clone(),
                who: (*entry).clone(),
                said,
                emoji: None,
                at: entry.time.timestamp_millis(),
                unread: unread(entry.time, entry.history),
            });
        }
        for given in conversation.given() {
            if given.nick.eq_ignore_ascii_case(&self.me) {
                continue;
            }
            let Some((key, entry)) = keyed
                .iter()
                .find(|(_, entry)| mine(entry) && entry.id.as_deref() == Some(given.id.as_str()))
            else {
                continue;
            };
            let Some(said) = words(entry) else {
                continue;
            };
            // Who reacted, as their own line's head would show them.
            let them = Entry {
                from: given.nick.clone(),
                owner: None,
                time: given.at,
                id: None,
                kind: EntryKind::Message(String::new()),
                history: false,
                mention: false,
                sent_from: None,
                reply_to: None,
                formatted: None,
                edited: false,
                sealed: None,
            };
            hits.push(Hit {
                kind: ActivityKind::Reaction,
                key: key.clone(),
                who: them,
                said,
                emoji: Some(&given.emoji),
                at: given.at.timestamp_millis(),
                unread: conversation.read.is_some_and(|read| given.at > read),
            });
        }
        hits
    }
}

/// What `entry` says, when it's a message, an action, a notice or a zirc post.
fn words(entry: &Entry) -> Option<String> {
    match &entry.kind {
        EntryKind::Message(text) | EntryKind::Action(text) | EntryKind::Notice(text) => {
            Some(text.clone())
        }
        EntryKind::Zirc(message) => notice_text(&message.event),
        _ => None,
    }
}
