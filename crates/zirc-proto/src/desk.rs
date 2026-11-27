//! A coworker's desk, as people and the sessions sitting there read it.
//!
//! What a session sitting down is handed, so it carries on as the coworker rather than
//! starting afresh (the user's choice, 27 Nov 2026: re-seating is the sentence each time, so
//! `zirc desk take` hands the new session what the last one was doing and what's waiting).
//! The server's sidecar keeps it for each coworker beside its seat, whose last holder `zirc
//! desk take` says already: what a session at the desk last said it was doing there, what
//! woke the coworker lately and what waits for it ([`Handover`]). Machines ask for it at
//! [`HANDOVER_PATH`], on the organisation's server alone: it holds what people wrote, which
//! never leaves the server.
//!
//! Every word `zirc desk take` prints of it is here ([`Handover::lines`]).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::woke::Woke;

/// Where a machine asks for coworker `{name}`'s handover, under the server's content API.
pub const HANDOVER_PATH: &str = "/coworkers/{name}/handover";

/// How many of a coworker's latest wakes are kept for its handover.
pub const RECENT: usize = 5;

/// How much of what came a handover's line shows, in characters.
pub const SHOWN: usize = 160;

/// Something that came for a coworker: what woke it, or waits to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Came {
    /// What woke it, or will.
    pub woke: Woke,
    /// Its words: a direct message's, or what woke it.
    pub text: String,
    /// When, unix seconds.
    pub at: u64,
}

impl Came {
    /// It as a handover's line says it, `now` being unix seconds: `3 h ago, Rajesh wrote to
    /// it: the login page 500s`.
    #[must_use]
    pub fn line(&self, now: u64) -> String {
        let what = self.woke.words(self.woke.from.as_deref());
        let text = crate::clip_line(&self.text, SHOWN);
        let when = ago(now.saturating_sub(self.at));
        if text.is_empty() {
            format!("{when}, {what}")
        } else {
            format!("{when}, {what}: {text}")
        }
    }
}

/// What a coworker's desk hands the next session to sit there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Handover {
    /// The coworker.
    pub coworker: String,
    /// What a session at the desk last said it was doing there: its latest `taking:` or
    /// intent, said as the coworker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// What woke the coworker lately, oldest first, at most [`RECENT`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<Came>,
    /// What waits for it, oldest first: what reaches the new session's watch once it sits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<Came>,
}

impl Handover {
    /// Whether there's nothing to hand over: a coworker no session has sat at, nothing come.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.doing.is_none() && self.recent.is_empty() && self.waiting.is_empty()
    }

    /// What `zirc desk take` prints of it, after the coworker's brief, a line each, `now`
    /// being unix seconds: none when there's nothing to hand over.
    #[must_use]
    pub fn lines(&self, now: u64) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(doing) = &self.doing {
            lines.push(format!("{} was on: {doing}", self.coworker));
        }
        if !self.recent.is_empty() {
            lines.push(format!("what woke {} lately:", self.coworker));
            lines.extend(
                self.recent
                    .iter()
                    .map(|came| format!("  {}", came.line(now))),
            );
        }
        if !self.waiting.is_empty() {
            lines.push(format!(
                "waiting for {}, which your watch now brings you:",
                self.coworker
            ));
            lines.extend(
                self.waiting
                    .iter()
                    .map(|came| format!("  {}", came.line(now))),
            );
        }
        lines
    }
}

/// How long ago `seconds` was, as a handover says it: `just now`, `12 min ago`, `3 h ago`,
/// `2 days ago`.
#[must_use]
pub fn ago(seconds: u64) -> String {
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        86_400..172_800 => "a day ago".to_owned(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::woke::{By, What};

    fn came(what: What, by: By, from: Option<&str>, text: &str, at: u64) -> Came {
        Came {
            woke: Woke {
                from: from.map(str::to_owned),
                ..Woke::new(what, by)
            },
            text: text.to_owned(),
            at,
        }
    }

    #[test]
    fn a_handover_says_what_its_coworker_was_on_what_woke_it_and_what_waits() {
        let handover = Handover {
            coworker: "john".into(),
            doing: Some("taking: fix the flaky login test".into()),
            recent: vec![came(
                What::Wrote,
                By::Person,
                Some("Raj"),
                "the login\npage 500s",
                10_000 - 600,
            )],
            waiting: vec![came(
                What::Wrote,
                By::Person,
                Some("Andre"),
                "",
                10_000 - 30,
            )],
        };
        assert_eq!(
            handover.lines(10_000),
            [
                "john was on: taking: fix the flaky login test",
                "what woke john lately:",
                "  10 min ago, Raj wrote to it: the login page 500s",
                "waiting for john, which your watch now brings you:",
                "  just now, Andre wrote to it",
            ]
        );
        assert!(Handover::default().lines(1).is_empty());
        assert!(Handover::default().is_empty());
    }

    #[test]
    fn how_long_ago_is_said_in_the_largest_whole_unit() {
        assert_eq!(ago(59), "just now");
        assert_eq!(ago(61), "1 min ago");
        assert_eq!(ago(3 * 3600 + 5), "3 h ago");
        assert_eq!(ago(90_000), "a day ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }
}
