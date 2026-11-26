//! A direct message the server refused because whom it's to isn't connected, as a machine
//! whose zircd is stopped for an update (a machine's account isn't always-on, so the server
//! keeps nothing for it meanwhile): the send fails where the person is looking, and what
//! they wrote goes back in the conversation's composer to send again, with nothing sent by
//! itself (the user's choice, 25 Nov 2026: "Fail send, keep it in the composer, allow to
//! retry"). Decided here for every app: each takes the composer's text again from
//! [`Chat::draft_in`] when [`Chat::draft_restored_in`] moves.
//!
//! The server echoes each message it takes (`echo-message`), and answers one it can't
//! deliver with `ERR_NOSUCHNICK` instead, in the order they were sent. So what was said
//! directly waits here, oldest first, until its echoes or its refusal come.

use super::{Chat, Line, fold, is_channel, target};

/// The most messages kept waiting for their echo: past this, the oldest are forgotten.
const MOST: usize = 50;

/// What a conversation says when its message wasn't sent as `nick` isn't connected.
#[must_use]
pub fn not_connected(nick: &str) -> String {
    format!(
        "Not sent: {nick} isn't connected. Your message is back where you write, to send again."
    )
}

/// A direct message sent and not yet echoed.
#[derive(Debug, Clone)]
pub(super) struct Unechoed {
    /// Whom the server was asked to deliver it to, folded.
    nick: String,
    /// The conversation it was written in.
    conversation: String,
    /// What was written.
    text: String,
    /// What each message it went as says, whose echo or refusal is still to come.
    bodies: Vec<String>,
    /// Whether it went back to the composer already.
    refused: bool,
}

impl Chat {
    /// `text` was written in the conversation `name` and went as messages saying `bodies`:
    /// when that's directly to someone, on a server that echoes what it takes, it waits for
    /// their echoes.
    pub(super) fn awaiting_echo(&mut self, name: &str, text: &str, bodies: Vec<String>) {
        let echoes = self.link.as_ref().is_some_and(|link| link.echo);
        if is_channel(name) || !echoes || bodies.is_empty() {
            return;
        }
        self.unechoed.push(Unechoed {
            nick: fold(target(name)),
            conversation: name.to_owned(),
            text: text.to_owned(),
            bodies,
            refused: false,
        });
        if self.unechoed.len() > MOST {
            self.unechoed.remove(0);
        }
    }

    /// `line`, a message, is one of ours to someone coming back: it was delivered. Anything
    /// else said to them, such as a zirc event, waits for nothing.
    pub(super) fn echoed(&mut self, line: &Line) {
        let (Some(nick), Some(body)) = (line.params.first(), line.params.get(1)) else {
            return;
        };
        if self.unechoed.is_empty() || !super::nick_of(line).eq_ignore_ascii_case(&self.me) {
            return;
        }
        let folded = fold(nick);
        let found = self.unechoed.iter().enumerate().find_map(|(at, u)| {
            let of = u.bodies.iter().position(|b| b == body);
            (u.nick == folded)
                .then_some(of.map(|of| (at, of)))
                .flatten()
        });
        if let Some((at, of)) = found {
            self.unechoed[at].bodies.remove(of);
            if self.unechoed[at].bodies.is_empty() {
                self.unechoed.remove(at);
            }
        }
    }

    /// The server wouldn't deliver to `nick`, as it isn't connected: the oldest message
    /// still waiting for it goes back to its composer, and its conversation says why. Whether
    /// it was one of ours; a refusal of anything else is said as any error is.
    pub(super) fn refused_to(&mut self, nick: &str) -> bool {
        let folded = fold(nick);
        let Some(waiting) = self.unechoed.iter_mut().find(|u| u.nick == folded) else {
            return false;
        };
        let first = !std::mem::replace(&mut waiting.refused, true);
        let (conversation, text) = (waiting.conversation.clone(), waiting.text.clone());
        waiting.bodies.remove(0);
        if waiting.bodies.is_empty() {
            self.unechoed
                .retain(|u| !(u.nick == folded && u.bodies.is_empty()));
        }
        if first && let Some(at) = self.position(&conversation) {
            let draft = &self.conversations[at].draft;
            // The field may not have emptied for the send yet, and still say it.
            let back = if draft.trim().is_empty() || *draft == text {
                text
            } else {
                format!("{text}\n{draft}")
            };
            self.conversations[at].draft = back;
            self.conversations[at].draft_restored += 1;
            self.given_back.insert(fold(&conversation));
            self.info(at, not_connected(nick));
        }
        true
    }

    /// How many times the conversation `name`'s composer was given back what was written in
    /// it ([`Chat::draft_in`]): an app showing it takes the composer's text again whenever
    /// this moves. Zero for one there isn't.
    #[must_use]
    pub fn draft_restored_in(&self, name: &str) -> u64 {
        self.position(name)
            .map_or(0, |at| self.conversations[at].draft_restored)
    }
}
