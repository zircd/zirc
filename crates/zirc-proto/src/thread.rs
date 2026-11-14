//! A thread: a channel of its own under a repository's channel, `#api/login-times-out`,
//! started from one message in that channel. The person who starts it posts `x-zirc/thread`,
//! a [`crate::Event::Custom`] whose data is a [`Started`], in the parent channel, so every app
//! learns of it and shows "↳ N replies" under the message, and plain IRC clients read its
//! text. In the thread, the first message quotes the one it hangs off, tagged
//! [`THREAD_OF_TAG`]. Both are optional to understand: a client that knows neither sees an
//! ordinary channel and an ordinary line.

use serde::{Deserialize, Serialize};

use crate::Event;

/// The custom event kind a thread's start is sent as.
pub const KIND: &str = "x-zirc/thread";

/// The `IRCv3` client tag on a thread's first message: the id of the message in the parent
/// channel that the thread hangs off.
pub const THREAD_OF_TAG: &str = "+zirc.dev/thread-of";

/// A thread started from a message: what [`KIND`]'s data says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    /// The thread's channel: `#api/login-times-out`.
    pub channel: String,
    /// The server's id for the message in the parent channel it hangs off.
    pub of: String,
}

impl Started {
    /// It as an event, saying in words what it is for plain IRC clients: `started a thread:
    /// #api/login-times-out`.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).unwrap_or_default(),
            text: format!("started a thread: {}", self.channel),
        }
    }

    /// What `event` says of a thread's start, if it's one.
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
    fn goes_as_a_custom_event_and_back() {
        let started = Started {
            channel: "#api/login-times-out".into(),
            of: "m1".into(),
        };
        let event = started.event();
        assert_eq!(event.human_text(), "started a thread: #api/login-times-out");
        assert_eq!(Started::of(&event), Some(started));
        assert_eq!(
            Started::of(&Event::Text {
                body: "hello".into()
            }),
            None
        );
    }
}
