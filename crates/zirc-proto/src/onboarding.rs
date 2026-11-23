//! Getting started with zirc, declared once: the eight steps a person or an organisation
//! takes ([`STEPS`]), what each waits for, and the signals that prove each was done
//! ([`Signal`]). zirc.dev records each step the first time it's reached (its `step_reached`
//! event), from what it sees itself and from what the tenant's recorder saw
//! ([`First`], at [`FIRSTS_PATH`]); the apps and the dashboard show the next step from the
//! same list, through the web app's generated types (`bazel run //web:zirc_proto_types`).
//!
//! A step's number suggests an order, but a step waits only for what it [`Step::needs`]: one
//! done early ticks off all the same. The next step is the lowest-numbered one not reached
//! whose needs are.
//!
//! Metadata only: which signal, whose account and when, never what anyone said.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Whose a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "StepScope")]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Each member's own: reached by them, for them.
    Person,
    /// The organisation's: reached once, by whoever does it, for everyone in it.
    Organisation,
}

/// What proves a step was done, and who sees it happen.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS, JsonSchema,
)]
#[ts(rename = "StepSignal")]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// zirc.dev: the organisation's server is running.
    ServerReady,
    /// zirc.dev: a member let a machine in with `zirc login`.
    Machine,
    /// The recorder: a session on a member's machine started watching ([`crate::activity::Watching`]),
    /// or, from a zircd before that record, an agent of theirs said something.
    Agent,
    /// zirc.dev: a member signed in on the iPhone or Android app.
    Phone,
    /// The recorder: an agent of a member's wrote a direct message, as one answering them
    /// does.
    AgentDirect,
    /// The recorder: the organisation's CI said something ([`crate::ci::only_cis`]).
    Ci,
    /// The recorder: a session on a member's machine was woken by its watch
    /// ([`crate::activity::Wake`]).
    Wake,
    /// The recorder: zircd told a session on a member's machine that another changes what it
    /// changes ([`crate::activity::Notice`], an overlap).
    Overlap,
    /// zirc.dev: a second member joined the organisation.
    Teammate,
}

impl Signal {
    /// Every signal.
    pub const ALL: [Self; 9] = [
        Self::ServerReady,
        Self::Machine,
        Self::Agent,
        Self::Phone,
        Self::AgentDirect,
        Self::Ci,
        Self::Wake,
        Self::Overlap,
        Self::Teammate,
    ];

    /// How it's written, as serde writes it.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ServerReady => "server_ready",
            Self::Machine => "machine",
            Self::Agent => "agent",
            Self::Phone => "phone",
            Self::AgentDirect => "agent_direct",
            Self::Ci => "ci",
            Self::Wake => "wake",
            Self::Overlap => "overlap",
            Self::Teammate => "teammate",
        }
    }

    /// The signal written `id`, if it's one.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|signal| signal.id() == id)
    }

    /// Whether the tenant's recorder sees it, rather than zirc.dev: the ones it reports at
    /// [`FIRSTS_PATH`].
    #[must_use]
    pub const fn recorded(self) -> bool {
        matches!(
            self,
            Self::Agent | Self::AgentDirect | Self::Ci | Self::Wake | Self::Overlap
        )
    }

    /// The signal a row of the tenant's activity store gives, if any: its `kind`, whether its
    /// `account` is a machine's (`agent`), whether it's a `direct` message's, and its kept
    /// `data`. What the recorder keeps the first of ([`First`]), and what a backfill reads
    /// history with.
    #[must_use]
    pub fn of_row(
        kind: &str,
        account: Option<&str>,
        agent: bool,
        direct: bool,
        data: Option<&serde_json::Value>,
    ) -> Option<Self> {
        if crate::ci::only_cis(kind) && crate::ci::speaks_for_ci(account) {
            return Some(Self::Ci);
        }
        if !agent || account.is_none() {
            return None;
        }
        match kind {
            crate::activity::WAKE_KIND => Some(Self::Wake),
            crate::activity::NOTICE_KIND => {
                let what = data
                    .and_then(|data| data.get("what"))
                    .and_then(|w| w.as_str());
                (what == Some("overlap")).then_some(Self::Overlap)
            }
            // What zircd says for a session by itself, or a wake's outcome, isn't the agent.
            crate::activity::WAKE_OUTCOME_KIND
            | crate::activity::PROMPT_KIND
            | crate::activity::HINT_SHOWN_KIND
            | crate::activity::GOTCHA_TOLD_KIND
            | "working" => None,
            _ if direct => Some(Self::AgentDirect),
            _ => Some(Self::Agent),
        }
    }
}

/// One step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(rename = "OnboardingStep")]
pub struct Step {
    /// Its number, from 1, which suggests the order.
    pub number: u8,
    /// Its id, for prompts about it and the apps' words: `server`, `computer` and so on.
    pub id: &'static str,
    /// Whose it is.
    pub scope: Scope,
    /// The steps that must be reached before it can be done, by number.
    pub needs: &'static [u8],
    /// What proves it: every one of these, and it's reached when the last of them was seen.
    pub signals: &'static [Signal],
    /// Whether the dashboard's Get started lists it; the others tick off when they happen.
    pub get_started: bool,
}

/// The steps, in number order.
pub const STEPS: [Step; 8] = [
    Step {
        number: 1,
        id: "server",
        scope: Scope::Organisation,
        needs: &[],
        signals: &[Signal::ServerReady],
        get_started: true,
    },
    Step {
        number: 2,
        id: "computer",
        scope: Scope::Person,
        needs: &[1],
        signals: &[Signal::Machine],
        get_started: true,
    },
    Step {
        number: 3,
        id: "agent",
        scope: Scope::Person,
        needs: &[2],
        signals: &[Signal::Agent],
        get_started: true,
    },
    Step {
        number: 4,
        id: "phone",
        scope: Scope::Person,
        needs: &[3],
        signals: &[Signal::Phone, Signal::AgentDirect],
        get_started: true,
    },
    Step {
        number: 5,
        id: "ci",
        scope: Scope::Organisation,
        needs: &[2],
        signals: &[Signal::Ci],
        get_started: true,
    },
    Step {
        number: 6,
        id: "first_wake",
        scope: Scope::Person,
        needs: &[3],
        signals: &[Signal::Wake],
        get_started: true,
    },
    Step {
        number: 7,
        id: "first_overlap",
        scope: Scope::Person,
        needs: &[3],
        signals: &[Signal::Overlap],
        get_started: false,
    },
    Step {
        number: 8,
        id: "teammate",
        scope: Scope::Organisation,
        needs: &[1],
        signals: &[Signal::Teammate],
        get_started: false,
    },
];

impl Step {
    /// When it was reached, by when each signal was `first` seen: when the last of its
    /// signals was, once all were.
    pub fn reached_at<T: Ord + Copy>(&self, first: impl Fn(Signal) -> Option<T>) -> Option<T> {
        self.signals
            .iter()
            .map(|signal| first(*signal))
            .try_fold(None, |last: Option<T>, at| Some(last.max(Some(at?))))
            .flatten()
    }
}

/// From when zirc.dev records steps as they're reached, in RFC 3339: one reached before then is
/// history, which a backfill records as filled in, so the funnel counts only those who went
/// through getting started.
pub const RECORDED_FROM: &str = "2026-11-24T00:00:00Z";

/// The step numbered `number`, if there's one.
#[must_use]
pub fn step(number: u8) -> Option<&'static Step> {
    STEPS.iter().find(|step| step.number == number)
}

/// The next step for someone who has reached `reached` (numbers, theirs and their
/// organisation's): the lowest-numbered one not reached whose needs all are.
#[must_use]
pub fn next(reached: &[u8]) -> Option<&'static Step> {
    STEPS.iter().find(|step| {
        !reached.contains(&step.number) && step.needs.iter().all(|need| reached.contains(need))
    })
}

/// Where a prompt was shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "PromptSurface")]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// The `zirc` command line.
    Terminal,
    /// An agent relayed it, under a line of its watch. Recorded by the tenant's recorder as
    /// a hint shown ([`crate::activity::HintShown`]), never on zirc.dev as well.
    Agent,
    /// One of the apps: desktop, phone or web chat.
    App,
    /// zirc.dev's dashboard.
    Dashboard,
}

/// What became of a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "PromptOutcome")]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// It was shown.
    Shown,
    /// The person did what it said, or followed it.
    Acted,
    /// The person put it away.
    Dismissed,
}

/// A note shown once to each person, which every app agrees they've seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "OnceNote")]
#[serde(rename_all = "snake_case")]
pub enum Note {
    /// What a wake is, the first time one of their agents is woken.
    FirstWake,
    /// What an overlap is, the first time one of their agents is told of one.
    FirstOverlap,
}

impl Note {
    /// Every note.
    pub const ALL: [Self; 2] = [Self::FirstWake, Self::FirstOverlap];
}

/// What a prompt shown, acted on or dismissed says, as an app or the dashboard sends it to
/// zirc.dev at [`PROMPTS_PATH`]: which prompt, a step's id ([`Step::id`]), where and what
/// became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct PromptSeen {
    /// The step it's about, by its id.
    pub prompt: String,
    /// Where.
    pub surface: Surface,
    /// What became of it.
    pub outcome: Outcome,
}

/// Where an app sends a prompt's [`PromptSeen`], as its person: `POST`, 204.
pub const PROMPTS_PATH: &str = "/api/me/prompts";

/// Where an app reads (`GET`, a [`NotesSeen`]) and marks (`POST` `{"note"}`, 204) the
/// one-time notes its person has seen.
pub const NOTES_PATH: &str = "/api/me/notes";

/// The one-time notes a person has seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct NotesSeen {
    /// Each one they've seen.
    pub seen: Vec<Note>,
}

/// A record the recorder's activity store saw first: the first time a signal it sees was
/// seen from an account. Kept as long as the store, whatever the organisation keeps of its
/// messages, so zirc.dev can read it later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct First {
    /// The signal ([`Signal::recorded`]).
    pub signal: Signal,
    /// The account it came from: a machine's, whose owner it's for, or CI's.
    pub account: String,
    /// When, in milliseconds since the Unix epoch.
    #[ts(type = "number")]
    pub at: i64,
}

/// The sidecar's endpoint for the [`First`]s, `{"firsts": [...]}`, which zirc.dev reads.
pub const FIRSTS_PATH: &str = "/v1/firsts";

/// What [`FIRSTS_PATH`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Firsts {
    /// Each signal's first from each account.
    pub firsts: Vec<First>,
}

/// These as TypeScript, for the web app's generated file.
///
/// # Panics
///
/// Never: the steps and notes serialize to JSON.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    let steps = serde_json::to_string(&STEPS).expect("the steps serialize");
    vec![
        Scope::decl(config),
        Signal::decl(config),
        Step::decl(config),
        format!("const STEPS: readonly OnboardingStep[] = {steps};"),
        format!("const STEPS_RECORDED_FROM = {RECORDED_FROM:?};"),
        format!(
            "const RECORDED_SIGNALS: readonly StepSignal[] = [{}];",
            Signal::ALL
                .into_iter()
                .filter(|signal| signal.recorded())
                .map(|signal| format!("{:?}", signal.id()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Surface::decl(config),
        Outcome::decl(config),
        Note::decl(config),
        format!(
            "const ONCE_NOTES: readonly OnceNote[] = {};",
            serde_json::to_string(&Note::ALL).expect("notes serialize")
        ),
        PromptSeen::decl(config),
        format!("const PROMPTS_PATH = {PROMPTS_PATH:?};"),
        format!("const NOTES_PATH = {NOTES_PATH:?};"),
        NotesSeen::decl(config),
        First::decl(config),
        format!("const FIRSTS_PATH = {FIRSTS_PATH:?};"),
        Firsts::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn steps_are_numbered_in_order_and_need_only_earlier_ones() {
        for (at, step) in STEPS.iter().enumerate() {
            assert_eq!(usize::from(step.number), at + 1, "{}", step.id);
            assert!(!step.signals.is_empty(), "{}", step.id);
            for need in step.needs {
                assert!(*need < step.number, "{} needs {need}", step.id);
            }
        }
    }

    #[test]
    fn next_is_the_lowest_whose_needs_are_met() {
        assert_eq!(next(&[]).map(|s| s.id), Some("server"));
        assert_eq!(next(&[1]).map(|s| s.id), Some("computer"));
        // CI first: the agent is still next, and CI stays done.
        assert_eq!(next(&[1, 2, 5]).map(|s| s.id), Some("agent"));
        assert_eq!(next(&[1, 2, 3]).map(|s| s.id), Some("phone"));
        assert_eq!(next(&[1, 2, 3, 4, 5, 6, 7, 8]), None);
    }

    #[test]
    fn a_step_is_reached_when_the_last_of_its_signals_was() {
        let phone = step(4).unwrap();
        let seen = |phone_at, direct_at| {
            move |signal| match signal {
                Signal::Phone => phone_at,
                Signal::AgentDirect => direct_at,
                _ => None,
            }
        };
        assert_eq!(phone.reached_at(seen(Some(3), Some(5))), Some(5));
        assert_eq!(phone.reached_at(seen(Some(7), Some(5))), Some(7));
        assert_eq!(phone.reached_at(seen(Some(7), None)), None);
        assert!(chrono_like(RECORDED_FROM));
    }

    /// Whether `text` reads as an RFC 3339 time in UTC, as the web app and ops parse it.
    fn chrono_like(text: &str) -> bool {
        text.len() == 20 && text.ends_with('Z') && text.as_bytes()[10] == b'T'
    }

    #[test]
    fn signals_round_trip_by_id() {
        for signal in Signal::ALL {
            assert_eq!(Signal::from_id(signal.id()), Some(signal));
            assert_eq!(serde_json::to_value(signal).unwrap(), json!(signal.id()));
        }
        assert_eq!(Signal::from_id("nope"), None);
    }

    #[test]
    fn a_rows_signal() {
        let of = |kind, account, agent, direct, data: Option<serde_json::Value>| {
            Signal::of_row(kind, account, agent, direct, data.as_ref())
        };
        assert_eq!(
            of(
                crate::activity::WATCHING_KIND,
                Some("ada-mbp"),
                true,
                false,
                None
            ),
            Some(Signal::Agent)
        );
        assert_eq!(
            of("intent", Some("ada-mbp"), true, false, None),
            Some(Signal::Agent)
        );
        assert_eq!(
            of("text", Some("ada-mbp"), true, true, None),
            Some(Signal::AgentDirect)
        );
        assert_eq!(of("text", Some("ada"), false, true, None), None);
        assert_eq!(
            of(
                crate::activity::WAKE_KIND,
                Some("ada-mbp"),
                true,
                false,
                None
            ),
            Some(Signal::Wake)
        );
        let overlap = Some(json!({"what": "overlap"}));
        assert_eq!(
            of(
                crate::activity::NOTICE_KIND,
                Some("ada-mbp"),
                true,
                false,
                overlap
            ),
            Some(Signal::Overlap)
        );
        let ahead = Some(json!({"what": "conflict-ahead"}));
        assert_eq!(
            of(
                crate::activity::NOTICE_KIND,
                Some("ada-mbp"),
                true,
                false,
                ahead
            ),
            None
        );
        assert_eq!(of("working", Some("ada-mbp"), true, false, None), None);
        assert_eq!(
            of(
                crate::ci::LANDING_KIND,
                Some(crate::ci::NICK),
                false,
                false,
                None
            ),
            Some(Signal::Ci)
        );
        // A machine saying a landing landed isn't CI.
        assert_eq!(
            of(crate::ci::LANDING_KIND, Some("ada-mbp"), true, false, None),
            Some(Signal::Agent)
        );
    }

    #[test]
    fn a_first_and_a_prompt_read_as_sent() {
        let first: First =
            serde_json::from_value(json!({"signal": "wake", "account": "ada-mbp", "at": 5}))
                .unwrap();
        assert_eq!(first.signal, Signal::Wake);
        let seen: PromptSeen = serde_json::from_value(
            json!({"prompt": "agent", "surface": "dashboard", "outcome": "dismissed"}),
        )
        .unwrap();
        assert_eq!(seen.outcome, Outcome::Dismissed);
    }
}
