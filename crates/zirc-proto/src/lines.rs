//! Which lines a worktree's work changes in each of its files: `x-zirc/lines`, a
//! [`crate::Event::Custom`] whose data is a [`Lines`], sent beside the session's working set
//! ([`crate::Event::Working`]) and, like it, live: out of every channel's history. With it,
//! another machine can tell whether its own changes to the same file are on the same lines
//! or elsewhere, rather than only that both change it.
//!
//! The lines are of the commit the work is built on, the working set's `base`, so they line
//! up only with changes counted from the same commit: a reader built on another first
//! counts its own changes from this one, and without that commit it can't tell.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Event;

/// The custom event kind it's sent as.
pub const KIND: &str = "x-zirc/lines";

/// The most spans of lines said of one file: closer ones are merged into one first, which
/// can only make a reader see the same lines where there were none, never miss them.
pub const MOST_SPANS: usize = 8;

/// What a session's work changes, file by file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Lines {
    /// The commit the lines are of, by the first [`crate::BASE_DIGITS`] digits of its id:
    /// the working set's `base`.
    pub base: String,
    /// Whether this adds to what the session's last said rather than replacing it, as each
    /// part after a first does when it's too long for one line.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub continued: bool,
    /// For each file, the lines of `base` changed, as `[first, count]`, with a count of 0
    /// for lines added after `first`, as [`crate::Event::Push`]'s.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lines: BTreeMap<String, Vec<[u32; 2]>>,
}

impl Lines {
    /// It as the event that says it. It has no words: it's live, which apps don't show and
    /// plain IRC clients never get, and words go in a line twice, in the payload and after
    /// it, which left no room for a single file's lines on a long path (12,865 dropped in a
    /// day, 14 to 16 Nov 2026).
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).unwrap_or_default(),
            text: String::new(),
        }
    }

    /// What `event` says, if it's one.
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

/// `spans`, sorted, as `most` at most: the two with the smallest gap between them merged
/// into one that covers both, until few enough are left. An insertion counts as its line.
#[must_use]
pub fn coarse(mut spans: Vec<[u32; 2]>, most: usize) -> Vec<[u32; 2]> {
    let last = |[start, len]: [u32; 2]| start + len.max(1) - 1;
    spans.sort_unstable();
    while spans.len() > most.max(1) {
        let Some(at) =
            (0..spans.len() - 1).min_by_key(|&i| spans[i + 1][0].saturating_sub(last(spans[i])))
        else {
            break;
        };
        let (first, next) = (spans[at], spans[at + 1]);
        let end = last(first).max(last(next));
        spans.splice(at..=at + 1, [[first[0], end - first[0] + 1]]);
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_itself_and_reads_back() {
        let lines = Lines {
            base: "abc1234".into(),
            continued: false,
            lines: [("src/a.rs".to_owned(), vec![[10, 3], [40, 0]])].into(),
        };
        assert_eq!(Lines::of(&lines.event()), Some(lines));
        assert_eq!(Lines::of(&Event::Text { body: "hi".into() }), None);
    }

    #[test]
    fn merges_the_closest_spans_first() {
        let spans = vec![[100, 2], [10, 1], [12, 1], [50, 5]];
        assert_eq!(
            coarse(spans.clone(), 4),
            [[10, 1], [12, 1], [50, 5], [100, 2]]
        );
        // 10 and 12 are closest: one span from 10 through 12.
        assert_eq!(coarse(spans.clone(), 3), [[10, 3], [50, 5], [100, 2]]);
        assert_eq!(coarse(spans, 1), [[10, 92]]);
    }
}
