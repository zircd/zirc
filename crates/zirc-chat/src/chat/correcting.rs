//! Correcting a message of ours: an ordinary message carrying [`EDIT`], the id of the one it
//! corrects. zirc's apps show its words in place of the old ones, marked [`EDITED`], keeping
//! the old one's place, reactions and replies; plain IRC clients show it as an ordinary
//! line. Only its author can correct a message, and only for [`EDIT_WINDOW`] after saying
//! it: every app reads both from here, and a correction that breaks either is an ordinary
//! message.

use chrono::{DateTime, Utc};

use super::{
    Chat, EDIT, EDIT_WINDOW, Entry, EntryKind, Line, formatted, sending::tag_opening,
    strip_formatting,
};

/// A message's menu: corrects it.
pub const EDIT_LABEL: &str = "Edit";

/// After a corrected message's words, muted: `(edited)`.
pub const EDITED: &str = "(edited)";

/// What the composer says while it corrects a message, over its old words.
pub const EDITING: &str = "Editing";

/// What stops correcting a message, for a screen reader, beside [`EDITING`].
pub const STOP_EDITING: &str = "Stop editing";

/// Why a correction wasn't sent: the message is gone or too old.
pub const TOO_LATE_TO_EDIT: &str = "Messages can be edited for 20 minutes after they're sent.";

/// Whether `entry`, said by `from`, can be corrected by `me` at `now`: our own message or
/// action, with the server's id, said less than [`EDIT_WINDOW`] ago.
#[must_use]
pub fn editable(entry: &Entry, me: &str, now: DateTime<Utc>) -> bool {
    entry.id.is_some()
        && !me.is_empty()
        && entry.from.eq_ignore_ascii_case(me)
        && matches!(entry.kind, EntryKind::Message(_) | EntryKind::Action(_))
        && now - entry.time < EDIT_WINDOW
}

impl Chat {
    /// Whether message `id` in `target` is ours to correct now ([`editable`]).
    #[must_use]
    pub fn can_edit(&self, target: &str, id: &str) -> bool {
        self.conversation(target)
            .and_then(|c| c.entry(id))
            .is_some_and(|entry| editable(entry, &self.me, (self.clock)()))
    }

    /// The words message `id` in `target` says, to put in the composer to correct it, while
    /// it's ours to correct ([`Chat::can_edit`]): with IRC's formatting codes, as typed.
    #[must_use]
    pub fn edit_text(&self, target: &str, id: &str) -> Option<String> {
        let entry = self.conversation(target)?.entry(id)?;
        if !editable(entry, &self.me, (self.clock)()) {
            return None;
        }
        entry.formatted.clone().or_else(|| match &entry.kind {
            EntryKind::Message(text) | EntryKind::Action(text) => Some(text.clone()),
            _ => None,
        })
    }

    /// Says `text` in `target` in place of our message `id`: an ordinary message (an action,
    /// if it was one) carrying [`EDIT`]. Nothing, and a note saying why, once it isn't ours
    /// to correct ([`Chat::can_edit`]); the server's echo, or without one this, puts the
    /// words in its place.
    pub fn say_edit(&mut self, target: &str, id: &str, text: &str) -> Vec<Line> {
        let action = self
            .conversation(target)
            .and_then(|c| c.entry(id))
            .is_some_and(|entry| matches!(entry.kind, EntryKind::Action(_)));
        if !self.can_edit(target, id) {
            self.note(TOO_LATE_TO_EDIT.to_owned());
            return Vec::new();
        }
        let mut lines = self.said(target, text, action);
        tag_opening(&mut lines, EDIT, id);
        if !self.link.as_ref().is_some_and(|link| link.echo) {
            let words = strip_formatting(text);
            let kind = if action {
                EntryKind::Action(words)
            } else {
                EntryKind::Message(words)
            };
            let (me, now) = (self.me.clone(), (self.clock)());
            if let Some(at) = self.position(target) {
                self.conversations[at].edit(id, None, &me, now, kind, formatted(text));
            }
        }
        lines
    }

    /// Puts `entry`, from `line` in conversation `name`, in the place of the message its
    /// [`EDIT`] tag names, when it has one and its author may ([`super::Conversation::edit`]).
    /// Whether it did; when it didn't, it's an ordinary message.
    pub(super) fn corrected(&mut self, line: &Line, name: &str, entry: &Entry) -> bool {
        let Some(id) = line.tag(EDIT).filter(|id| !id.is_empty()) else {
            return false;
        };
        let Some(at) = self.position(name) else {
            return false;
        };
        self.conversations[at].edit(
            id,
            entry.id.as_deref(),
            &entry.from,
            entry.time,
            entry.kind.clone(),
            entry.formatted.clone(),
        )
    }
}
