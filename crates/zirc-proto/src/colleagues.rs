//! What an organisation lets its members' agents do with its other members and their agents,
//! as its owners and admins set it on zirc.dev's Server tab
//! ([`crate::desired::Settings::colleague_queries`] and
//! [`crate::desired::Settings::colleague_answers`]), each allowed unless they turn it off.
//!
//! zirc.dev gives it to the server's sidecar, which says it in [`crate::state::CHANNEL`] as an
//! event of [`KIND`] from zirc's own account ([`crate::names::SYSTEM`]), which no person or
//! machine can hold. Each machine's zircd takes the newest it heard from that account, and
//! allows both when it heard none.

use serde::{Deserialize, Serialize};

use crate::Event;
use crate::desired::Settings;

/// The kind of event that says it.
pub const KIND: &str = "x-zirc/colleagues";

/// What members' agents may do with other members and their agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Colleagues {
    /// Whether a member may let other members' agents query their machine's agents
    /// (`zirc ask`, with the machine's own setting at `anyone`). Off, a machine takes
    /// queries only from its owner's own, whatever it's set to.
    #[serde(default = "allowed")]
    pub queries: bool,
    /// Whether a member an agent wrote to (`zirc dm --to`) may answer it: their reply reaches
    /// that session, as a colleague's word, never its own person's. Off, only its person's do.
    #[serde(default = "allowed")]
    pub answers: bool,
}

fn allowed() -> bool {
    true
}

impl Default for Colleagues {
    fn default() -> Self {
        Self {
            queries: true,
            answers: true,
        }
    }
}

impl Colleagues {
    /// As a server's settings have it.
    #[must_use]
    pub fn of_settings(settings: &Settings) -> Self {
        Self {
            queries: settings.colleague_queries.unwrap_or(true),
            answers: settings.colleague_answers.unwrap_or(true),
        }
    }

    /// The event that says it, with a line for people reading the channel.
    #[must_use]
    pub fn event(self) -> Event {
        let say = |allowed: bool| if allowed { "may" } else { "may not" };
        Event::Custom {
            kind: KIND.into(),
            data: serde_json::to_value(self).unwrap_or_default(),
            // Short, so the line fits beside its payload.
            text: format!(
                "agents {} ask colleagues' agents; colleagues {} answer agents",
                say(self.queries),
                say(self.answers)
            ),
        }
    }

    /// What `event` says, if it's one of [`KIND`]. Whose word it is, the caller checks.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_and_reads_itself_back() {
        let off = Colleagues {
            queries: false,
            answers: true,
        };
        assert_eq!(Colleagues::of(&off.event()), Some(off));
        assert_eq!(Colleagues::of(&Event::custom("x-fjord/deploy", "hi")), None);
    }

    #[test]
    fn allows_both_unless_settings_turn_one_off() {
        assert_eq!(
            Colleagues::of_settings(&Settings::default()),
            Colleagues::default()
        );
        let settings = Settings {
            colleague_answers: Some(false),
            ..Settings::default()
        };
        assert!(!Colleagues::of_settings(&settings).answers);
        assert!(Colleagues::of_settings(&settings).queries);
    }
}
