//! Getting started with zirc, declared once: the seven steps a person or an office takes
//! ([`STEPS`]: hire, sign up, install and seat, say hi, connect tools, phone, done, the user's
//! order of 27 Nov 2026), what each waits for, and the signals that prove each was done
//! ([`Signal`]). zirc.dev records each step the first time it's reached (its `step_reached`
//! event), from what it sees itself and from what the tenant's recorder saw
//! ([`First`], at [`FIRSTS_PATH`]); the apps and the dashboard show the next step from the
//! same list, through the web app's generated types (`bazel run //web:zirc_proto_types`).
//!
//! A step's number suggests an order, but a step waits only for what it [`Step::needs`]: one
//! done early ticks off all the same. The next step is the lowest-numbered one not reached
//! whose needs are.
//!
//! The apps show no prompt for them (the user's choice, 27 Nov 2026): zirc.dev's setup page
//! and the command line's `Next:` line do. What an office still has to connect is
//! [`missing`]'s to say, for setup's tools step and whenever a job needs a tool again.
//!
//! Metadata only: which signal, whose account and when, never what anyone said.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::coworkers::Staff;
use crate::inbound::Tool;

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
    /// zirc.dev: a second member joined the organisation. No step reads it since 27 Nov 2026.
    Teammate,
    /// zirc.dev: the office hired a coworker ([`crate::coworkers::Coworker`]).
    Hired,
    /// The recorder: a session on a member's machine sat at a coworker's desk
    /// ([`crate::activity::Seated`]).
    Seated,
    /// The recorder: a session at a coworker's desk was woken by a direct message from the
    /// person whose machine it runs on ([`crate::activity::Wake::wrote`]), which it answers.
    Answered,
    /// zirc.dev: the tools the office's jobs need were each heard, a test or a post
    /// ([`missing`] had nothing left but CI).
    Tools,
}

impl Signal {
    /// Every signal.
    pub const ALL: [Self; 13] = [
        Self::ServerReady,
        Self::Machine,
        Self::Agent,
        Self::Phone,
        Self::AgentDirect,
        Self::Ci,
        Self::Wake,
        Self::Overlap,
        Self::Teammate,
        Self::Hired,
        Self::Seated,
        Self::Answered,
        Self::Tools,
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
            Self::Hired => "hired",
            Self::Seated => "seated",
            Self::Answered => "answered",
            Self::Tools => "tools",
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
            Self::Agent
                | Self::AgentDirect
                | Self::Ci
                | Self::Wake
                | Self::Overlap
                | Self::Seated
                | Self::Answered
        )
    }

    /// The signal a row of the tenant's activity store gives, if any: its `kind`, whether its
    /// `account` is a machine's (`agent`), whether it's a `direct` message's, and its kept
    /// `data`. What the recorder keeps the first of ([`First`]), and what a backfill reads
    /// history with. A row gives one signal: a wake at a desk its person's message caused is
    /// [`Signal::Answered`], not [`Signal::Wake`] as well, which the recorder keeps from the
    /// first wake of another kind.
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
            crate::activity::WAKE_KIND if answered(data) => Some(Self::Answered),
            crate::activity::WAKE_KIND => Some(Self::Wake),
            crate::activity::SEATED_KIND => Some(Self::Seated),
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

/// Whether a wake's record says a session at a coworker's desk was woken by its own person's
/// direct message.
fn answered(data: Option<&serde_json::Value>) -> bool {
    data.and_then(|data| serde_json::from_value::<crate::activity::Wake>(data.clone()).ok())
        .is_some_and(|wake| {
            wake.desk.is_some() && wake.wrote == Some(crate::activity::Wrote::Owner)
        })
}

/// One step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(rename = "OnboardingStep")]
pub struct Step {
    /// Its number, from 1, which suggests the order.
    pub number: u8,
    /// Its id, for prompts about it and the apps' words: `hire`, `seat` and so on.
    pub id: &'static str,
    /// Whose it is.
    pub scope: Scope,
    /// The steps that must be reached before it can be done, by number.
    pub needs: &'static [u8],
    /// What proves it: every one of these, and it's reached when the last of them was seen.
    /// None for one that's reached once what it needs is ([`progress`]): done.
    pub signals: &'static [Signal],
    /// Whether the dashboard's Get started lists it; the others tick off when they happen.
    pub get_started: bool,
    /// Whether it can be begun before what it needs is reached, so it's offered any time: the
    /// phone's app installs while the computer is being set up.
    pub early: bool,
    /// What it's called, wherever it's listed or suggested. [`COWORKER`] stands for the
    /// coworker it's about, and [`SENTENCE`] for what seats it ([`words`] fills them in).
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

/// What a step's words write for the coworker they're about ([`words`]).
pub const COWORKER: &str = "{coworker}";

/// What a step's words write for the sentence that seats that coworker
/// ([`crate::coworkers::tell_a_session`]).
pub const SENTENCE: &str = "{sentence}";

/// What a step's words say of a coworker when nobody knows which: setup hires one.
pub const SOME_COWORKER: &str = "your coworker";

/// What the sentence that seats a coworker says for its name when nobody knows it.
pub const NO_NAME: &str = "<name>";

/// A step's `text` for the coworker shown as `coworker` ([`crate::coworkers::Coworker::shown`]),
/// or [`SOME_COWORKER`] when there's none to name: `Say hi to John`. zirc.dev, which runs no
/// Rust, says it again in `web/src/lib/onboarding.ts` (`stepWords`), from the generated
/// constants: keep the two alike.
#[must_use]
pub fn words(text: &str, coworker: Option<&str>) -> String {
    let sentence = crate::coworkers::tell_a_session(coworker.unwrap_or(NO_NAME));
    text.replace(SENTENCE, &sentence)
        .replace(COWORKER, coworker.unwrap_or(SOME_COWORKER))
}

/// Each step's id ([`Step::id`]), for whoever tells one step from another.
pub mod id {
    /// Step 1.
    pub const HIRE: &str = "hire";
    /// Step 2.
    pub const SIGN_UP: &str = "sign_up";
    /// Step 3.
    pub const SEAT: &str = "seat";
    /// Step 4.
    pub const SAY_HI: &str = "say_hi";
    /// Step 5.
    pub const TOOLS: &str = "tools";
    /// Step 6.
    pub const PHONE: &str = "phone";
    /// Step 7.
    pub const DONE: &str = "done";
}

/// The steps, in number order.
pub const STEPS: [Step; 7] = [
    Step {
        number: 1,
        id: id::HIRE,
        scope: Scope::Organisation,
        needs: &[],
        signals: &[Signal::Hired],
        get_started: true,
        early: false,
        title: "Hire your first coworker",
        about: "One is enough to start; hire more once they're at work.",
        next_line: "hire your first coworker at zirc.dev.",
        agent_hint: None,
    },
    Step {
        number: 2,
        id: id::SIGN_UP,
        scope: Scope::Organisation,
        needs: &[],
        signals: &[Signal::ServerReady],
        get_started: true,
        early: false,
        title: "Sign up your office",
        about: "Keeps your coworker and starts your office's server.",
        next_line: "your office's server is starting: zirc.dev says when it's ready.",
        agent_hint: None,
    },
    Step {
        number: 3,
        id: id::SEAT,
        scope: Scope::Person,
        needs: &[1, 2],
        signals: &[Signal::Machine, Signal::Seated],
        get_started: true,
        early: false,
        title: "Install Zirc and seat {coworker}",
        about: "In the repository {coworker} works in, install Zirc, then start a new Claude Code or Codex session for {coworker} alone.",
        next_line: "start a new Claude Code or Codex session here for {coworker}, and tell it: {sentence}",
        agent_hint: None,
    },
    Step {
        number: 4,
        id: id::SAY_HI,
        scope: Scope::Person,
        needs: &[3],
        signals: &[Signal::Answered],
        get_started: true,
        early: false,
        title: "Say hi to {coworker}",
        about: "Say hello, or give {coworker} something real to do: the session at the desk wakes and answers.",
        next_line: "say hi to {coworker} from zirc.dev or the Zirc app: the session at the desk wakes and answers.",
        agent_hint: None,
    },
    Step {
        number: 5,
        id: id::TOOLS,
        scope: Scope::Organisation,
        needs: &[3],
        signals: &[Signal::Ci, Signal::Tools],
        get_started: true,
        early: false,
        title: "Connect their tools",
        about: "So {coworker} wakes by itself when something needs them, not only when you write: CI first, for the whole office.",
        next_line: "connect CI, so your coworkers wake when a build fails: `zirc ci connect`",
        agent_hint: Some(crate::hints::kind::NEXT_CI),
    },
    Step {
        number: 6,
        id: id::PHONE,
        scope: Scope::Person,
        needs: &[3],
        signals: &[Signal::Phone],
        get_started: true,
        early: true,
        title: "Get the phone app",
        about: "Hear from {coworker} wherever you are. Skip it if you like.",
        next_line: "get the Zirc app on your phone: zirc.dev/app",
        agent_hint: Some(crate::hints::kind::NEXT_PHONE),
    },
    Step {
        number: 7,
        id: id::DONE,
        scope: Scope::Person,
        needs: &[1, 2, 3, 4, 5],
        signals: &[],
        get_started: true,
        early: false,
        title: "Done",
        about: "Your office is set up: this is its overview from now on.",
        next_line: "nothing left: your office is set up.",
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
/// computer in the first time, and watches the repository. Step 3's first command.
pub const REPO_ADD_COMMAND: &str = "zirc repo add";

/// From when zirc.dev records the seven steps as they're reached, in RFC 3339: one reached
/// before then is history, which zirc.dev's sweep and the backfill record as filled in, so the
/// funnel counts only those who went through getting started.
pub const RECORDED_FROM: &str = "2026-11-27T00:00:00Z";

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
    /// Why it isn't reached though it was begun, where zirc.dev can tell ([`Stall`]): never
    /// from [`progress`], which knows only what's reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stall: Option<Stall>,
}

/// One thing zirc.dev checked about a step that was begun and isn't reached, from what it
/// records itself: never a guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "StepCheck")]
pub struct Check {
    /// Whether it's as it should be.
    pub ok: bool,
    /// What was checked and how it stands, in a sentence.
    pub text: String,
}

/// A step begun and not reached after a while: what zirc.dev checked, the one command that
/// fixes it, and another step to do meanwhile. zirc.dev works it out, as only it holds the
/// facts, and every surface says these words: the dashboard under the step, the command line
/// and the agent with [`STEPS_PATH`]'s answer. A stalled step never just says it's waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "StepStall", optional_fields)]
pub struct Stall {
    /// What happened and what didn't, in a sentence.
    pub summary: String,
    /// What zirc.dev checked, in the order things happen: the first that isn't ok is why.
    pub checks: Vec<Check>,
    /// The one command that fixes it, where there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// Another step to do meanwhile, by number: the lowest-numbered other one that can be
    /// done now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meanwhile: Option<u8>,
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
            // A step nothing proves but what it needs, done, is reached with the last of them.
            let state = if reached.contains(&step.number)
                || (step.signals.is_empty() && after.is_empty())
            {
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
                stall: None,
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

/// The tools an office is offered beside those its jobs need: office-wide, each with a Skip,
/// whether or not a job needs it (the user's choice, 27 Nov 2026: Linear optional).
pub const OPTIONAL_TOOLS: [Tool; 1] = [Tool::Linear];

/// A job someone holds that no tool wakes yet: none of those it could be woken by has been
/// heard. It's kept all the same, and waits ([`missing`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct JobWaiting {
    /// The job, by name.
    pub job: String,
    /// The tools any one of which wakes it, in the order zirc.dev shows tools.
    pub tools: Vec<Tool>,
    /// What it says of itself meanwhile: `waiting for Sentry or PagerDuty`.
    pub words: String,
}

/// What an office still has to connect, as setup's tools step and a job that needs a tool
/// again both show it ([`missing`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Missing {
    /// Whether CI is still to connect: every office's, as any coworker waits for builds, and a
    /// fixer can't work without it (the user's words, 27 Nov 2026).
    pub ci: bool,
    /// Each held job no tool wakes yet.
    pub jobs: Vec<JobWaiting>,
    /// The [`OPTIONAL_TOOLS`] not heard yet, offered with a Skip and never waited for.
    pub optional: Vec<Tool>,
}

impl Missing {
    /// Whether nothing is still to connect, what's optional aside: setup's tools step is done
    /// ([`Signal::Tools`] and [`Signal::Ci`]).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.ci && self.jobs.is_empty()
    }
}

/// `waiting for Sentry or PagerDuty`.
fn waiting_for(tools: &[Tool]) -> String {
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    match names.split_last() {
        Some((last, [])) => format!("waiting for {last}"),
        Some((last, rest)) => format!("waiting for {} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// What `staff`'s office still has to connect, given whether its CI was heard and the tools
/// that were, a test or a post (a known tool is heard for the office, whichever job's address
/// it posted to: [`Tool::org_wide`]): the one rule for what an office is missing. CI always;
/// for each job someone holds, one of the tools that wake it, unless one was heard; and the
/// optional tools, offered. A job with no tools needs only CI. Setup's tools step reads it,
/// and so does every later hire whose job needs a tool the office hasn't connected.
#[must_use]
pub fn missing(staff: &Staff, ci_heard: bool, heard: &[Tool]) -> Missing {
    let jobs = staff
        .jobs
        .iter()
        .filter(|job| staff.holders(&job.name).next().is_some())
        .filter_map(|job| {
            let tools = job.tools();
            (!tools.is_empty() && !tools.iter().any(|tool| heard.contains(tool))).then(|| {
                JobWaiting {
                    job: job.name.clone(),
                    words: waiting_for(&tools),
                    tools,
                }
            })
        })
        .collect();
    Missing {
        ci: !ci_heard,
        jobs,
        optional: OPTIONAL_TOOLS
            .into_iter()
            .filter(|tool| !heard.contains(tool))
            .collect(),
    }
}

/// zirc.dev's page that invites someone to the organisation an app is signed in to, given
/// its server as `?host=`: it finds the organisation and goes on to its people's invites, or
/// says who may invite. A copy, as the web app's routes are its folders: the page is
/// `web/src/app/invite/page.tsx`, which names this.
pub const INVITE_PATH: &str = "/invite";

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

/// What an app sends to [`NOTES_PATH`] (`POST`) to mark one note seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct NoteSeen {
    /// The note.
    pub note: Note,
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
        format!("const DONE_LABEL = {DONE_LABEL:?};"),
        format!("const NEXT_LABEL = {NEXT_LABEL:?};"),
        format!("const AVAILABLE_LABEL = {AVAILABLE_LABEL:?};"),
        format!("const ONBOARDING_APP_PAGE = {APP_PAGE:?};"),
        format!("const STEP_COWORKER = {COWORKER:?};"),
        format!("const STEP_SENTENCE = {SENTENCE:?};"),
        format!("const STEP_SOME_COWORKER = {SOME_COWORKER:?};"),
        format!("const STEP_NO_NAME = {NO_NAME:?};"),
        format!(
            "const STEP_SENTENCE_WORDS = {:?};",
            crate::coworkers::tell_a_session(COWORKER)
        ),
        Missing::decl(config),
        JobWaiting::decl(config),
        format!(
            "const OPTIONAL_TOOLS: readonly InboundTool[] = [{}];",
            OPTIONAL_TOOLS
                .map(|tool| format!("{:?}", tool.as_str()))
                .join(", ")
        ),
        format!(
            "const STEP_ID = {{ {} }} as const;",
            [
                ("HIRE", id::HIRE),
                ("SIGN_UP", id::SIGN_UP),
                ("SEAT", id::SEAT),
                ("SAY_HI", id::SAY_HI),
                ("TOOLS", id::TOOLS),
                ("PHONE", id::PHONE),
                ("DONE", id::DONE),
            ]
            .map(|(name, id)| format!("{name}: {id:?}"))
            .join(", ")
        ),
        State::decl(config),
        Check::decl(config),
        Stall::decl(config),
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
        NoteSeen::decl(config),
        First::decl(config),
        format!("const FIRSTS_PATH = {FIRSTS_PATH:?};"),
        Firsts::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coworkers::{Coworker, Job};
    use crate::role_report::Subject;
    use serde_json::json;

    #[test]
    fn steps_are_numbered_in_order_and_need_only_earlier_ones() {
        for (at, step) in STEPS.iter().enumerate() {
            assert_eq!(usize::from(step.number), at + 1, "{}", step.id);
            // Only done is proved by what it needs alone.
            assert_eq!(step.signals.is_empty(), step.id == id::DONE, "{}", step.id);
            for need in step.needs {
                assert!(*need < step.number, "{} needs {need}", step.id);
            }
        }
        let ids: Vec<&str> = STEPS.iter().map(|step| step.id).collect();
        assert_eq!(
            ids,
            [
                "hire", "sign_up", "seat", "say_hi", "tools", "phone", "done"
            ]
        );
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
        let by = |id| STEPS.iter().find(|step| step.id == id).unwrap();
        assert!(by(id::TOOLS).next_line.contains(crate::ci::CONNECT_COMMAND));
        assert!(by(id::PHONE).next_line.contains(APP_PAGE));
        // An agent has something to say of CI and the phone, by kinds that are.
        let hinted: Vec<&str> = STEPS.iter().filter_map(|step| step.agent_hint).collect();
        assert_eq!(hinted.len(), 2);
        for kind in hinted {
            assert!(crate::hints::kind::ALL.contains(&kind), "{kind}");
        }
    }

    #[test]
    fn a_steps_words_name_the_coworker_and_the_sentence_that_seats_it() {
        let seat = step(3).unwrap();
        assert_eq!(
            words(seat.title, Some("John")),
            "Install Zirc and seat John"
        );
        assert_eq!(
            words(seat.next_line, Some("John")),
            "start a new Claude Code or Codex session here for John, and tell it: You're John. \
             Take your desk."
        );
        assert_eq!(
            words(step(4).unwrap().title, None),
            "Say hi to your coworker"
        );
        for step in &STEPS {
            for text in [step.title, step.about, step.next_line] {
                let said = words(text, Some("John"));
                assert!(!said.contains('{') && !said.contains('}'), "{said}");
            }
        }
    }

    #[test]
    fn next_is_the_lowest_whose_needs_are_met() {
        assert_eq!(next(&[]).map(|s| s.id), Some("hire"));
        assert_eq!(next(&[1]).map(|s| s.id), Some("sign_up"));
        assert_eq!(next(&[1, 2]).map(|s| s.id), Some("seat"));
        // CI connected before the hello: it just ticks, and saying hi is still next.
        assert_eq!(next(&[1, 2, 3, 5]).map(|s| s.id), Some("say_hi"));
        assert_eq!(next(&[1, 2, 3, 4]).map(|s| s.id), Some("tools"));
        // The phone is suggested last, and can be skipped: done doesn't wait for it.
        assert_eq!(next(&[1, 2, 3, 4, 5]).map(|s| s.id), Some("phone"));
        assert_eq!(next(&[1, 2, 3, 4, 5, 6]), None);
    }

    fn labels(reached: &[u8]) -> Vec<String> {
        progress(reached)
            .steps
            .into_iter()
            .map(|step| step.label)
            .collect()
    }

    #[test]
    fn each_steps_state_in_words() {
        // Hired and signed up: seating is next, the phone's app can go on now.
        assert_eq!(
            labels(&[1, 2]),
            [
                "Done",
                "Done",
                "Waiting for it",
                "After step 3",
                "After step 3",
                "Any time",
                "After steps 3, 4 and 5"
            ]
        );
        // Everything but the phone: done is done, and the phone still offered.
        assert_eq!(
            labels(&[1, 2, 3, 4, 5]),
            [
                "Done",
                "Done",
                "Done",
                "Done",
                "Done",
                "Waiting for it",
                "Done"
            ]
        );
        let all = progress(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(all.next, None);
        assert!(all.steps.iter().all(|step| step.state == State::Done));
    }

    #[test]
    fn the_next_is_never_one_begun_early() {
        let signed_up = progress(&[1, 2]);
        assert_eq!(signed_up.next, Some(3));
        let phone = &signed_up.steps[5];
        assert_eq!((phone.state, phone.next), (State::Available, false));
        assert!(phone.after.is_empty());
    }

    #[test]
    fn the_web_apps_table_says_what_progress_says() {
        let (rows, by_reached) = progress_table();
        assert_eq!(by_reached.len(), 128);
        // Steps 1, 2 and 5 reached: bits 0, 1 and 4.
        let steps: Vec<StepProgress> = by_reached[0b1_0011]
            .iter()
            .map(|row| rows[*row].clone())
            .collect();
        assert_eq!(steps, progress(&[1, 2, 5]).steps);
    }

    #[test]
    fn a_step_is_reached_when_the_last_of_its_signals_was() {
        let seat = step(3).unwrap();
        let seen = |machine_at, seated_at| {
            move |signal| match signal {
                Signal::Machine => machine_at,
                Signal::Seated => seated_at,
                _ => None,
            }
        };
        assert_eq!(seat.reached_at(seen(Some(3), Some(5))), Some(5));
        assert_eq!(seat.reached_at(seen(Some(7), Some(5))), Some(7));
        assert_eq!(seat.reached_at(seen(Some(7), None)), None);
        // Done is never reached by a signal: progress says it, from what it needs.
        assert_eq!(step(7).unwrap().reached_at(|_| Some(1)), None);
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
        let machine = Some("ada-mbp");
        assert_eq!(
            of(crate::activity::WATCHING_KIND, machine, true, false, None),
            Some(Signal::Agent)
        );
        assert_eq!(
            of("intent", machine, true, false, None),
            Some(Signal::Agent)
        );
        assert_eq!(
            of("text", machine, true, true, None),
            Some(Signal::AgentDirect)
        );
        assert_eq!(of("text", Some("ada"), false, true, None), None);
        assert_eq!(
            of(crate::activity::WAKE_KIND, machine, true, false, None),
            Some(Signal::Wake)
        );
        let overlap = Some(json!({"what": "overlap"}));
        assert_eq!(
            of(crate::activity::NOTICE_KIND, machine, true, false, overlap),
            Some(Signal::Overlap)
        );
        let ahead = Some(json!({"what": "conflict-ahead"}));
        assert_eq!(
            of(crate::activity::NOTICE_KIND, machine, true, false, ahead),
            None
        );
        assert_eq!(of("working", machine, true, false, None), None);
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
            of(crate::ci::LANDING_KIND, machine, true, false, None),
            Some(Signal::Agent)
        );
    }

    #[test]
    fn a_desk_taken_and_a_hello_answered_are_signals_of_their_own() {
        let of = |kind, data: serde_json::Value| {
            Signal::of_row(kind, Some("ada-mbp"), true, false, Some(&data))
        };
        let seated = json!({"desk": "john"});
        assert_eq!(
            of(crate::activity::SEATED_KIND, seated),
            Some(Signal::Seated)
        );
        let wake = |desk: Option<&str>, wrote: Option<&str>| {
            json!({
                "id": "w1",
                "why": "direct",
                "kind": "text",
                "desk": desk,
                "wrote": wrote,
            })
        };
        let signal_of = |data| {
            Signal::of_row(
                crate::activity::WAKE_KIND,
                Some("ada-mbp"),
                true,
                false,
                Some(&data),
            )
        };
        let shape =
            serde_json::from_value::<crate::activity::Wake>(wake(Some("john"), Some("owner")));
        assert!(shape.is_ok(), "{shape:?}");
        assert_eq!(
            signal_of(wake(Some("john"), Some("owner"))),
            Some(Signal::Answered)
        );
        // Another member writing, or a wake for the job, is a wake like any.
        assert_eq!(
            signal_of(wake(Some("john"), Some("member"))),
            Some(Signal::Wake)
        );
        assert_eq!(signal_of(wake(Some("john"), None)), Some(Signal::Wake));
        assert_eq!(signal_of(wake(None, None)), Some(Signal::Wake));
    }

    fn staff(jobs: &[(&str, &[Subject])], held: &[&str]) -> Staff {
        Staff {
            coworkers: held
                .iter()
                .map(|job| Coworker {
                    name: format!("{job}_holder"),
                    shown_name: None,
                    jobs: vec![(*job).to_owned()],
                    repository: None,
                    picture: None,
                    hue: None,
                    added_by: None,
                })
                .collect(),
            jobs: jobs
                .iter()
                .map(|(name, reports)| Job {
                    name: (*name).to_owned(),
                    brief: "Does it.".to_owned(),
                    triggers: Vec::new(),
                    reports: reports.to_vec(),
                })
                .collect(),
        }
    }

    #[test]
    fn an_office_misses_ci_and_a_tool_for_each_held_job_that_needs_one() {
        let fixer = ("fixer", &[Subject::Flaky][..]);
        let oncall = ("oncall", &[Subject::Error, Subject::Incident][..]);
        // A fixer needs CI alone; Linear is offered beside, never waited for.
        let office = staff(&[fixer, oncall], &["fixer"]);
        let missing_now = missing(&office, false, &[]);
        assert!(missing_now.ci && missing_now.jobs.is_empty());
        assert_eq!(missing_now.optional, [Tool::Linear]);
        assert!(!missing_now.is_empty());
        assert!(missing(&office, true, &[]).is_empty());
        // On-call held: it waits for either of its tools until one is heard.
        let office = staff(&[fixer, oncall], &["fixer", "oncall"]);
        let waits = missing(&office, true, &[]);
        assert_eq!(
            waits.jobs,
            [JobWaiting {
                job: "oncall".to_owned(),
                tools: vec![Tool::Sentry, Tool::Pagerduty],
                words: "waiting for Sentry or PagerDuty".to_owned(),
            }]
        );
        let heard = missing(&office, true, &[Tool::Pagerduty, Tool::Linear]);
        assert!(heard.is_empty() && heard.optional.is_empty());
    }
}
