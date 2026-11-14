//! A run of a workflow ([`crate::workflow`]) and what's said about it.
//!
//! The workflow's nick posts a [`RunEvent`] (`x-zirc/run`) in its channel each time a run
//! changes: started, a step given to someone, claimed, done, and so on. Each carries the run's
//! card, with the same id every time, so apps draw one card that keeps up, and the change
//! that made it. The run's history is these events, oldest first: the sidecar folds them
//! back into the run when it starts again, so nothing else needs keeping.
//!
//! Whoever does a step says so with a [`StepAction`] (`x-zirc/step`, `zirc step`) in the same
//! channel: claiming it, handing it back with its outputs, failing it, or answering an
//! approval. The workflow decides what each counts for: only the people a step went to can
//! claim it, and only an approver's own account can answer.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Event;
use crate::card::Card;

/// The custom event kind a run's change is posted as.
pub const KIND: &str = "x-zirc/run";

/// The custom event kind a step's worker says what they did as.
pub const STEP_KIND: &str = "x-zirc/step";

/// The most a step's outputs are, in bytes as JSON.
pub const MAX_OUTPUTS: usize = 1500;

/// A run's change, posted by its workflow's nick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunEvent {
    /// The run: its workflow's name and its number, such as `master-12`.
    pub run: String,
    /// When, in unix seconds, as the workflow's clock said: how long a step has been quiet
    /// is counted from it.
    #[serde(default)]
    pub at: u64,
    /// What changed.
    pub change: Change,
    /// The run's card as it stands now, which apps draw in place of the last one.
    pub card: Card,
}

/// What changed in a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Change {
    /// It started.
    Started {
        /// Who or what started it: a nick, `state:<name>` for a state's trigger, or
        /// `every:<n>:<time>` for the time that came round, of the `n`th trigger.
        by: String,
        /// What it was given.
        inputs: BTreeMap<String, String>,
        /// The version of the workflow it runs, which it keeps to its end.
        version: String,
    },
    /// A step went to whoever may do it, named in the event's text so their watches wake.
    Assigned {
        /// The step.
        step: String,
        /// Who may claim it.
        to: Vec<String>,
    },
    /// Someone took a step on.
    Claimed {
        /// The step.
        step: String,
        /// Who.
        by: String,
    },
    /// A step was done, and handed back its outputs.
    Done {
        /// The step.
        step: String,
        /// Who did it; the workflow's own nick for a wait or a post.
        by: String,
        /// What it handed back.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        outputs: BTreeMap<String, String>,
    },
    /// An approver answered.
    Answered {
        /// The step.
        step: String,
        /// Who.
        by: String,
        /// Their answer.
        yes: bool,
        /// For a yes to an irreversible approval: when it counts, in unix seconds, unless a
        /// no comes first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        counts_at: Option<u64>,
    },
    /// A step's `when` didn't hold, so it didn't run.
    Skipped {
        /// The step.
        step: String,
    },
    /// A task reached one of its stages, as the workflow saw, or went back from one.
    Stage {
        /// The step.
        step: String,
        /// The stage it's at now; none when it went back to before the first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
        /// Who moved it.
        by: String,
        /// What moved it, such as the branch pushed or the landing's build.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A step was quiet past its `nudge`, and whoever has it was asked how it goes.
    Nudged {
        /// The step.
        step: String,
    },
    /// A step was quiet past its `escalate` and went to its fallback.
    Escalated {
        /// The step.
        step: String,
        /// Who may claim it now.
        to: Vec<String>,
    },
    /// It ended.
    Finished {
        /// How.
        outcome: Outcome,
        /// Why, in a few words.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "RunOutcome")]
pub enum Outcome {
    /// Every step was done or skipped.
    Done,
    /// A step failed, or its time ran out with nobody to fall back to.
    Failed,
    /// An approver said no, or someone cancelled it.
    Cancelled,
}

impl Change {
    /// Who did it, when someone did: started the run, took a step on, did it, answered it
    /// or moved its task along, as `machine/session` or a person's nick.
    #[must_use]
    pub fn by(&self) -> Option<&str> {
        match self {
            Self::Started { by, .. }
            | Self::Claimed { by, .. }
            | Self::Done { by, .. }
            | Self::Answered { by, .. }
            | Self::Stage { by, .. } => Some(by),
            Self::Assigned { .. }
            | Self::Skipped { .. }
            | Self::Nudged { .. }
            | Self::Escalated { .. }
            | Self::Finished { .. } => None,
        }
    }
}

impl RunEvent {
    /// The event that carries it, saying `text`.
    ///
    /// # Panics
    ///
    /// Never: a run's event serializes to JSON.
    #[must_use]
    pub fn event(&self, text: String) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a run's event serializes"),
            text,
        }
    }

    /// The run's event `event` carries, if it's one.
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

/// What a step's worker says they did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StepAction {
    /// The run, such as `master-12`.
    pub run: String,
    /// The step.
    pub step: String,
    /// What they did.
    pub act: Act,
}

/// What a worker did with a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Act {
    /// Took it on.
    Claim,
    /// Did it, handing back its outputs.
    Done {
        /// Each of the step's outputs.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        outputs: BTreeMap<String, String>,
    },
    /// Couldn't do it, which fails the run.
    Fail {
        /// Why.
        why: String,
    },
    /// Answered an approval.
    Answer {
        /// Yes or no.
        yes: bool,
    },
    /// Cancelled the whole run.
    Cancel,
}

impl StepAction {
    /// The event that carries it, with a line saying it for people.
    ///
    /// # Panics
    ///
    /// Never: a step's action serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        let what = match &self.act {
            Act::Claim => "claimed".to_owned(),
            Act::Done { .. } => "done with".to_owned(),
            Act::Fail { why } => format!("failed ({})", why.replace('\n', " ")),
            Act::Answer { yes: true } => "said yes to".to_owned(),
            Act::Answer { yes: false } => "said no to".to_owned(),
            Act::Cancel => "cancelled".to_owned(),
        };
        let text = match self.act {
            Act::Cancel => format!("{what} `{}`", self.run),
            _ => format!("{what} `{}` of `{}`", self.step, self.run),
        };
        Event::Custom {
            kind: STEP_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a step's action serializes"),
            text,
        }
    }

    /// The step's action `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == STEP_KIND => {
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
    fn a_run_event_carries_its_card_where_apps_find_it() {
        let event = RunEvent {
            run: "master-1".into(),
            at: 1000,
            change: Change::Claimed {
                step: "fix".into(),
                by: "m3-mac/cc-1".into(),
            },
            card: Card {
                id: "master-1".into(),
                title: "master-1: fixing master".into(),
                state: crate::card::State::Running,
                about: None,
                fields: Vec::new(),
                links: Vec::new(),
                actions: Vec::new(),
                steps: Vec::new(),
                note: None,
            },
        };
        let custom = event.event("m3-mac/cc-1 claimed `fix`".into());
        assert!(crate::check_kind(KIND).is_none());
        assert_eq!(Card::in_event(&custom), Some(event.card.clone()));
        assert_eq!(RunEvent::of(&custom), Some(event));
    }

    #[test]
    fn a_step_action_says_what_was_done() {
        let action = StepAction {
            run: "master-1".into(),
            step: "fix".into(),
            act: Act::Done {
                outputs: BTreeMap::from([("cause".into(), "mine".into())]),
            },
        };
        let event = action.event();
        assert!(crate::check_kind(STEP_KIND).is_none());
        assert_eq!(event.human_text(), "done with `fix` of `master-1`");
        assert_eq!(StepAction::of(&event), Some(action));
        let cancel = StepAction {
            run: "master-1".into(),
            step: String::new(),
            act: Act::Cancel,
        };
        assert_eq!(cancel.event().human_text(), "cancelled `master-1`");
    }
}
