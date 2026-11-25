//! Direct messages leaving the sidebar: those the person closed, until something new is said
//! in them. One is still found by Jump to and search, and shown while it's open.

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::Chat;
use super::conversation::fold;
use super::list::DirectRow;

/// A direct message the person closed, and when: apps keep these per organisation, as they
/// keep stars, and give them back ([`Chat::set_closed`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Closed {
    /// The conversation's name.
    pub name: String,
    /// When it was closed, in milliseconds since 1970.
    #[ts(type = "number")]
    pub at_ms: i64,
}

impl Chat {
    /// Closes the direct message `name`: it leaves the sidebar until something newer than now
    /// is said in it, or it's opened again.
    pub fn close_direct(&mut self, name: &str) {
        self.closed.insert(fold(name), (self.clock)());
    }

    /// What an ended agent's note's [`super::REMOVE_FROM_MESSAGES`] does: closes the direct
    /// message `name` ([`Chat::close_direct`]) and, while it's shown, shows the one before
    /// it instead, as leaving a channel does. Its history is kept for Jump to and search.
    pub fn remove_from_messages(&mut self, name: &str) -> Vec<super::Line> {
        self.close_direct(name);
        match self.position(name) {
            Some(at) => self.stop_showing(at),
            None => Vec::new(),
        }
    }

    /// The direct messages closed, sorted by name, for an app to keep.
    #[must_use]
    pub fn closed(&self) -> Vec<Closed> {
        let mut closed: Vec<Closed> = self
            .closed
            .iter()
            .map(|(name, at)| Closed {
                name: name.clone(),
                at_ms: at.timestamp_millis(),
            })
            .collect();
        closed.sort_by(|a, b| a.name.cmp(&b.name));
        closed
    }

    /// Gives back the direct messages an app kept closed ([`Chat::closed`]).
    pub fn set_closed(&mut self, closed: impl IntoIterator<Item = Closed>) {
        self.closed = closed
            .into_iter()
            .filter_map(|closed| {
                DateTime::from_timestamp_millis(closed.at_ms).map(|at| (fold(&closed.name), at))
            })
            .collect();
    }

    /// Whether the direct message `row` is left out of the sidebar: closed with nothing said
    /// since. Never the conversation shown, nor one with something unread.
    pub(super) fn left_out(&self, row: &DirectRow) -> bool {
        if self.selected().name.eq_ignore_ascii_case(&row.name) || row.unread > 0 {
            return false;
        }
        let name = fold(&row.name);
        self.closed
            .get(&name)
            .is_some_and(|closed| row.latest.is_none_or(|latest| latest <= *closed))
    }
}
