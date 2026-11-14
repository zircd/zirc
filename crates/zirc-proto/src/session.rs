//! What an agent session is called, beyond its short handle: `x-zirc/session`, a
//! [`crate::Event::Custom`] whose data is a [`SessionCard`], sent by the session's machine
//! when the session is given a title (`zirc name --title`), and again when it changes. Apps
//! keep the latest card for each machine and session, and show its title when someone
//! points at the session's name, which stays a short IRC-safe handle (`desktop`, `ios`).
//!
//! A card never carries where the session can be opened: that link is the owner's alone,
//! kept on their machine and served only over its zircd's socket.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;

/// The custom event kind a session's card is sent as.
pub const KIND: &str = "x-zirc/session";

/// The longest title a card carries, in characters.
pub const MAX_TITLE: usize = 120;

/// What a session is called, as its machine says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields)]
pub struct SessionCard {
    /// The session's id, as its messages carry it.
    pub session: String,
    /// Its short handle ([`crate::Message::name`]), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its full title, as the agent tool shows it, at most [`MAX_TITLE`] characters.
    pub title: String,
    /// The agent tool it runs in ([`crate::Message::tool`]), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

impl SessionCard {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a card serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a session card serializes"),
            text: self.text(),
        }
    }

    /// The card `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// What it says, for plain IRC clients: `desktop is "Desktop: 3f review fixes"`.
    #[must_use]
    pub fn text(&self) -> String {
        let who = self.name.as_deref().unwrap_or(&self.session);
        format!("{who} is \"{}\"", self.title)
    }
}

/// The custom event kind a machine says which of its sessions are live in: [`Live`].
pub const LIVE_KIND: &str = "x-zirc/sessions";

/// How long a session stays live after its last `zirc watch` closed, in seconds: as long
/// as a `zirc watch --once` takes to be started again after it wakes, and then some.
pub const LIVE_GRACE_SECS: u64 = 180;

/// Which of a machine's agent sessions are live: those with a `zirc watch` open, or one
/// closed less than [`LIVE_GRACE_SECS`] ago. Its machine says it as a [`LIVE_KIND`]
/// event, only true now ([`crate::live`]), in its repositories' channels and the
/// organisation's, whenever it changes and to whoever joins. It replaces what the
/// machine said before: a session it no longer names has ended. Apps show a session as
/// live from its first watch, before it says anything, and fold away those that ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Live {
    /// The live sessions' ids, sorted.
    pub sessions: Vec<String>,
    /// The workflows' roles each live session joined (`zirc role join`), by session: what
    /// the server's workflows give a role's steps to ([`crate::workflow::Who::Role`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(as = "Option<BTreeMap<String, Vec<String>>>", optional)]
    pub roles: BTreeMap<String, Vec<String>>,
}

impl Live {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a list of strings serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: LIVE_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a live list serializes"),
            text: match self.sessions.len() {
                1 => "1 agent session live".to_owned(),
                n => format!("{n} agent sessions live"),
            },
        }
    }

    /// The list `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == LIVE_KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }
}

/// `title` as a card carries it: one line, trimmed, at most [`MAX_TITLE`] characters, cut
/// with `…`. `None` when nothing's left.
#[must_use]
pub fn title(title: &str) -> Option<String> {
    let line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    Some(crate::clip(&line, MAX_TITLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travels_as_a_custom_event_plain_clients_show() {
        let card = SessionCard {
            session: "cc-ab600723".into(),
            name: Some("desktop".into()),
            title: "Desktop: 3f review fixes".into(),
            tool: Some("claude-code".into()),
        };
        let event = card.event();
        assert_eq!(SessionCard::of(&event), Some(card));
        assert_eq!(
            event.human_text(),
            "desktop is \"Desktop: 3f review fixes\""
        );
        assert_eq!(SessionCard::of(&Event::Text { body: "hi".into() }), None);
    }

    #[test]
    fn a_live_list_is_only_true_now() {
        let live = Live {
            sessions: vec!["cc-1".into(), "cc-2".into()],
            roles: BTreeMap::new(),
        };
        let event = live.event();
        assert_eq!(Live::of(&event), Some(live.clone()));
        // Roles ride along, and a reader from before them takes the list as it was.
        let with_roles = Live {
            roles: BTreeMap::from([("cc-1".into(), vec!["fixers".into()])]),
            ..live
        };
        assert_eq!(Live::of(&with_roles.event()), Some(with_roles.clone()));
        let Event::Custom { data, .. } = with_roles.event() else {
            unreachable!("a live list is a custom event")
        };
        assert_eq!(data["roles"]["cc-1"][0], "fixers");
        assert!(crate::live(&event));
        assert_eq!(event.level(), Some(crate::Level::Debug));
        assert_eq!(Live::of(&Event::Text { body: "hi".into() }), None);
    }

    #[test]
    fn a_title_is_one_line_and_not_too_long() {
        assert_eq!(
            title("  Fix\nthe   login\tpage "),
            Some("Fix the login page".into())
        );
        assert_eq!(title(" \n "), None);
        let long = "x".repeat(MAX_TITLE + 10);
        let cut = title(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_TITLE);
        assert!(cut.ends_with('…'));
    }
}
