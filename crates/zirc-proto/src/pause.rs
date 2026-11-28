//! Pausing an agent session from an app: `x-zirc/pause`, a [`crate::Event::Custom`] whose
//! data is a [`Pause`], sent by the session's own person to the nick of the machine it works
//! on (the user's choice, 28 Nov 2026: zirc leans on watching what agents do and stepping in,
//! and every working row on the Office has Message, Pause and Open). Only the machine's owner
//! pauses its sessions: its zircd takes the event from no one else, as it takes direct
//! messages ([`crate::Event::Text`]) from no one else.
//!
//! zircd tells the session through its `zirc watch`, whose hint says to finish or undo the
//! step in hand, say in one line where it stopped, and wait with the watch running. While
//! it's paused, zircd wakes it only for its person's own words: what else concerns it is held
//! and told once the pause ends, with the person's Resume or their next message to it (the
//! user's choices, 28 Nov 2026). The machine says which of its sessions are paused in its
//! live list ([`crate::session::Live::paused`]), so every app's row reads [`paused_by`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;

/// The custom event kind a pause or a resume is sent as.
pub const KIND: &str = "x-zirc/pause";

/// A session paused, or resumed, by its person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Pause {
    /// The session on the machine it's sent to: its id, or the name it goes by.
    pub session: String,
    /// Whether it pauses the session; `false` resumes it.
    pub paused: bool,
}

impl Pause {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a pause serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a pause serializes"),
            text: self.text(),
        }
    }

    /// The pause `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// What it says, for plain IRC clients: `Pause cc-1: stop at a safe point and wait`,
    /// or `Resume cc-1`.
    #[must_use]
    pub fn text(&self) -> String {
        if self.paused {
            format!("Pause {}: stop at a safe point and wait", self.session)
        } else {
            format!("Resume {}", self.session)
        }
    }
}

/// What a paused session's row says in every app, in place of what it's doing: `Paused by
/// you` to its own person, `Paused by Henrik` to everyone else. `by` is who paused it as the
/// reader calls them.
#[must_use]
pub fn paused_by(by: &str) -> String {
    format!("Paused by {by}")
}

/// What `zirc watch` prints when the session's person pauses it: `paused by mattias`, under
/// which its hint says what to do.
#[must_use]
pub fn paused_line(by: &str) -> String {
    format!("paused by {by}: stop at a safe point and wait")
}

/// The same when they resume it.
#[must_use]
pub fn resumed_line(by: &str) -> String {
    format!("resumed by {by}: carry on where you stopped")
}

/// What heads the lines zircd held while the session was paused, told once the pause ends:
/// `cc-1: while you were paused, these 2 concerned you:`.
#[must_use]
pub fn held_heading(session: &str, count: usize) -> String {
    let these = match count {
        1 => "this".to_owned(),
        n => format!("these {n}"),
    };
    format!("{session}: while you were paused, {these} concerned you:")
}

/// What follows them.
#[must_use]
pub fn held_end(session: &str) -> String {
    format!("{session}: that's what came while you were paused")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_goes_as_its_event_and_comes_back() {
        let pause = Pause {
            session: "cc-1".into(),
            paused: true,
        };
        assert_eq!(Pause::of(&pause.event()), Some(pause.clone()));
        assert_eq!(pause.text(), "Pause cc-1: stop at a safe point and wait");
        let resume = Pause {
            paused: false,
            ..pause
        };
        assert_eq!(resume.text(), "Resume cc-1");
        assert_eq!(Pause::of(&Event::Text { body: "hi".into() }), None);
    }

    #[test]
    fn its_words() {
        assert_eq!(paused_by("you"), "Paused by you");
        assert_eq!(
            held_heading("cc-1", 1),
            "cc-1: while you were paused, this concerned you:"
        );
        assert_eq!(
            held_heading("cc-1", 3),
            "cc-1: while you were paused, these 3 concerned you:"
        );
    }
}
