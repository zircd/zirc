//! Sealed parts of messages in the chat ([`crate::sealed`]): the organisation's keys this
//! device holds, which the app hands over ([`Chat::set_keys`]), what each part of a message
//! is here ([`Chat::sealed_parts`]), which the person revealed, and sealing what they type
//! between `||` markers.
//!
//! Every app draws a part from [`SealedPart`] and says the same words: a hidden part is
//! `🔒 sealed`, never its value, until the person reveals it, and it hides again once they
//! look away ([`Chat::hide_sealed`]).

use std::cell::Cell;
use std::collections::HashSet;

use zirc_proto::sealed::{CLIENT_TAGS_MAX, PLACEHOLDER, TAG};

use super::sending::tag_opening;
use super::{CLIENT_TAG, Chat, Entry, EntryKind, Line, REPLY, agent, formatted, kind_of};
use crate::sealed::{
    Context, Key, NONCE_LEN, Opened, SealError, open_message, seal_message, split,
};

/// What stands before a sealed part, in every app.
pub const LOCK: &str = "🔒";

/// What a hidden part says.
pub const SEALED: &str = "sealed";

/// What a part says on a device without the key it was sealed with.
pub const SEALED_NO_KEY: &str = "sealed, and this device has no key for it";

/// What a part says that doesn't open with the key it names: changed, or moved.
pub const SEALED_FAILED: &str = "couldn't be opened";

/// Why a message with sealed parts isn't sent from a device without a key: never as text.
pub const NO_KEY_TO_SEAL: &str = "not sent: this device has no key to seal || parts with. Add \
     your organisation's key from a teammate, or run zirc team-key add";

/// Why sealed parts aren't sent but in a new message or a reply.
pub const SEALS_ONLY_SAYING: &str =
    "not sent: || parts are sealed only in a new message or a reply";

/// Why a message with sealed parts too long for one message isn't sent: its parts would be
/// apart from their places.
pub const SEALS_IN_ONE: &str =
    "not sent: a message with sealed parts goes as one, and this is too long for that";

/// Why an app without randomness doesn't seal.
pub const CANT_SEAL_HERE: &str = "not sent: this app can't seal || parts yet";

/// A message's sealed parts as they came: the `+zirc.dev/sealed` tag's value, and where it
/// was sent, which they're bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedTag {
    /// The tag's value.
    pub value: String,
    /// The channel or nick its PRIVMSG was sent to.
    pub target: String,
}

impl SealedTag {
    /// The sealed parts `line`, a PRIVMSG, came with, if any.
    pub(super) fn of(line: &Line) -> Option<Self> {
        Some(Self {
            value: line.tag(TAG)?.to_owned(),
            target: line.params.first()?.clone(),
        })
    }
}

/// What a sealed part is on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedState {
    /// It opens here, and shows as [`SEALED`] until revealed.
    Hidden,
    /// Revealed: it shows its value.
    Revealed,
    /// This device holds no key with the id it names.
    NoKey,
    /// It doesn't open with the key it names: changed, or moved.
    Failed,
}

/// One sealed part of a message, as every app draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPart {
    /// Which part, from 1, as `zirc unseal` counts them.
    pub number: usize,
    /// What it is here.
    pub state: SealedState,
    /// What it shows after the lock: its value when revealed, else words saying what it is.
    pub text: String,
}

/// The keys the app handed over, and the parts the person revealed, by message id and
/// number.
#[derive(Debug, Default)]
pub(super) struct Sealing {
    keys: Vec<Key>,
    revealed: HashSet<(String, usize)>,
}

/// Randomness for an app that has none to give: none, so it doesn't seal.
#[must_use]
pub fn no_random(_: &mut [u8]) -> bool {
    false
}

/// Whether `text` has parts to seal between `||` markers.
#[must_use]
pub fn seals(text: &str) -> bool {
    !split(text).parts.is_empty()
}

impl Chat {
    /// The organisation's keys this device holds, the newest first, which seals: as the app
    /// keeps them, in the system's secret store.
    pub fn set_keys(&mut self, keys: Vec<Key>) {
        self.sealing.keys = keys;
    }

    /// Whether this device holds a key to seal with.
    #[must_use]
    pub fn can_seal(&self) -> bool {
        !self.sealing.keys.is_empty()
    }

    /// What each of `entry`'s sealed parts is here, in order: none for a message without.
    #[must_use]
    pub fn sealed_parts(&self, entry: &Entry) -> Vec<SealedPart> {
        let Some(tag) = &entry.sealed else {
            return Vec::new();
        };
        let context = Context {
            target: &tag.target,
        };
        let id = entry.id.as_deref().unwrap_or_default();
        open_message(&self.sealing.keys, &context, &tag.value)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(at, opened)| {
                let number = at + 1;
                let revealed = self.sealing.revealed.contains(&(id.to_owned(), number));
                let (state, text) = match opened {
                    Opened::Value(value) if revealed => (SealedState::Revealed, value),
                    Opened::Value(_) => (SealedState::Hidden, SEALED.to_owned()),
                    Opened::NoKey => (SealedState::NoKey, SEALED_NO_KEY.to_owned()),
                    Opened::Failed => (SealedState::Failed, SEALED_FAILED.to_owned()),
                };
                SealedPart {
                    number,
                    state,
                    text,
                }
            })
            .collect()
    }

    /// `text`, `entry`'s, with each `[sealed]` its parts stand for drawn as its
    /// [`SealedPart`] is, `🔒 sealed` or `🔒 <value>`: for an app that draws text, as the
    /// terminal does, and for copying what's shown.
    #[must_use]
    pub fn sealed_text(&self, entry: &Entry, text: &str) -> String {
        let parts = self.sealed_parts(entry);
        if parts.is_empty() {
            return text.to_owned();
        }
        let mut shown = String::with_capacity(text.len());
        let mut rest = text;
        for part in &parts {
            let Some(at) = rest.find(PLACEHOLDER) else {
                break;
            };
            shown.push_str(&rest[..at]);
            shown.push_str(LOCK);
            shown.push(' ');
            shown.push_str(&part.text);
            rest = &rest[at + PLACEHOLDER.len()..];
        }
        shown.push_str(rest);
        shown
    }

    /// Reveals or hides `entry`'s sealed parts that open here, all at once: the person asked.
    pub fn toggle_sealed(&mut self, entry: &Entry) {
        let Some(id) = entry.id.clone() else {
            return;
        };
        let opening: Vec<usize> = self
            .sealed_parts(entry)
            .iter()
            .filter(|part| matches!(part.state, SealedState::Hidden | SealedState::Revealed))
            .map(|part| part.number)
            .collect();
        let shown = opening
            .iter()
            .any(|&number| self.sealing.revealed.contains(&(id.clone(), number)));
        for number in opening {
            if shown {
                self.sealing.revealed.remove(&(id.clone(), number));
            } else {
                self.sealing.revealed.insert((id.clone(), number));
            }
        }
    }

    /// Hides every part revealed: the person looked away, or picked another message.
    pub fn hide_sealed(&mut self) {
        self.sealing.revealed.clear();
    }

    /// The lines that say `text` to `target`, its `||…||` parts sealed with the newest key,
    /// replying to `to` if it does; why not, when they can't go. Nothing goes unsealed.
    pub(super) fn say_sealed(
        &mut self,
        target: &str,
        text: &str,
        to: Option<&str>,
    ) -> Result<Vec<Line>, &'static str> {
        if agent(target).is_some() {
            return Err(SEALS_ONLY_SAYING);
        }
        let key = self.sealing.keys.first().cloned().ok_or(NO_KEY_TO_SEAL)?;
        let split = split(text);
        if self.outgoing(&split.text, false).len() != 1 {
            return Err(SEALS_IN_ONE);
        }
        let random = self.random;
        let given = Cell::new(true);
        let nonce = || {
            let mut nonce = [0; NONCE_LEN];
            given.set(given.get() && random(&mut nonce));
            nonce
        };
        let value = seal_message(
            &key,
            &Context { target },
            &split.parts,
            nonce,
            self.room_for_sealed(to),
        );
        if !given.get() {
            return Err(CANT_SEAL_HERE);
        }
        let value = value.map_err(|SealError::TooLong| SEALS_IN_ONE)?;
        self.sent(target);
        let mut lines = self.said(target, &split.text, false);
        tag_opening(&mut lines, TAG, &value);
        if let Some(to) = to {
            tag_opening(&mut lines, REPLY, to);
        }
        if !self.link.as_ref().is_some_and(|link| link.echo) {
            let at = self.ensure(target, kind_of(target));
            let entry = Entry {
                from: self.me.clone(),
                kind: EntryKind::Message(super::strip_formatting(&split.text)),
                sent_from: self.options.client.clone(),
                reply_to: to.map(str::to_owned),
                formatted: formatted(&split.text),
                sealed: Some(SealedTag {
                    value,
                    target: target.to_owned(),
                }),
                ..Entry::info((self.clock)(), String::new())
            };
            self.conversations[at].insert(entry, self.options.max_entries);
        }
        Ok(lines)
    }

    /// What's left of a message's client tags for its sealed parts, after the app's name
    /// and a reply's id, and the `;` between them.
    fn room_for_sealed(&self, to: Option<&str>) -> usize {
        let taken = |name: &str, value: &str| name.len() + 1 + value.len() + 1;
        let client = self
            .options
            .client
            .as_deref()
            .map_or(0, |client| taken(CLIENT_TAG, client));
        let reply = to.map_or(0, |to| taken(REPLY, to));
        CLIENT_TAGS_MAX.saturating_sub(client + reply)
    }
}

#[cfg(test)]
mod tests;
