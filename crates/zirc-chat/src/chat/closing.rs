//! Direct messages leaving the sidebar: those the person closed, until something new is said
//! in them, and the threads of our builds' machines a while after each went offline. Either
//! is still found by Jump to and search, and shown while it's open.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::Chat;
use super::conversation::fold;
use super::list::DirectRow;

/// How long a build's machine's thread stays in the sidebar after it goes offline.
pub const BUILD_GONE_AFTER: Duration = Duration::minutes(5);

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

/// Whether a machine, by its own name, is a build's: zirc.dev names the machine it makes for
/// a CI build `build-<number>`, or `build-<8 hex digits>` without one (web/src/lib/ci-machines.ts:
/// change both).
#[must_use]
pub fn is_build_machine(machine: &str) -> bool {
    machine.strip_prefix("build-").is_some_and(|id| {
        (!id.is_empty() && id.len() <= 9 && id.bytes().all(|b| b.is_ascii_digit()))
            || (id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

impl Chat {
    /// Closes the direct message `name`: it leaves the sidebar until something newer than now
    /// is said in it, or it's opened again. For a suggestion ([`Chat::is_suggestion`]), it
    /// hides it the same way.
    pub fn close_direct(&mut self, name: &str) {
        self.closed.insert(fold(name), (self.clock)());
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

    /// Notes that `nick` went offline at `at`, or came back (`None`).
    pub(super) fn went(&mut self, nick: &str, at: Option<DateTime<Utc>>) {
        match at {
            Some(at) => {
                self.gone.insert(fold(nick), at);
            }
            None => {
                self.gone.remove(&fold(nick));
            }
        }
    }

    /// Whether the direct message `row` is left out of the sidebar: closed with nothing said
    /// since, or the thread of one of our builds' machines gone for [`BUILD_GONE_AFTER`], or
    /// since before we knew. Never the conversation shown, nor one with something unread.
    pub(super) fn left_out(&self, row: &DirectRow, online: &[String]) -> bool {
        if self.selected().name.eq_ignore_ascii_case(&row.name) || row.unread > 0 {
            return false;
        }
        let name = fold(&row.name);
        if let Some(closed) = self.closed.get(&name)
            && row.latest.is_none_or(|latest| latest <= *closed)
        {
            return true;
        }
        let ended_build = self.whose(&row.name).is_some_and(|(owner, machine)| {
            owner.eq_ignore_ascii_case(&self.me) && is_build_machine(&machine)
        });
        ended_build
            && !online
                .iter()
                .any(|nick| nick.eq_ignore_ascii_case(&row.name))
            && self
                .gone
                .get(&name)
                .is_none_or(|at| (self.clock)() - *at >= BUILD_GONE_AFTER)
    }
}
