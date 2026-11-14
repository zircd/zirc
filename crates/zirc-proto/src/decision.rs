//! A decision a person made, recorded where every session can find it: `x-zirc/decision`,
//! a [`crate::Event::Custom`] whose data is a [`Decision`]. An agent session posts one with
//! `zirc decide` when its user decides something in the session, quoting them; a person
//! posts their own from their account (`/decide` in the TUI), or from a terminal on their
//! machine. Who really said it is never the event's word but the server's: readers judge
//! that by the account a decision was sent from (zirc-core's `decisions`).
//!
//! It's a custom event, so readers that predate it, and plain IRC clients, show its text.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Event;

/// The custom event kind a decision is sent as.
pub const KIND: &str = "x-zirc/decision";

/// A decision: who made it, in their own words, and what it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Decision {
    /// Who decided: a person's handle.
    pub by: String,
    /// Their words, verbatim.
    pub quote: String,
    /// What it covers, in a few words, to look it up by: `cloud spend`, `the release`.
    pub about: String,
    /// The agent session it was said to, which relays it; none when the person posted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Where to read more: the conversation, a document, an issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The sessions and branches it's for, whose watches it wakes besides the relaying
    /// session's, the ones it names, and the ones that joined `#ops`.
    #[serde(rename = "for", default, skip_serializing_if = "Vec::is_empty")]
    pub for_: Vec<String>,
    /// The server's id of the earlier decision this one takes the place of, which then
    /// decides nothing more: readers show only the newest of a chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

impl Decision {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a decision serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a decision serializes"),
            text: self.text(),
        }
    }

    /// The decision `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// What it says, for people, as Markdown (the house style, `zirc agent-guide`): a bold
    /// title with what it covers, the person's words as a block quote, then bullets for who
    /// decided and what it concerns. Apps draw it as it is, and a plain IRC client shows
    /// the same lines. It's the sender's word only: readers that can tell who sent it say
    /// so instead.
    ///
    /// ```text
    /// **Decision:** cloud spend
    /// > no new paid infra without asking me
    ///
    /// - decided by `mattias`, relayed by `cc-1`
    /// - for `login`
    /// ```
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = format!("**Decision:** {}\n", self.about.replace('\n', " "));
        for line in self.quote.lines() {
            let _ = writeln!(text, "> {line}");
        }
        text.push('\n');
        let by = format!("- decided by `{}`", self.by);
        match &self.session {
            Some(session) => {
                let _ = write!(text, "{by}, relayed by `{session}`");
            }
            None => text.push_str(&by),
        }
        if !self.for_.is_empty() {
            let for_: Vec<String> = self.for_.iter().map(|f| format!("`{f}`")).collect();
            let _ = write!(text, "\n- for {}", for_.join(", "));
        }
        if let Some(link) = &self.link {
            let _ = write!(text, "\n- {link}");
        }
        if self.replaces.is_some() {
            text.push_str("\n- in place of an earlier decision");
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travels_as_a_custom_event_any_reader_shows() {
        let decision = Decision {
            by: "mattias".into(),
            quote: "no new paid infra without asking me".into(),
            about: "cloud spend".into(),
            session: Some("cc-1".into()),
            link: None,
            for_: vec!["login".into()],
            replaces: None,
        };
        let event = decision.event();
        assert!(crate::check_kind(KIND).is_none());
        assert_eq!(
            event.human_text(),
            "**Decision:** cloud spend\n> no new paid infra without asking me\n\n\
             - decided by `mattias`, relayed by `cc-1`\n- for `login`"
        );
        assert_eq!(Decision::of(&event), Some(decision.clone()));
        let again = Decision {
            replaces: Some("01jbx7ab".into()),
            ..decision
        };
        assert!(
            again
                .text()
                .ends_with("\n- in place of an earlier decision")
        );
        assert_eq!(Decision::of(&again.event()), Some(again));
        assert_eq!(
            Decision::of(&Event::Text {
                body: "decided".into()
            }),
            None
        );
    }
}
