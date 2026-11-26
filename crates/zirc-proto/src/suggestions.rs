//! Suggestions for a team's playbook: what its server noticed in how its sessions' work went
//! and proposes keeping as a lesson. The first kind: a test target that failed the first
//! landing of several sessions ([`SESSIONS`] within [`DAYS`]), proposed as a gotcha for the
//! paths those sessions changed.
//!
//! The server says each step in the repository's channel, as zirc's own account, as a
//! [`Suggestion`] in a custom event of [`KIND`]:
//!
//! 1. [`State::Asked`]: what it noticed, asking the sessions it happened to for the lesson in
//!    one line, as a reply to that message.
//! 2. [`State::Proposed`]: the first reply's words, or after [`DRAFT_SECS`] without one the
//!    plain facts ([`Suggestion::facts`]), for a person's reaction.
//! 3. [`State::Kept`], on a person's [`KEEP`]: the server says the gotcha in the repository's
//!    gotchas room, where every machine reads it. Or [`State::Muted`], on a person's
//!    [`MUTE`]: nothing more is said of it for [`MUTED_DAYS`].
//!
//! Only a person's reaction counts, never a machine's.

use serde::{Deserialize, Serialize};

use crate::Event;
use crate::gotchas::{self, Gotcha};

/// The kind of the event a suggestion's steps are said in.
pub const KIND: &str = "x-zirc/suggestion";

/// How many different sessions' first landings a target must fail before it's suggested.
pub const SESSIONS: usize = 3;

/// Within how many days.
pub const DAYS: u32 = 7;

/// How long the sessions asked have to say the lesson, in seconds, before the plain facts
/// are proposed.
pub const DRAFT_SECS: u64 = 3_600;

/// How many days a muted suggestion stays quiet.
pub const MUTED_DAYS: u32 = 7;

/// The reaction that keeps a proposed lesson.
pub const KEEP: &str = "👍";

/// The reaction that mutes a suggestion.
pub const MUTE: &str = "👎";

/// How many paths a suggestion names at most: those most of its sessions changed.
pub const PATHS: usize = 3;

/// Where a suggestion stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// The sessions it happened to are asked for the lesson.
    Asked,
    /// A lesson is proposed, for a person's reaction.
    Proposed,
    /// A person kept it: it's a gotcha now.
    Kept,
    /// A person muted it.
    Muted,
}

/// A suggestion, as each of its steps says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Its id ([`Suggestion::id_of`]).
    pub id: String,
    /// Where it stands.
    pub state: State,
    /// The test target that failed.
    pub target: String,
    /// The sessions whose first landing it failed, oldest first.
    pub sessions: Vec<String>,
    /// The paths most of them changed, at most [`PATHS`].
    pub paths: Vec<String>,
    /// The lesson proposed or kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Who wrote the lesson proposed, or kept or muted it: a session or an account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

impl Suggestion {
    /// The id of the suggestion about `target` in the repository whose channel is `repo`: the
    /// same each time it's noticed, so it's suggested once.
    #[must_use]
    pub fn id_of(repo: &str, target: &str) -> String {
        crate::hints::fingerprint(&format!("first-landing {repo} {target}"))
    }

    /// What was noticed, as a lesson in plain facts: what's proposed when no session says it
    /// better.
    #[must_use]
    pub fn facts(&self) -> String {
        format!(
            "{} failed the first landing of {} sessions in {DAYS} days that changed these paths: run it before landing a change here.",
            self.target,
            self.sessions.len()
        )
    }

    /// The gotcha it becomes once kept, learnt by whoever wrote its words.
    #[must_use]
    pub fn gotcha(&self) -> Gotcha {
        Gotcha {
            paths: self.paths.clone(),
            text: self.text.clone().unwrap_or_else(|| self.facts()),
            who: self.by.clone().unwrap_or_default(),
            at: None,
            id: None,
        }
    }

    /// What its step says, for people and for the sessions it names.
    #[must_use]
    pub fn says(&self) -> String {
        let paths = self.paths.join(", ");
        let tag = format!("[suggestion {}]", crate::short_id(&self.id));
        let by = self.by.as_deref().unwrap_or("someone");
        let text = self.text.clone().unwrap_or_else(|| self.facts());
        match self.state {
            State::Asked => format!(
                "suggestion: {} failed the first landing of {} sessions in {DAYS} days ({}), which changed {paths}. One of you: reply to this message with the lesson in one line, what goes wrong and what to do instead, and it's proposed as a gotcha for those paths. With no reply in an hour, the plain facts are. {tag}",
                self.target,
                self.sessions.len(),
                self.sessions.join(", "),
            ),
            State::Proposed => format!(
                "proposed gotcha for {paths}, in {by}'s words: {text} A person's {KEEP} keeps it; {MUTE} mutes this for {MUTED_DAYS} days. {tag}"
            ),
            State::Kept => format!("{by} kept the gotcha for {paths}: {text} {tag}"),
            State::Muted => format!(
                "{by} muted the suggestion about {} for {MUTED_DAYS} days {tag}",
                self.target
            ),
        }
    }

    /// As its event.
    ///
    /// # Panics
    ///
    /// Never: it serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a suggestion serializes"),
            text: self.says(),
        }
    }

    /// The suggestion `event` says, if it says one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// The event that says its gotcha, in the repository's gotchas room.
    #[must_use]
    pub fn gotcha_event(&self) -> Event {
        let gotcha = self.gotcha();
        Event::Custom {
            kind: gotchas::KIND.to_owned(),
            data: serde_json::json!(gotchas::Data {
                paths: gotcha.paths
            }),
            text: gotcha.text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(state: State) -> Suggestion {
        Suggestion {
            id: Suggestion::id_of("#api", "//web:e2e"),
            state,
            target: "//web:e2e".into(),
            sessions: vec!["cc-a".into(), "cc-b".into(), "cc-c".into()],
            paths: vec!["web/e2e".into(), "web/src/app".into()],
            text: None,
            by: None,
        }
    }

    #[test]
    fn a_suggestion_is_read_back_from_its_event_and_names_its_sessions() {
        let asked = suggestion(State::Asked);
        assert_eq!(Suggestion::of(&asked.event()), Some(asked.clone()));
        let says = asked.says();
        assert!(says.contains(
            "//web:e2e failed the first landing of 3 sessions in 7 days (cc-a, cc-b, cc-c)"
        ));
        assert!(says.contains("web/e2e, web/src/app"));
        assert!(says.ends_with(&format!("[suggestion {}]", crate::short_id(&asked.id))));
        assert_eq!(Suggestion::of(&Event::custom("x-fjord/deploy", "hi")), None);
    }

    #[test]
    fn the_same_target_in_a_repository_is_the_same_suggestion() {
        assert_eq!(
            Suggestion::id_of("#api", "//web:e2e"),
            Suggestion::id_of("#api", "//web:e2e")
        );
        assert_ne!(
            Suggestion::id_of("#api", "//web:e2e"),
            Suggestion::id_of("#web", "//web:e2e")
        );
    }

    #[test]
    fn kept_without_anyones_words_it_is_a_gotcha_of_the_plain_facts() {
        let mut kept = suggestion(State::Kept);
        kept.by = Some("ada".into());
        let gotcha = kept.gotcha();
        assert_eq!(gotcha.paths, ["web/e2e", "web/src/app"]);
        assert_eq!(
            gotcha.text,
            "//web:e2e failed the first landing of 3 sessions in 7 days that changed these paths: run it before landing a change here."
        );
        // Said as any gotcha is, so machines read it as one.
        let said = gotchas::lesson(&kept.gotcha_event());
        assert!(said.is_some());
        kept.text = Some("Run //web:e2e before landing.".into());
        assert_eq!(kept.gotcha().text, "Run //web:e2e before landing.");
        assert!(kept.says().starts_with(
            "ada kept the gotcha for web/e2e, web/src/app: Run //web:e2e before landing."
        ));
    }
}
