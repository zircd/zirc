//! The server's own nicks, each connected to Ergo's loopback as an account of zirc's own: a
//! workflow's, or a role's (`crate::roles`). What keeps one connected, catching up and
//! passing on what it hears is [`connection`], said once for both.

use std::time::Duration;

use crate::exporter::Said as Item;

/// How long a nick's connection waits before connecting again, at first and at most.
pub(crate) const RETRY: Duration = Duration::from_secs(5);
pub(crate) const RETRY_MOST: Duration = Duration::from_mins(5);

/// How long connecting may take.
pub(crate) const WITHIN: Duration = Duration::from_secs(8);

/// How far back a nick catches up on a channel's history when it joins, but `#workflows`,
/// which it reads whole.
pub const WINDOW: Duration = Duration::from_hours(24);

/// A message in a channel's history.
pub(crate) struct Row {
    /// Its id on the server.
    pub msgid: String,
    /// The channel, as the history keeps it.
    pub channel: String,
    /// When, in unix seconds, as the server says.
    pub at: u64,
    /// The message.
    pub item: Item,
}

pub mod connection;
