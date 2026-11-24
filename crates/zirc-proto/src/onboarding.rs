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
                let notice = data.and_then(|data| {
                    serde_json::from_value::<crate::activity::Notice>(data.clone()).ok()
                });
                notice
                    .is_some_and(|notice| notice.what == crate::activity::Noticed::Overlap)
                    .then_some(Self::Overlap)
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
    /// Whether it can be begun before what it needs is reached, so it's offered any time: the
    /// phone's app installs while the computer is being set up, though the step is reached
    /// only once an agent answers there.
    pub early: bool,
    /// What it's called, wherever it's listed or suggested.
    pub title: &'static str,
    /// What it's for, in a line under its title.
    pub about: &'static str,
    /// What to do to reach it, as a command line says it after `Next:`, when it's the next
    /// step ([`Progress::next`]): one line, naming the command where there's one.
    pub next_line: &'static str,
    /// The kind of the hint an agent's first watch shows while this is its person's next
    /// step ([`crate::hints::kind`]'s `next.*`), where an agent has something to tell them.
    pub agent_hint: Option<&'static str>,
}

/// What every surface starts its prompt for the next step with: `Next: <what to do>`.
pub const NEXT_PREFIX: &str = "Next:";

/// Where the apps are had, as a prompt names it.
pub const APP_PAGE: &str = "zirc.dev/app";

/// Each step's id ([`Step::id`]), for whoever tells one step from another.
pub mod id {
    /// Step 1.
    pub const SERVER: &str = "server";
    /// Step 2.
    pub const COMPUTER: &str = "computer";
    /// Step 3.
    pub const AGENT: &str = "agent";
    /// Step 4.
    pub const PHONE: &str = "phone";
    /// Step 5.
    pub const CI: &str = "ci";
    /// Step 6.
    pub const FIRST_WAKE: &str = "first_wake";
    /// Step 7.
    pub const FIRST_OVERLAP: &str = "first_overlap";
    /// Step 8.
    pub const TEAMMATE: &str = "teammate";
}

/// The steps, in number order.
pub const STEPS: [Step; 8] = [
    Step {
        number: 1,
        id: id::SERVER,
        scope: Scope::Organisation,
        needs: &[],
        signals: &[Signal::ServerReady],
        get_started: true,
        early: false,
        title: "Your server",
        about: "Made when you signed up.",
        next_line: "your server is starting: zirc.dev says when it's ready.",
        agent_hint: None,
    },
    Step {
        number: 2,
        id: id::COMPUTER,
        scope: Scope::Person,
        needs: &[1],
        signals: &[Signal::Machine],
        get_started: true,
        early: false,
        title: "Your computer",
        about: "In your repository, on the computer your agents run on.",
        next_line: "in a repository you work in, run `zirc repo add`.",
        agent_hint: None,
    },
    Step {
        number: 3,
        id: id::AGENT,
        scope: Scope::Person,
        needs: &[2],
        signals: &[Signal::Agent],
        get_started: true,
        early: false,
        title: "Your agent",
        about: "Start Claude Code or Codex in that repository. It joins Zirc on its own.",
        next_line: "start your agent here (or restart it) and give it a task. It joins Zirc by itself.",
        agent_hint: None,
    },
    Step {
        number: 4,
        id: id::PHONE,
        scope: Scope::Person,
        needs: &[3],
        signals: &[Signal::Phone, Signal::AgentDirect],
        get_started: true,
        early: true,
        title: "Your phone",
        about: "Get the app now; this ticks off when your agent answers you from it.",
        next_line: "message your agent from the Zirc app: zirc.dev/app",
        agent_hint: Some(crate::hints::kind::NEXT_PHONE),
    },
    Step {
        number: 5,
        id: id::CI,
        scope: Scope::Organisation,
        needs: &[2],
        signals: &[Signal::Ci],
        get_started: true,
        early: false,
        title: "CI",
        about: "So your agent wakes when a build fails.",
        next_line: "connect CI, so your agent wakes when a build fails: `zirc ci connect`",
        agent_hint: Some(crate::hints::kind::NEXT_CI),
    },
    Step {
        number: 6,
        id: id::FIRST_WAKE,
        scope: Scope::Person,
        needs: &[3, 5],
        signals: &[Signal::Wake],
        get_started: true,
        early: false,
        title: "Your first wake",
        about: "See it happen, without waiting for a real failure.",
        next_line: "see a wake without waiting for a failure: `zirc ci test-failure`",
        agent_hint: Some(crate::hints::kind::NEXT_FIRST_WAKE),
    },
    Step {
        number: 7,
        id: id::FIRST_OVERLAP,
        scope: Scope::Person,
        needs: &[3],
        signals: &[Signal::Overlap],
        get_started: false,
        early: false,
        title: "Your first overlap",
        about: "Two agents changing the same files settle it between them.",
        next_line: "nothing to do: two of your agents changing the same files settle it between them.",
        agent_hint: None,
    },
    Step {
        number: 8,
        id: id::TEAMMATE,
        scope: Scope::Organisation,
        needs: &[1],
        signals: &[Signal::Teammate],
        get_started: false,
        early: false,
        title: "A teammate",
        about: "Teammates' agents hear yours.",
        next_line: "invite a teammate at zirc.dev: their agents hear yours.",
        agent_hint: None,
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

/// How zirc is installed on a computer: the installer, which every surface that says it reads.
pub const INSTALL_COMMAND: &str = "curl -fsSL https://zirc.dev/install | sh";

/// What connects a computer, run in a repository once zirc is installed: it signs the
/// computer in the first time, and watches the repository. Step 2's command.
pub const REPO_ADD_COMMAND: &str = "zirc repo add";

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
/// organisation's): the lowest-numbered one not reached that can be done now
/// ([`progress`]'s `next`).
#[must_use]
pub fn next(reached: &[u8]) -> Option<&'static Step> {
    progress(reached).next.and_then(step)
}

/// How a step stands for someone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "StepState")]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Reached.
    Done,
    /// Not reached, and it can be done now: what it needs is reached, or it's one that can
    /// be begun [`Step::early`].
    Available,
    /// Not reached, and it waits for the steps in [`StepProgress::after`].
    Waiting,
}

/// What a surface says of a step that's done.
pub const DONE_LABEL: &str = "Done";
/// What a surface says of the next step, while it waits for the person to do it.
pub const NEXT_LABEL: &str = "Waiting for it";
/// What a surface says of a step that can be done now and isn't the next.
pub const AVAILABLE_LABEL: &str = "Any time";

/// One step as it stands for someone: what every surface draws, deciding nothing itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct StepProgress {
    /// The step's number.
    pub number: u8,
    /// The step's id.
    pub id: String,
    /// How it stands.
    pub state: State,
    /// The steps it still waits for, by number, when it's [`State::Waiting`].
    pub after: Vec<u8>,
    /// Its state in words: `Done`, `Waiting for it` (the next), `Any time`, `After step 2`,
    /// `After steps 3 and 5`.
    pub label: String,
    /// Whether it's the next step: the one to highlight and suggest.
    pub next: bool,
}

/// Every step as it stands for someone, and which is next: what [`STEPS_PATH`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Progress {
    /// Each step, in number order.
    pub steps: Vec<StepProgress>,
    /// The next step's number: the lowest-numbered one not reached that can be done now.
    /// None once every step is reached.
    pub next: Option<u8>,
}

/// Where a surface reads its person's [`Progress`] in an organisation: `GET`, as one of the
/// member's apps or machines with its account (HTTP Basic), or in a browser as the signed-in
/// member of `?org=<slug>`.
pub const STEPS_PATH: &str = "/api/me/steps";

/// `After step 2`, `After steps 3 and 5`, `After steps 2, 3 and 5`.
fn after_label(after: &[u8]) -> String {
    let numbers: Vec<String> = after.iter().map(u8::to_string).collect();
    match numbers.split_last() {
        Some((last, [])) => format!("After step {last}"),
        Some((last, rest)) => format!("After steps {} and {last}", rest.join(", ")),
        None => AVAILABLE_LABEL.to_owned(),
    }
}

/// Every step as it stands for someone who has reached `reached` (numbers, theirs and their
/// organisation's), and which is next. The one place that decides it: the dashboard, the
/// command line, the agent's hint and the apps all show this.
#[must_use]
pub fn progress(reached: &[u8]) -> Progress {
    let mut next = None;
    let steps = STEPS
        .iter()
        .map(|step| {
            let after: Vec<u8> = step
                .needs
                .iter()
                .copied()
                .filter(|need| !reached.contains(need))
                .collect();
            let state = if reached.contains(&step.number) {
                State::Done
            } else if after.is_empty() || step.early {
                State::Available
            } else {
                State::Waiting
            };
            // The next is one whose needs are reached: one begun early isn't suggested
            // before the steps it can't finish without.
            let is_next = next.is_none() && state == State::Available && after.is_empty();
            if is_next {
                next = Some(step.number);
            }
            let label = match state {
                State::Done => DONE_LABEL.to_owned(),
                State::Available if is_next => NEXT_LABEL.to_owned(),
                State::Available => AVAILABLE_LABEL.to_owned(),
                State::Waiting => after_label(&after),
            };
            StepProgress {
                number: step.number,
                id: step.id.to_owned(),
                state,
                after: if state == State::Waiting {
                    after
                } else {
                    Vec::new()
                },
                label,
                next: is_next,
            }
        })
        .collect();
    Progress { steps, next }
}

/// [`progress`] for every set of steps reached, for the web app, which can't call it: each
/// distinct [`StepProgress`] once, and for each set (bit `n - 1` set when step `n` is
/// reached) the row of each step in number order.
fn progress_table() -> (Vec<StepProgress>, Vec<Vec<usize>>) {
    let mut rows: Vec<StepProgress> = Vec::new();
    let by_reached = (0..1_u16 << STEPS.len())
        .map(|set| {
            let reached: Vec<u8> = STEPS
                .iter()
                .map(|step| step.number)
                .filter(|number| set & (1 << (number - 1)) != 0)
                .collect();
            progress(&reached)
                .steps
                .into_iter()
                .map(|step| {
                    rows.iter().position(|row| *row == step).unwrap_or_else(|| {
                        rows.push(step);
                        rows.len() - 1
                    })
                })
                .collect()
        })
        .collect();
    (rows, by_reached)
}

/// What a person's own agents did today, for the card that invites a teammate: counts only,
/// from the organisation's activity store, as the dashboard's Today has them for the team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "AgentsToday")]
pub struct Today {
    /// How many times their agents' watches woke, in the last 24 hours.
    pub wakes: u32,
    /// How many builds that failed for their agents then landed, in the last 24 hours.
    pub fixed: u32,
}

/// Where a surface reads its person's [`Today`] in an organisation: `GET`, as
/// [`STEPS_PATH`] is asked.
pub const TODAY_PATH: &str = "/api/me/today";

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
    let (rows, by_reached) = progress_table();
    vec![
        Scope::decl(config),
        Signal::decl(config),
        Step::decl(config),
        format!("const STEPS: readonly OnboardingStep[] = {steps};"),
        format!("const STEPS_RECORDED_FROM = {RECORDED_FROM:?};"),
        format!("const INSTALL_COMMAND = {INSTALL_COMMAND:?};"),
        format!("const REPO_ADD_COMMAND = {REPO_ADD_COMMAND:?};"),
        format!("const NEXT_PREFIX = {NEXT_PREFIX:?};"),
        Today::decl(config),
        format!("const TODAY_PATH = {TODAY_PATH:?};"),
        format!("const ONBOARDING_APP_PAGE = {APP_PAGE:?};"),
        State::decl(config),
        StepProgress::decl(config),
        Progress::decl(config),
        format!("const STEPS_PATH = {STEPS_PATH:?};"),
        format!(
            "const STEP_PROGRESS_ROWS: readonly StepProgress[] = {};",
            serde_json::to_string(&rows).expect("progress serializes")
        ),
        format!(
            "const STEP_PROGRESS_BY_REACHED: readonly (readonly number[])[] = {};",
            serde_json::to_string(&by_reached).expect("progress serializes")
        ),
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
    fn every_step_says_what_to_do_next_in_a_line() {
        for step in &STEPS {
            assert!(
                !step.next_line.is_empty() && !step.next_line.contains('\n'),
                "{}",
                step.id
            );
        }
        assert!(
            step(5)
                .unwrap()
                .next_line
                .contains(crate::ci::CONNECT_COMMAND)
        );
        assert!(
            step(6)
                .unwrap()
                .next_line
                .contains(crate::ci::TEST_FAILURE_COMMAND)
        );
        assert!(step(4).unwrap().next_line.contains(APP_PAGE));
        // An agent has something to say of the phone, CI and a first wake, by kinds that are.
        let hinted: Vec<&str> = STEPS.iter().filter_map(|step| step.agent_hint).collect();
        assert_eq!(hinted.len(), 3);
        for kind in hinted {
            assert!(crate::hints::kind::ALL.contains(&kind), "{kind}");
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

    fn labels(reached: &[u8]) -> Vec<String> {
        progress(reached)
            .steps
            .into_iter()
            .filter(|step| super::step(step.number).is_some_and(|s| s.get_started))
            .map(|step| step.label)
            .collect()
    }

    #[test]
    fn each_steps_state_in_words() {
        // Just signed up: the server came with the account, and the phone's app can go on now.
        assert_eq!(
            labels(&[1]),
            [
                "Done",
                "Waiting for it",
                "After step 2",
                "Any time",
                "After step 2",
                "After steps 3 and 5"
            ]
        );
        // CI connected before the phone: it just ticks, the phone is next, the wake any time.
        assert_eq!(
            labels(&[1, 2, 3, 5]),
            ["Done", "Done", "Done", "Waiting for it", "Done", "Any time"]
        );
        // CI not heard yet: the wake waits for it alone.
        assert_eq!(
            labels(&[1, 2, 3, 4]),
            [
                "Done",
                "Done",
                "Done",
                "Done",
                "Waiting for it",
                "After step 5"
            ]
        );
        let all = progress(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(all.next, None);
        assert!(all.steps.iter().all(|step| step.state == State::Done));
    }

    #[test]
    fn the_next_is_never_one_begun_early() {
        // The phone is offered any time, but suggested only once an agent can answer there.
        let just_signed_up = progress(&[1]);
        assert_eq!(just_signed_up.next, Some(2));
        let phone = &just_signed_up.steps[3];
        assert_eq!((phone.state, phone.next), (State::Available, false));
        assert!(phone.after.is_empty());
    }

    #[test]
    fn the_web_apps_table_says_what_progress_says() {
        let (rows, by_reached) = progress_table();
        assert_eq!(by_reached.len(), 256);
        // Steps 1, 2 and 5 reached: bits 0, 1 and 4.
        let steps: Vec<StepProgress> = by_reached[0b1_0011]
            .iter()
            .map(|row| rows[*row].clone())
            .collect();
        assert_eq!(steps, progress(&[1, 2, 5]).steps);
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
