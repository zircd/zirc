//! What the connection is doing, and what the server and its services said: a status bar's
//! business, kept out of every conversation.

use std::collections::VecDeque;

use chrono::{DateTime, TimeDelta, Utc};
pub use zirc_proto::logging::Logged;

/// How many of the server's notices are kept.
const MAX_NOTICES: usize = 50;
/// How long a notice is news, for [`Status::recent`].
const RECENT: TimeDelta = TimeDelta::minutes(2);

/// Where the connection is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Connection {
    /// Connecting, or reconnecting after it was lost.
    #[default]
    Connecting,
    /// Connected and registered.
    Connected,
    /// Lost, and why.
    Disconnected(String),
}

/// Something the server or one of its services said, or an error it answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// When it arrived.
    pub time: DateTime<Utc>,
    /// Who said it: a service such as `NickServ`, or empty for the server.
    pub from: String,
    /// What it said, without formatting.
    pub text: String,
}

impl Notice {
    /// It as one line: `NickServ: You are now logged in`.
    #[must_use]
    pub fn line(&self) -> String {
        if self.from.is_empty() {
            self.text.clone()
        } else {
            format!("{}: {}", self.from, self.text)
        }
    }
}

/// The connection, and what was said about it.
#[derive(Debug, Clone, Default)]
pub struct Status {
    /// Where the connection is.
    pub connection: Connection,
    /// The latest notices, oldest first.
    pub notices: VecDeque<Notice>,
    /// What the server logs of what people send, as its MOTD said on connecting: for
    /// everyone to see where they type, while it's so.
    pub logged: Option<Logged>,
    /// What this app asked: away, saying why (`away` when it gave no reason), until it says
    /// it's back; asked again on each connection. The person is away only once each of their
    /// apps is (Ergo's auto-away), so this may be set while [`super::Chat::away`] for us
    /// isn't: away here, shown as here while another app is active.
    pub away_here: Option<String>,
}

impl Status {
    /// The latest notice, while it's still news at `now`.
    #[must_use]
    pub fn recent(&self, now: DateTime<Utc>) -> Option<&Notice> {
        self.notices
            .back()
            .filter(|notice| now - notice.time < RECENT)
    }

    pub(super) fn notice(&mut self, time: DateTime<Utc>, from: &str, text: String) {
        if text.is_empty() {
            return;
        }
        self.notices.push_back(Notice {
            time,
            from: from.to_owned(),
            text,
        });
        if self.notices.len() > MAX_NOTICES {
            self.notices.pop_front();
        }
    }
}
