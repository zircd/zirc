//! Sealed parts of messages in the chat ([`crate::sealed`]): the organisation's keys this
//! device holds, which the app hands over ([`Chat::set_keys`]), what each part of a message
//! is here ([`Chat::sealed_parts`]), which the person revealed, and sealing what they type
//! between `||` markers.
//!
//! Every app draws a part from [`SealedPart`] and says the same words: a hidden part is
//! `🔒 sealed`, never its value, until the person reveals it, and it hides again once they
//! look away ([`Chat::hide_sealed`]).

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;
use zirc_proto::sealed::{ASK_TAG, Ask, CLIENT_TAGS_MAX, GRANT_TAG, PLACEHOLDER, Sealed, TAG};

use super::sending::tag_opening;
use super::{
    CLIENT_TAG, Chat, Entry, EntryKind, Line, REPLY, RichBlock, RichSpan, RichSpanKind, agent,
    formatted, kind_of,
};
use crate::sealed::{
    Context, DeviceKey, Key, NONCE_LEN, Opened, SealError, grant, open_grant, open_message,
    seal_message, split,
};

/// What stands before a sealed part, in every app.
pub const LOCK: &str = "🔒";

/// What a hidden part says.
pub const SEALED: &str = "sealed";

/// What shows a hidden part's value.
pub const REVEAL: &str = "Reveal";

/// The composer's lock, which puts the selection between `||` markers to seal it.
pub const SEAL: &str = "Seal (||…||)";

/// The Settings page of the organisation's keys ([`super::SettingsKind::Sealed`]).
pub const SEALED_MESSAGES: &str = "Sealed messages";

/// What the page says sealed parts are.
pub const SEALED_EXPLAINED: &str = "A part of a message between || markers goes sealed with \
     your organisation's key: only devices holding it can read it, never the server. Get the \
     key from a teammate in person or on a call, never in a message, and paste it here.";

/// The page's field for a key.
pub const PASTE_KEY: &str = "Paste a key";

/// The page's button that adds it.
pub const ADD_KEY: &str = "Add key";

/// The heading over the keys this device holds.
pub const KEYS_HERE: &str = "Keys on this device";

/// What the page says with no key yet.
pub const NO_KEYS_HERE: &str =
    "None yet: sealed parts show locked here, and this device can't send them";

/// What marks the key that seals: the newest.
pub const SEALS_WITH: &str = "seals";

/// What a device says once it's handed the organisation's key, by a teammate's.
pub const KEY_RECEIVED: &str = "Sealed parts open here now: a teammate's device handed this one \
     the organisation's key";

/// How long a device waits before answering the same device's ask again.
const ANSWER_AGAIN_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(1);

/// The menu item that copies a sealed part's value, numbered when a message has several.
pub const COPY_SEALED: &str = "Copy sealed part";

/// What's said once a key is added, by its organisation and id.
#[must_use]
pub fn key_added(org: &str, id: &str) -> String {
    format!("Added {org}'s key {id}: sealed parts open here, and this device seals with it")
}

/// The menu item for copying part `number` of a message with `parts`.
#[must_use]
pub fn copy_sealed_label(number: u32, parts: usize) -> String {
    if parts > 1 {
        format!("{COPY_SEALED} {number}")
    } else {
        COPY_SEALED.to_owned()
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
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

/// A sealed part as a span of a line's words carries it ([`RichSpan::sealed`]): which
/// message and part it is, to reveal or copy, and what it is here. The span's text is what
/// it shows after the [`LOCK`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SealedSpan {
    /// The message's id, which [`Chat::toggle_sealed_in`] and [`Chat::sealed_value`] take.
    pub message: String,
    /// Which part, from 1.
    pub number: u32,
    /// What it is here: only a hidden or revealed part offers Reveal (or Hide) and Copy.
    pub state: SealedState,
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
    /// This device's key pair for being handed the key, which the app keeps; none in an app
    /// that isn't handed keys.
    device: Option<DeviceKey>,
    /// Whether an ask is out, for an answer to be taken.
    asking: bool,
    /// What's been asked for this connection, by key id (empty for any), so it's asked once.
    asked: HashSet<String>,
    /// Keys handed over, for the app to keep ([`Chat::take_received_keys`]).
    received: Vec<Key>,
    /// When each asking device, by its public key, was last answered.
    answered: HashMap<String, DateTime<Utc>>,
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

    /// This device's key pair for being handed the organisation's key, which the app keeps
    /// in its secret store: with it, the chat asks for the key in the organisation's channel
    /// when it has none, or lacks one a part was sealed with, and takes the answer.
    /// Answers the ask to send, when it's already in the channel and has no key.
    pub fn set_device(&mut self, device: DeviceKey) -> Vec<Line> {
        self.sealing.device = Some(device);
        self.ask_for_key(None)
    }

    /// The keys a teammate's device handed this one since last asked, for the app to keep
    /// as its own, the newest first.
    pub fn take_received_keys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.sealing.received)
    }

    /// The `TAGMSG` asking the organisation's channel for its newest key, or the one with id
    /// `id`, as this device lacks it; none when it has it, has asked already, has no device
    /// key or isn't in the channel.
    pub(super) fn ask_for_key(&mut self, id: Option<&str>) -> Vec<Line> {
        let Some(device) = &self.sealing.device else {
            return Vec::new();
        };
        let joined = self
            .conversation(super::ORG_CHANNEL)
            .is_some_and(|conversation| conversation.joined);
        let has = match id {
            Some(id) => self.sealing.keys.iter().any(|key| key.id() == id),
            None => !self.sealing.keys.is_empty(),
        };
        if !joined || has || !self.sealing.asked.insert(id.unwrap_or_default().to_owned()) {
            return Vec::new();
        }
        let ask = Ask {
            p: device.public(),
            k: id.map(str::to_owned),
        };
        let Ok(value) = serde_json::to_string(&ask) else {
            return Vec::new();
        };
        self.sealing.asking = true;
        let mut line = Line::new("TAGMSG", [super::ORG_CHANNEL]);
        line.tags = vec![(ASK_TAG.to_owned(), Some(value))];
        vec![line]
    }

    /// The ask for the key a received message's sealed parts need, if this device lacks it.
    pub(super) fn ask_for_sealed(&mut self, line: &Line) -> Vec<Line> {
        let needed = line
            .tag(TAG)
            .and_then(Sealed::from_tag)
            .and_then(|sealed| sealed.parts.into_iter().next())
            .map(|part| part.k);
        match needed {
            Some(id) => self.ask_for_key(Some(&id)),
            None => Vec::new(),
        }
    }

    /// A `TAGMSG`'s ask or answer for the key, handled: what to send, or `None` when it's
    /// neither.
    pub(super) fn sealing_tag(&mut self, line: &Line) -> Option<Vec<Line>> {
        if let Some(value) = line.tag(ASK_TAG) {
            return Some(self.answer_ask(line, value));
        }
        let value = line.tag(GRANT_TAG)?;
        self.take_grant(line, value);
        Some(Vec::new())
    }

    /// Hands the asker the newest key, encrypted to its device: only a member's own device,
    /// signed in to an account, never a guest or a machine, and only that key.
    fn answer_ask(&mut self, line: &Line, value: &str) -> Vec<Line> {
        let from = super::nick_of(line);
        let target = line.params.first().map_or("", String::as_str);
        let Some(key) = self.sealing.keys.first().cloned() else {
            return Vec::new();
        };
        let Ok(ask) = serde_json::from_str::<Ask>(value) else {
            return Vec::new();
        };
        let member = line
            .tag("account")
            .is_some_and(|account| !account.is_empty() && account != "*")
            && !self.is_guest(from)
            && super::owner_by_host(line).is_none();
        let now = (self.clock)();
        let lately = self
            .sealing
            .answered
            .get(&ask.p)
            .is_some_and(|at| now - *at < ANSWER_AGAIN_AFTER);
        if from.eq_ignore_ascii_case(&self.me)
            || !target.eq_ignore_ascii_case(super::ORG_CHANNEL)
            || !member
            || lately
            || ask.k.as_ref().is_some_and(|id| *id != key.id())
        {
            return Vec::new();
        }
        // A one-off key's bytes are rarely not a key: a few tries.
        let random = self.random;
        let made = (0..4).find_map(|_| {
            let (mut ephemeral, mut nonce) = ([0; 32], [0; NONCE_LEN]);
            (random(&mut ephemeral) && random(&mut nonce))
                .then(|| grant(&key, &ask.p, ephemeral, nonce))
                .flatten()
        });
        let Some(value) = made else {
            return Vec::new();
        };
        self.sealing.answered.insert(ask.p, now);
        let mut line = Line::new("TAGMSG", [from]);
        line.tags = vec![(GRANT_TAG.to_owned(), Some(value))];
        vec![line]
    }

    /// Keeps the key an answer to our own ask hands this device, saying so once.
    fn take_grant(&mut self, line: &Line, value: &str) {
        let to_me = line
            .params
            .first()
            .is_some_and(|target| target.eq_ignore_ascii_case(&self.me));
        let Some(device) = self
            .sealing
            .device
            .as_ref()
            .filter(|_| to_me && self.sealing.asking)
        else {
            return;
        };
        let Some(key) = open_grant(device, value) else {
            return;
        };
        if self.sealing.keys.contains(&key) {
            return;
        }
        self.sealing.asking = false;
        self.sealing.keys.insert(0, key.clone());
        self.sealing.received.push(key);
        self.note(KEY_RECEIVED.to_owned());
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

    /// [`Chat::toggle_sealed`] for the message `id`: its chip was clicked or tapped.
    pub fn toggle_sealed_in(&mut self, id: &str) {
        if let Some(entry) = self.sealed_entry(id) {
            self.toggle_sealed(&entry);
        }
    }

    /// The value of part `number` of the message `id`, for its menu's Copy, whether revealed
    /// or not: `None` when it doesn't open here.
    #[must_use]
    pub fn sealed_value(&self, id: &str, number: u32) -> Option<String> {
        let entry = self.sealed_entry(id)?;
        let tag = entry.sealed.as_ref()?;
        let at = usize::try_from(number).ok()?.checked_sub(1)?;
        let opened = open_message(
            &self.sealing.keys,
            &Context {
                target: &tag.target,
            },
            &tag.value,
        )?;
        match opened.into_iter().nth(at)? {
            Opened::Value(value) => Some(value),
            Opened::NoKey | Opened::Failed => None,
        }
    }

    /// The message `id`, with sealed parts, in whichever conversation holds it.
    fn sealed_entry(&self, id: &str) -> Option<Entry> {
        self.conversations
            .iter()
            .flat_map(|conversation| &conversation.entries)
            .find(|entry| entry.id.as_deref() == Some(id) && entry.sealed.is_some())
            .cloned()
    }

    /// The parts of the message `id` that open here, by number, for its menu's Copy.
    #[must_use]
    pub fn sealed_numbers(&self, id: &str) -> Vec<u32> {
        self.sealed_entry(id)
            .map(|entry| self.sealed_parts(&entry))
            .unwrap_or_default()
            .into_iter()
            .filter(|part| matches!(part.state, SealedState::Hidden | SealedState::Revealed))
            .filter_map(|part| u32::try_from(part.number).ok())
            .collect()
    }

    /// `rich`, `entry`'s words as spans, with each `[sealed]` its parts stand for a chip
    /// ([`RichSpan::sealed`]).
    pub(super) fn seal_spans(&self, entry: &Entry, rich: &mut [RichBlock]) {
        let parts = self.sealed_parts(entry);
        let message = entry.id.clone().unwrap_or_default();
        let mut parts = parts.into_iter();
        RichBlock::each_run_mut(rich, &mut |spans| {
            let mut sealed = Vec::with_capacity(spans.len());
            for span in spans.drain(..) {
                if span.kind != RichSpanKind::Text || !span.text.contains(PLACEHOLDER) {
                    sealed.push(span);
                    continue;
                }
                let mut rest = span.text.as_str();
                while let Some(at) = rest.find(PLACEHOLDER) {
                    let Some(part) = parts.next() else {
                        break;
                    };
                    if at > 0 {
                        sealed.push(RichSpan {
                            text: rest[..at].to_owned(),
                            ..span.clone()
                        });
                    }
                    sealed.push(RichSpan {
                        text: format!("{LOCK} {}", part.text),
                        sealed: Some(SealedSpan {
                            message: message.clone(),
                            number: u32::try_from(part.number).unwrap_or(u32::MAX),
                            state: part.state,
                        }),
                        ..span.clone()
                    });
                    rest = &rest[at + PLACEHOLDER.len()..];
                }
                if !rest.is_empty() {
                    sealed.push(RichSpan {
                        text: rest.to_owned(),
                        ..span.clone()
                    });
                }
            }
            *spans = sealed;
        });
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
