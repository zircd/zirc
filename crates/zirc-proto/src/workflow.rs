//! A workflow: a process a team declares once, in a file, and zirc runs step by step, routing
//! each step to the agent session or person who does it, in chat where everyone sees it.
//!
//! A workflow is a TOML file, kept in a repository (`.zirc/workflows/<name>.toml`) and sent to
//! the server with `zirc workflow deploy`, which posts it to the organisation's
//! [`CHANNEL`] as a [`Deploy`]: `x-zirc/workflow`, a [`crate::Event::Custom`] whose data
//! carries the file as written. The server's sidecar checks it, makes the workflow's own user,
//! [`Workflow::nick`], and runs it: each run is a card the nick keeps up to date in
//! [`Workflow::channel`] ([`crate::run`]).
//!
//! ```toml
//! name = "master"
//! nick = "master_bot"
//! channel = "#ops"
//!
//! [[input]]
//! name = "owner"
//!
//! [[on]]
//! state = "master"
//! level = "broken"
//! inputs = { owner = "owner", why = "why" }
//!
//! [[step]]
//! id = "fix"
//! who = "inputs.owner"
//! says = "Master is broken ({why}): fix it or revert your landing."
//! outputs = ["commit"]
//! fallback = "fixers"
//! nudge = "15m"
//! escalate = "30m"
//! stages = [
//!   { name = "seen", on = "said" },
//!   { name = "working", on = "push" },
//!   { name = "landing", on = "landing-queued" },
//!   { name = "landed", on = "landed" },
//! ]
//!
//! [[step]]
//! id = "green"
//! kind = "wait"
//! until = { state = "master", level = "ok" }
//! ```
//!
//! This module holds the file's shape and the rules a file must keep that need nothing but
//! the file ([`Workflow::check`]); zirc-core reads the TOML and runs it.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;
use crate::state::Level;

/// The custom event kind a workflow is deployed as.
pub const KIND: &str = "x-zirc/workflow";

/// The organisation's channel workflows are deployed to, where the sidecar reads them.
pub const CHANNEL: &str = "#workflows";

/// The nick the server's host of workflows answers deploys as, in [`CHANNEL`]: one of zirc's
/// own ([`crate::names::RESERVED_INTEGRATION_NICKS`]), so no team's hook posts as it.
pub const HOST: &str = "zirc-workflows";

/// The longest a workflow's file is, in bytes: its deploy is one multiline message.
pub const MAX_SOURCE: usize = 8192;

/// The most steps a workflow has.
pub const MAX_STEPS: usize = 32;

/// The longest a workflow's name, a step's id, an input's or a role's name is.
pub const MAX_NAME: usize = 32;

/// The most channels a workflow follows from the start.
pub const MAX_FOLLOW: usize = 16;

/// Whether `channel` is a channel's name.
fn is_channel(channel: &str) -> bool {
    channel.len() > 1 && channel.starts_with('#') && !channel.contains([' ', ','])
}

/// The longest a step's `says` is, in characters.
pub const MAX_SAYS: usize = 1000;

/// Where a server's sidecar lists the workflows it runs ([`Hosting`]), for zirc.dev's dashboard.
pub const LIST_PATH: &str = "/v1/workflows";

/// The workflows a server runs, as [`LIST_PATH`] answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "WorkflowHosting")]
pub struct Hosting {
    /// Whether the server runs workflows at all: zirc.dev turns them on for an organisation.
    pub on: bool,
    /// Those deployed, by name.
    pub workflows: Vec<Listed>,
    /// The last deploy of each name the server didn't take, until one of that name is.
    #[serde(default)]
    pub refused: Vec<Refused>,
}

/// A deploy the server didn't take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "WorkflowRefused")]
pub struct Refused {
    /// The workflow's name, as the file said it.
    pub name: String,
    /// Which file it was ([`Listed::version`]).
    pub version: String,
    /// Why not.
    pub why: String,
}

/// A deploy from zirc.dev's dashboard, as it POSTs it to [`LIST_PATH`]: the server checks the
/// file and says it in [`CHANNEL`] as a [`Deploy`] of `by`'s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "WorkflowDeploying")]
pub struct Deploying {
    /// The file.
    pub source: String,
    /// The account of the person signed in at zirc.dev, an owner or admin of the organisation.
    pub by: String,
}

/// What [`LIST_PATH`] answers a [`Deploying`] it said in [`CHANNEL`]: whether the server took
/// it shows in its list, as this version of the workflow or among those refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "WorkflowSent")]
pub struct Sent {
    /// The workflow's name, as the file says it.
    pub name: String,
    /// Which file it is ([`Listed::version`]).
    pub version: String,
}

/// A workflow a server runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "WorkflowListed")]
pub struct Listed {
    /// Its name.
    pub name: String,
    /// The user it runs as.
    pub nick: String,
    /// What it's for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where its runs are posted.
    pub channel: String,
    /// Which of its files this is: a short hash of the file.
    pub version: String,
    /// Who first deployed it: they, an owner or an admin may replace it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The file, as deployed.
    pub source: String,
    /// Its runs that aren't finished, oldest first.
    pub runs: Vec<Going>,
    /// Its runs that have ended, the latest first, at most [`MAX_ENDED`]: those its nick
    /// has seen end, or read back from its channel's history. Always sent, even empty, so
    /// that its absence says the server is older and doesn't list them.
    #[serde(default)]
    #[ts(as = "Option<Vec<Ended>>", optional)]
    pub ended: Vec<Ended>,
}

/// How many of a workflow's ended runs [`Listed::ended`] lists.
pub const MAX_ENDED: usize = 20;

/// Where a workflow's ended runs are read back from the server's history a page at a time,
/// the latest first, at most [`MAX_ENDED`] a page: `{name}` is the workflow's, and
/// `?before=` a page's [`Runs::before`] for the page after it.
pub const RUNS_PATH: &str = "/v1/workflows/{name}/runs";

/// A page of a workflow's ended runs, as [`RUNS_PATH`] answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "WorkflowRuns")]
pub struct Runs {
    /// The runs, the latest first.
    pub runs: Vec<Ended>,
    /// Where the page of older runs starts, when there are more: what `?before=` takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// A run that has ended, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "WorkflowEnded")]
pub struct Ended {
    /// The run, such as `master-12`.
    pub run: String,
    /// How it ended.
    pub outcome: crate::run::Outcome,
    /// Why, in a few words, when it said: for a run that failed at `step`, why that step
    /// failed. A run that ended before the step was sent apart names it here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The step it failed at, when it failed at one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// What it was given.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(as = "Option<BTreeMap<String, String>>", optional)]
    pub inputs: BTreeMap<String, String>,
    /// What its steps handed back, a later step's over an earlier's of the same name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(as = "Option<BTreeMap<String, String>>", optional)]
    pub outputs: BTreeMap<String, String>,
    /// When it started, in unix seconds; unknown for a run read back from history that
    /// doesn't go back as far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
    /// When it ended, in unix seconds.
    pub ended: u64,
}

/// A run that isn't finished, and where it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "WorkflowRun")]
pub struct Going {
    /// The run, such as `master-12`.
    pub run: String,
    /// The step it's on.
    pub step: String,
    /// Who has that step: whoever took it on, or else everyone it went to. Nobody for a wait.
    pub to: Vec<String>,
    /// The stage a task is at, among its step's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// When the step last moved or its holder was last heard of, in unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// What it was given.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(as = "Option<BTreeMap<String, String>>", optional)]
    pub inputs: BTreeMap<String, String>,
    /// When it started, in unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
}

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Hosting::decl(config),
        Listed::decl(config),
        Going::decl(config),
        Ended::decl(config),
        Runs::decl(config),
        crate::run::Outcome::decl(config),
        Refused::decl(config),
        Deploying::decl(config),
        Sent::decl(config),
        format!("const WORKFLOW_LIST_PATH = {LIST_PATH:?};"),
        format!("const WORKFLOW_MAX_SOURCE = {MAX_SOURCE};"),
        format!("const WORKFLOW_MAX_ENDED = {MAX_ENDED};"),
        format!("const WORKFLOW_RUNS_PATH = {RUNS_PATH:?};"),
        Workflow::decl(config),
        Input::decl(config),
        OneFails::decl(config),
        Trigger::decl(config),
        Step::decl(config),
        StepKind::decl(config),
        Each::decl(config),
        Stage::decl(config),
        Signal::decl(config),
        Until::decl(config),
        Level::decl(config),
        // What a builder of the file offers, as the rules here have it.
        list("WORKFLOW_STATE_FIELDS", &STATE_FIELDS),
        record("WORKFLOW_HINTS", HINTS),
        list("WORKFLOW_SAID_FIELDS", &SAID_FIELDS),
        list("WORKFLOW_REACTION_FIELDS", &REACTION_FIELDS),
        list("WORKFLOW_LANDED_FIELDS", &LANDED_FIELDS),
        list("WORKFLOW_DAYS", &DAYS),
        format!("const WORKFLOW_MAX_STEPS = {MAX_STEPS};"),
        format!("const WORKFLOW_MAX_EACH = {MAX_EACH};"),
        format!("const WORKFLOW_EACH = {EACH:?};"),
    ]
}

/// What a builder of a workflow's file says of each part of it, for whoever is new to them:
/// each section (`section.<name>`), the comments a file opens with (`notes`), and each field,
/// by its key in the file, a step's as `step.<key>` and a trigger's as `on.<key>`. A few words
/// each, as a hint beside its label.
pub const HINTS: &[(&str, &str)] = &[
    (
        "section.workflow",
        "Who it runs as, where it posts and listens, and how its runs share it.",
    ),
    (
        "section.input",
        "What each run is given when it starts. Steps read them as {name}.",
    ),
    (
        "section.roles",
        "Named groups of people and agent sessions that steps go to, or fall back to.",
    ),
    (
        "section.on",
        "What starts a run. Naming its nick always does; each trigger starts one on its own.",
    ),
    (
        "section.step",
        "What a run does, in order. Each step goes to someone, waits or posts, and the run \
         moves on once it's done.",
    ),
    (
        "notes",
        "The comments at the top of its file: what to know before changing it.",
    ),
    (
        "name",
        "Lowercase letters, digits and -. Deploying the same name replaces it.",
    ),
    (
        "description",
        "What it's for, in a few words, shown on its card.",
    ),
    (
        "nick",
        "The user it runs as. Mentioning it starts a run: @nick run <input>=<value>.",
    ),
    (
        "channel",
        "Where each run's card is posted, and kept up to date as it goes.",
    ),
    (
        "follow",
        "More channels it listens in for its triggers. Inviting it to one adds it too.",
    ),
    (
        "one_run_per",
        "While a run given this input's value is going, starting another with the same \
         value starts none.",
    ),
    (
        "when_one_fails",
        "For a step given to several at once: whether the others carry on when one fails, \
         or the run stops.",
    ),
    (
        "on.kind",
        "A state being said, a time coming round, words said or a reaction where it \
         listens, or a landing that changes some paths.",
    ),
    (
        "on.state",
        "A state the team says, such as master, and the level that starts a run.",
    ),
    (
        "on.every",
        "An interval such as 1d or 1w, on a day and at a time in UTC if you like.",
    ),
    (
        "on.said",
        "Words a person or an integration says where it listens.",
    ),
    (
        "on.reaction",
        "A reaction a person puts on a message where it listens.",
    ),
    (
        "on.landed",
        "Paths a landing changes: a directory covers everything under it.",
    ),
    (
        "on.inputs",
        "What each run it starts is given, from what started it.",
    ),
    (
        "step.kind",
        "A task is work handed back with outputs; an approve, a person's 👍 or 👎; a wait \
         holds until a state is said; a post says something and goes on.",
    ),
    (
        "step.id",
        "How later steps refer to it: {id.output} reads what it handed back.",
    ),
    (
        "step.who",
        "inputs.<name> for whoever an input names, role:<name> for a role, or a nick. \
         Several at once each get a run of their own.",
    ),
    ("step.approver", "The person whose 👍 or 👎 it waits for."),
    (
        "step.until",
        "The state it waits for, and the level that lets the run go on.",
    ),
    (
        "step.when",
        "A condition on inputs and earlier outputs, such as fix.cause != flaky. Without \
         one, it always runs.",
    ),
    (
        "step.says",
        "What it tells whoever does it. {input} and {step.output} are filled in.",
    ),
    (
        "step.outputs",
        "What it hands back, by name: later steps read them as {id.name}.",
    ),
    (
        "step.irreversible",
        "A yes to what can't be undone counts only after a moment, in which a no still \
         stops it.",
    ),
    (
        "step.stages",
        "Progress the workflow follows from what its session does, such as pushing or \
         landing, without its saying so.",
    ),
    (
        "step.quiet",
        "Quiet past its nudge, whoever has it is asked how it goes; past that, it's handed \
         to the fallback role, or the run fails.",
    ),
];

/// `pairs` as a TypeScript record named `name`.
fn record(name: &str, pairs: &[(&str, &str)]) -> String {
    let pairs: Vec<String> = pairs.iter().map(|(k, v)| format!("{k:?}: {v:?}")).collect();
    format!(
        "const {name}: Readonly<Record<string, string>> = {{ {} }};",
        pairs.join(", ")
    )
}

/// `values` as a TypeScript constant named `name`.
fn list(name: &str, values: &[&str]) -> String {
    let values: Vec<String> = values.iter().map(|v| format!("{v:?}")).collect();
    format!("const {name}: readonly string[] = [{}];", values.join(", "))
}

/// A workflow, as its file says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields, rename = "Workflow")]
pub struct Workflow {
    /// Its name: lowercase letters, digits and `-`, such as `master`. A deploy of the same
    /// name replaces it.
    pub name: String,
    /// The user it runs as, which people mention to start it, such as `master_bot`: any
    /// account name ([`crate::names::is_account_name`]) that isn't zirc's own or an
    /// integration's, and that no person or machine has.
    pub nick: String,
    /// What it's for, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where its runs are posted and where it listens, such as `#ops`.
    pub channel: String,
    /// Other channels it listens in from the start, for what its workers do there, such as
    /// `#monorepo/landings`. Anyone in the organisation may also invite it elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow: Vec<String>,
    /// What a run is given when it starts.
    #[serde(default, rename = "input", skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    /// The input that tells runs apart, such as `test`: a run asked for while another with
    /// the same value of it goes on doesn't start, so CI saying a flaky test failed again
    /// while it's being fixed starts nothing new. Without it, every mention starts a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_run_per: Option<String>,
    /// What becomes of a step given to several at once (`each`) when one of them fails or
    /// runs out of time: the others carry on, and the step fails once all have ended, when
    /// it doesn't say; or the run stops at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_one_fails: Option<OneFails>,
    /// Each role's people: who a step falls back to (`fallback = "<role>"`) when its time is
    /// out, by handle, machine account or `machine/session`, or `role:<name>` for one of that
    /// role's live members (the sessions that joined it with `zirc role join`); the people
    /// after a `role:` stand in for it only while it has nobody, as in
    /// `fixers = ["role:master-fixers", "mattias"]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, Vec<String>>,
    /// What starts a run on its own, besides a mention.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<Trigger>,
    /// Its steps, run in order.
    #[serde(rename = "step")]
    pub steps: Vec<Step>,
}

/// What becomes of a step given to several at once when one of them fails
/// ([`Workflow::when_one_fails`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "WorkflowOneFails")]
pub enum OneFails {
    /// The others carry on; the step fails once all have ended.
    #[default]
    CarryOn,
    /// The run fails at once, and the others' work on it is called off.
    Stop,
}

/// Whom a step goes to, every one of them at once, each doing it on their own ([`Step::each`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(untagged)]
#[ts(rename = "WorkflowEach")]
pub enum Each {
    /// `inputs.<name>`: everyone the input names, separated by spaces or commas.
    Input(String),
    /// Those the file names: nicks, `machine/session`s, or `role:<name>` for one of a role's
    /// live members.
    Named(Vec<String>),
}

/// The most a step goes to at once.
pub const MAX_EACH: usize = 8;

/// The name a step given to several reads whom this one is by, as `{each}` in its `says`:
/// no input may have it.
pub const EACH: &str = "each";

/// Something a run is given when it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields, rename = "WorkflowInput")]
pub struct Input {
    /// Its name, which steps read as `{name}`.
    pub name: String,
    /// What it is, for whoever starts a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// Whether a run may start without it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// What starts a run on its own, each trigger one of: a state ([`crate::state`]) being said, a
/// time coming round (`every`), words someone says where the workflow listens (`said`), a
/// person's reaction to a message there (`reaction`), or a landing that changes some paths
/// (`landed`). What it knows of that fills the run's inputs (`inputs`), but a time, which
/// knows nothing and is given them (`with`).
///
/// ```toml
/// [[on]]
/// state = "master"
/// level = "broken"
/// inputs = { owner = "owner" }
///
/// [[on]]
/// every = "7d"          # at least [`MIN_EVERY`]
/// day = "mon"           # with 7d alone; Thursdays without it
/// at = "09:00"          # UTC, when `every` is whole days; midnight without it
/// with = { what = "update dependencies" }
///
/// [[on]]
/// said = "refund"       # a person or an integration says it, as words of its own
/// inputs = { ask = "text", who = "by" }   # of [`SAID_FIELDS`]
///
/// [[on]]
/// reaction = "🐛"        # a person's, on a message the workflow heard
/// inputs = { report = "text", from = "author" }   # of [`REACTION_FIELDS`]
///
/// [[on]]
/// landed = ["crates/zirc", "MODULE.bazel"]   # files, or everything under a directory
/// inputs = { commit = "commit", by = "session" } # of [`LANDED_FIELDS`]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields, rename = "WorkflowTrigger")]
pub struct Trigger {
    /// The state's name, such as `master`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The level that starts a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    /// Which input each thing the trigger knows fills: `input = "field"`, where the field
    /// is one its kind has ([`TriggerKind::fields`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// Words that start a run when a person or an integration says them where the workflow
    /// listens, whatever their case, as words of their own: `refund` isn't in `refunded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// The reaction that starts a run when a person gives it to a message the workflow
    /// heard, such as `🐛`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<String>,
    /// Paths a landing changes to start a run: a file, or a directory for everything under
    /// it; `.` for any landing. As CI says what a landing changed ([`crate::ci::Data`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub landed: Vec<String>,
    /// How often a run starts by itself, such as `1d` ([`seconds`]): at every whole multiple
    /// of it since 1 January 1970 UTC, so `1d` is each midnight and `6h` is 0, 6, 12 and 18.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// The time of day it starts at, `HH:MM` in UTC, when `every` is whole days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The day of the week it starts on, `mon` to `sun`, when `every` is `7d`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    /// What a run started by `every` is given: `input = "value"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub with: BTreeMap<String, String>,
}

/// What a trigger is, by which of its fields it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// `state`.
    State,
    /// `every`.
    Every,
    /// `said`.
    Said,
    /// `reaction`.
    Reaction,
    /// `landed`.
    Landed,
}

/// What a `said` trigger can pass on as inputs: the message's text, who said it, where, and
/// its id on the server.
pub const SAID_FIELDS: [&str; 4] = ["text", "by", "channel", "id"];

/// What a `reaction` trigger can pass on: the message's text, who wrote it, who reacted,
/// where, and the message's id.
pub const REACTION_FIELDS: [&str; 5] = ["text", "author", "by", "channel", "id"];

/// What a `landed` trigger can pass on: the landing's commit, branch, the session that
/// queued it, its build, the build's page and the commit's subject.
pub const LANDED_FIELDS: [&str; 6] = ["commit", "branch", "session", "build", "link", "subject"];

/// The longest `said` is, in characters.
pub const MAX_SAID: usize = 80;

/// The most paths `landed` names.
pub const MAX_LANDED: usize = 16;

impl TriggerKind {
    /// The fields its `inputs` may read; none for a time, which is given `with`.
    #[must_use]
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Self::State => &STATE_FIELDS,
            Self::Every => &[],
            Self::Said => &SAID_FIELDS,
            Self::Reaction => &REACTION_FIELDS,
            Self::Landed => &LANDED_FIELDS,
        }
    }

    /// The field of a trigger that makes it this kind, as a file writes it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Every => "every",
            Self::Said => "said",
            Self::Reaction => "reaction",
            Self::Landed => "landed",
        }
    }
}

/// Whether a landing that changed `changed` starts a run of a trigger on `paths`: a changed
/// file is one of them or under one; `.` is any landing. A changed path ending in `/` is a
/// directory CI named in place of the many files it changed there: anything in it may have.
#[must_use]
pub fn landed_under(paths: &[String], changed: &[String]) -> bool {
    let under = |path: &str, dir: &str| {
        path.strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || dir.ends_with('/'))
    };
    paths.iter().any(|path| {
        let path = path.trim_end_matches('/');
        path == "."
            || changed.iter().any(|c| {
                under(c.trim_end_matches('/'), path) || (c.ends_with('/') && under(path, c))
            })
    })
}

/// The shortest `every` may be, in seconds: a workflow isn't a clock.
pub const MIN_EVERY: u64 = 300;

/// How long after its time a run `every` starts may still start, in seconds, as when the
/// server was restarting then: later, that time is passed over.
pub const EVERY_GRACE: u64 = 3600;

/// The days `day` may name, Monday first.
pub const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

const DAY: u64 = 86_400;

/// When a trigger's `every` comes round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    /// How often, in seconds.
    pub every: u64,
    /// How long after each whole multiple of `every` since 1970, in seconds: its `at`, and
    /// its `day`.
    pub offset: u64,
}

impl Schedule {
    /// The time it last came round, at or before `now` (unix seconds); none before the first.
    #[must_use]
    pub fn last(&self, now: u64) -> Option<u64> {
        let since = now.checked_sub(self.offset)?;
        Some(since - since % self.every + self.offset)
    }

    /// The time it next comes round, after `now`.
    #[must_use]
    pub fn next(&self, now: u64) -> u64 {
        self.last(now).map_or(self.offset, |last| last + self.every)
    }
}

impl Trigger {
    /// Which kind it is.
    ///
    /// # Errors
    ///
    /// When it has none, or more than one, of the fields that say.
    pub fn kind(&self) -> Result<TriggerKind, String> {
        let has = [
            (self.state.is_some(), TriggerKind::State),
            (self.every.is_some(), TriggerKind::Every),
            (self.said.is_some(), TriggerKind::Said),
            (self.reaction.is_some(), TriggerKind::Reaction),
            (!self.landed.is_empty(), TriggerKind::Landed),
        ];
        let mut kinds = has.iter().filter(|(has, _)| *has).map(|(_, kind)| *kind);
        match (kinds.next(), kinds.next()) {
            (Some(kind), None) => Ok(kind),
            _ => Err("a trigger has one of state, every, said, reaction and landed".to_owned()),
        }
    }

    /// When it comes round, if it's one that does (`every`).
    ///
    /// # Errors
    ///
    /// What's wrong with its `every`, `at` or `day`.
    pub fn schedule(&self) -> Result<Option<Schedule>, String> {
        let Some(every) = &self.every else {
            return Ok(None);
        };
        let every = seconds(every)?;
        if every < MIN_EVERY {
            return Err(format!("every is at least {MIN_EVERY}s"));
        }
        let mut offset = 0;
        if let Some(at) = &self.at {
            let bad = || format!("at {at:?} isn't a time of day like 09:00");
            if every % DAY != 0 {
                return Err("at needs an every of whole days".to_owned());
            }
            let (hours, minutes) = at.split_once(':').ok_or_else(bad)?;
            let part = |text: &str, below: u64| {
                (text.len() == 2)
                    .then(|| text.parse::<u64>().ok())
                    .flatten()
                    .filter(|n| *n < below)
            };
            offset +=
                part(hours, 24).ok_or_else(bad)? * 3600 + part(minutes, 60).ok_or_else(bad)? * 60;
        }
        if let Some(day) = &self.day {
            if every != 7 * DAY {
                return Err("day needs an every of 7d".to_owned());
            }
            let index = DAYS
                .iter()
                .position(|d| d == day)
                .ok_or_else(|| format!("day {day:?} is none of {}", DAYS.join(", ")))?;
            // 1 January 1970 was a Thursday.
            offset += (index as u64 + 7 - 3) % 7 * DAY;
        }
        Ok(Some(Schedule { every, offset }))
    }
}

/// The fields of a state a trigger can pass on as inputs.
pub const STATE_FIELDS: [&str; 5] = ["repo", "name", "why", "owner", "link"];

/// A step of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields, rename = "WorkflowStep")]
pub struct Step {
    /// Its id, unique in the workflow: lowercase letters, digits and `-`.
    pub id: String,
    /// What kind of step it is; a task when it doesn't say.
    #[serde(default)]
    pub kind: StepKind,
    /// Who does it ([`Who`]): `inputs.<name>`, `role:<name>`, or a nick. Tasks and
    /// approvals have one; waits and posts don't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// For a task, in place of `who`: everyone it goes to at once, each doing it on their
    /// own, in a run of their own under this one (`master-3.1`, `master-3.2`), followed,
    /// nudged and handed on as any task is. Its `says` reads whose it is as `{each}`. The
    /// step is done when all of them are ([`Workflow::when_one_fails`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<Each>,
    /// What the workflow says when the step starts: the task, the question, or the post. It
    /// may read inputs as `{name}` and earlier steps' outputs as `{step.output}`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub says: String,
    /// What a task hands back when it's done: names its later steps read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    /// When to run it: [`Condition`]; always when it doesn't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// How long it may stay quiet, neither moving on nor anything said by whoever has it,
    /// before the workflow asks them how it goes: such as `15m` ([`seconds`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge: Option<String>,
    /// How long it may stay quiet after that nudge, or after it last moved when there's no
    /// nudge, before it goes to its `fallback`; once there, the run fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate: Option<String>,
    /// The role it goes to when it's quiet past `escalate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// For a task: how it goes, each stage reached on what the workflow sees of whoever has
    /// it, without their saying so. Reaching the last is the task done, unless it has
    /// `outputs`: then it waits there for them, handed back with `zirc step done`. A failed
    /// landing goes back to the stage before the one its landing reached.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<Stage>,
    /// For an approval: a yes counts only after [`crate::request::HOLD_SECONDS`], which a no
    /// meanwhile stops, as for an irreversible request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub irreversible: bool,
    /// For a wait: the state it waits for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Until>,
}

/// What kind of step a step is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "WorkflowStepKind")]
pub enum StepKind {
    /// Work an agent session or a person claims and hands back, with its outputs.
    #[default]
    Task,
    /// A yes or no from a person: 👍 or 👎 on the workflow's message, from their own
    /// account. A no ends the run.
    Approve,
    /// Waiting until a state is said, such as master being ok again.
    Wait,
    /// The workflow saying something, and going on.
    Post,
}

/// What's wrong with `role` as a role's name, if anything: what `zirc role join` checks
/// before it says so.
///
/// # Errors
///
/// A sentence saying what's wrong.
pub fn check_role(role: &str) -> Result<(), String> {
    name(role, "a role")
}

impl StepKind {
    /// Its word, as a file says it: `task`, `approve`, `wait` or `post`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Approve => "approve",
            Self::Wait => "wait",
            Self::Post => "post",
        }
    }
}

/// A stage of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(rename = "WorkflowStage")]
pub struct Stage {
    /// What the card calls it, such as `working`.
    pub name: String,
    /// What reaches it.
    pub on: Signal,
}

/// What the workflow sees of whoever has a task, wherever it's said in the organisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "WorkflowSignal")]
pub enum Signal {
    /// They said something, anywhere.
    Said,
    /// They said what they're about to change (`zirc intend`).
    Intent,
    /// They pushed a branch.
    Push,
    /// CI queued their landing.
    LandingQueued,
    /// CI landed it.
    Landed,
}

/// The state a wait step waits for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(rename = "WorkflowUntil")]
pub struct Until {
    /// The state's name, or an input naming it, as `{state}`: a run started by one of several
    /// states waits for the one that started it.
    pub state: String,
    /// Its level.
    pub level: Level,
}

/// Who a step goes to, as its `who` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// Whoever an input names: `inputs.owner`.
    Input(String),
    /// One of a role's members, the agent sessions that joined it (`zirc role join`): the
    /// least busy live one, and the next when they're quiet. With none, it waits for one.
    Role(String),
    /// One nick, person, machine or `machine/session`.
    Named(String),
}

impl Who {
    /// What `who` says.
    ///
    /// # Errors
    ///
    /// When it's none of the three.
    pub fn parse(who: &str) -> Result<Self, String> {
        if let Some(input) = who.strip_prefix("inputs.") {
            return name(input, "an input").map(|()| Self::Input(input.to_owned()));
        }
        if let Some(role) = who.strip_prefix("role:") {
            return name(role, "a role").map(|()| Self::Role(role.to_owned()));
        }
        if is_member(who) {
            return Ok(Self::Named(who.to_owned()));
        }
        Err(format!(
            "who = {who:?} is none of inputs.<name>, role:<name> or a nick"
        ))
    }
}

/// Whether `member` can be someone a role or a step names: a nick, or `machine/session`.
#[must_use]
pub fn is_member(member: &str) -> bool {
    match member.split_once('/') {
        Some((machine, session)) => {
            crate::names::is_account_name(machine)
                && crate::session_name(session).as_deref() == Some(session)
        }
        None => crate::names::is_account_name(member),
    }
}

/// What reads a value in a condition or a step's `says`: an input, or an earlier step's
/// output.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ref {
    /// `name`: an input.
    Input(String),
    /// `step.output`.
    Output {
        /// The step.
        step: String,
        /// Its output.
        output: String,
    },
}

impl Ref {
    /// What `text` reads.
    ///
    /// # Errors
    ///
    /// When it isn't `name` or `step.output`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.split_once('.') {
            Some((step, output)) => {
                name(step, "a step")?;
                name(output, "an output")?;
                Ok(Self::Output {
                    step: step.to_owned(),
                    output: output.to_owned(),
                })
            }
            None => name(text, "an input").map(|()| Self::Input(text.to_owned())),
        }
    }
}

/// A step's `when`: comparisons joined by `and`, each `<ref> <op> <value>`, where the op is one
/// of `==`, `!=`, `>`, `>=`, `<` or `<=`, and the value is a word, a number or a quoted
/// string. Numbers compare as numbers; anything else only by `==` and `!=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    /// Every one must hold.
    pub all: Vec<Comparison>,
}

/// One comparison of a [`Condition`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// What it reads.
    pub left: Ref,
    /// How it compares.
    pub op: Op,
    /// What with.
    pub right: String,
}

/// How a comparison compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `<`
    Lt,
    /// `<=`
    Le,
}

impl Condition {
    /// What `text` says.
    ///
    /// # Errors
    ///
    /// When a part of it isn't `<ref> <op> <value>`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let all = text
            .split(" and ")
            .map(|part| {
                let part = part.trim();
                let (at, op, len) = [
                    ("==", Op::Eq),
                    ("!=", Op::Ne),
                    (">=", Op::Ge),
                    ("<=", Op::Le),
                    (">", Op::Gt),
                    ("<", Op::Lt),
                ]
                .into_iter()
                .find_map(|(word, op)| part.find(word).map(|at| (at, op, word.len())))
                .ok_or_else(|| format!("{part:?} has no ==, !=, >, >=, < or <="))?;
                let left = Ref::parse(part[..at].trim())?;
                let right = part[at + len..].trim();
                let right = right
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .unwrap_or(right);
                if right.is_empty() {
                    return Err(format!("{part:?} compares with nothing"));
                }
                Ok(Comparison {
                    left,
                    op,
                    right: right.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { all })
    }

    /// Whether it holds, reading values with `value`: one it can't read, such as an output a
    /// step didn't hand back, is nothing, so `!=` anything else holds and `==` doesn't.
    #[must_use]
    pub fn holds(&self, value: impl Fn(&Ref) -> Option<String>) -> bool {
        self.all.iter().all(|c| {
            let left = value(&c.left).unwrap_or_default();
            match (left.trim().parse::<f64>(), c.right.parse::<f64>()) {
                (Ok(l), Ok(r)) => match c.op {
                    Op::Eq => (l - r).abs() < f64::EPSILON,
                    Op::Ne => (l - r).abs() >= f64::EPSILON,
                    Op::Gt => l > r,
                    Op::Ge => l >= r,
                    Op::Lt => l < r,
                    Op::Le => l <= r,
                },
                _ => match c.op {
                    Op::Eq => left == c.right,
                    Op::Ne => left != c.right,
                    _ => false,
                },
            }
        })
    }
}

/// The references `{…}` in `text` reads, in order.
///
/// # Errors
///
/// When a `{` isn't closed, or what's between isn't a [`Ref`].
pub fn refs(text: &str) -> Result<Vec<Ref>, String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| format!("a {{ in {text:?} isn't closed"))?;
        found.push(Ref::parse(&after[..close])?);
        rest = &after[close + 1..];
    }
    Ok(found)
}

/// `text` with each `{ref}` replaced by what `value` reads for it, or left as it is when it
/// reads nothing.
#[must_use]
pub fn fill(text: &str, value: impl Fn(&Ref) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        if let Some(v) = Ref::parse(inner).ok().and_then(|r| value(&r)) {
            out.push_str(&v);
        } else {
            out.push('{');
            out.push_str(inner);
            out.push('}');
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// How many seconds `text` says: a whole number and `s`, `m`, `h` or `d`, such as `30m`.
///
/// # Errors
///
/// When it isn't one, or is 0.
pub fn seconds(text: &str) -> Result<u64, String> {
    let bad = || format!("{text:?} isn't a time like 90s, 30m, 2h or 2d");
    let unit = match text.chars().last().ok_or_else(bad)? {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return Err(bad()),
    };
    let n: u64 = text[..text.len() - 1].parse().map_err(|_| bad())?;
    if n == 0 {
        return Err(bad());
    }
    n.checked_mul(unit).ok_or_else(bad)
}

/// Whether `text` is a name a workflow, step, input, output or role may have.
fn name(text: &str, what: &str) -> Result<(), String> {
    let fine = (1..=MAX_NAME).contains(&text.len())
        && text.starts_with(|c: char| c.is_ascii_lowercase())
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if fine {
        Ok(())
    } else {
        Err(format!(
            "{what}'s name {text:?} isn't 1-{MAX_NAME} lowercase letters, digits, - and _, \
             starting with a letter"
        ))
    }
}

/// Whether `nick` is one a workflow may run as: an account name, but none of zirc's own
/// ([`crate::names::RESERVED_HANDLES`], [`crate::names::SYSTEM`]) and no integration's.
/// Whether a person or machine already has it is the server's to say.
#[must_use]
pub fn is_workflow_nick(nick: &str) -> bool {
    crate::names::is_account_name(nick)
        && !nick.starts_with("zirc")
        && !crate::names::RESERVED_HANDLES.contains(&nick)
}

impl Workflow {
    /// The channels it listens in from the start, lowercase: its own, those it follows, the
    /// states' ([`crate::state::CHANNEL`]) when a trigger or a wait reads a state, and the
    /// organisation's ([`crate::ORG_CHANNEL`]), whose machines say their sessions' roles,
    /// when a step or a fallback goes to a role.
    #[must_use]
    pub fn channels(&self) -> Vec<String> {
        let states = self.on.iter().any(|t| t.state.is_some())
            || self.steps.iter().any(|s| s.until.is_some());
        let roles = self.steps.iter().any(|s| {
            s.who
                .as_deref()
                .is_some_and(|who| matches!(Who::parse(who), Ok(Who::Role(_))))
                || matches!(&s.each, Some(Each::Named(names)) if names.iter().any(|n| n.starts_with("role:")))
        }) || self
            .roles
            .values()
            .flatten()
            .any(|m| m.starts_with("role:"));
        let mut channels: Vec<String> = std::iter::once(self.channel.as_str())
            .chain(self.follow.iter().map(String::as_str))
            .chain(states.then_some(crate::state::CHANNEL))
            .chain(roles.then_some(crate::ORG_CHANNEL))
            .map(str::to_lowercase)
            .collect();
        let mut seen = BTreeSet::new();
        channels.retain(|c| seen.insert(c.clone()));
        channels
    }

    /// The first rule it breaks, if any: that its names are well formed, every step's `who`,
    /// `fallback`, `when` and `says` read what's there before it, each kind of step has what
    /// it needs, and every trigger fills every input a run needs.
    ///
    /// # Errors
    ///
    /// A sentence saying what's wrong.
    pub fn check(&self) -> Result<(), String> {
        name(&self.name, "the workflow")?;
        if !is_workflow_nick(&self.nick) {
            return Err(format!(
                "nick {:?} isn't one a workflow may have: lowercase letters, digits, _ and -, \
                 starting with a letter, and not zirc's own",
                self.nick
            ));
        }
        if let Some(bad) = std::iter::once(&self.channel)
            .chain(&self.follow)
            .find(|c| !is_channel(c))
        {
            return Err(format!("channel {bad:?} isn't a channel"));
        }
        if self.follow.len() > MAX_FOLLOW {
            return Err(format!("it follows more than {MAX_FOLLOW} channels"));
        }
        let mut inputs = BTreeSet::new();
        for input in &self.inputs {
            name(&input.name, "an input")?;
            if input.name == EACH {
                return Err(format!(
                    "an input is named {EACH:?}, which a step given to several reads whose it is by"
                ));
            }
            if !inputs.insert(input.name.as_str()) {
                return Err(format!("two inputs are both {:?}", input.name));
            }
        }
        if let Some(input) = &self.one_run_per
            && !inputs.contains(input.as_str())
        {
            return Err(format!(
                "one_run_per names {input:?}, which isn't one of its inputs"
            ));
        }
        for (role, members) in &self.roles {
            name(role, "a role")?;
            if members.is_empty() {
                return Err(format!("role {role:?} has nobody in it"));
            }
            let fits = |m: &&String| {
                m.strip_prefix("role:")
                    .map_or_else(|| is_member(m), |live| name(live, "a role").is_ok())
            };
            if let Some(bad) = members.iter().find(|m| !fits(m)) {
                return Err(format!(
                    "role {role:?} names {bad:?}, which is none of a nick, machine/session or \
                     role:<name>"
                ));
            }
        }
        for trigger in &self.on {
            self.check_trigger(trigger, &inputs)?;
        }
        if self.steps.is_empty() {
            return Err("it has no steps".to_owned());
        }
        if self.steps.len() > MAX_STEPS {
            return Err(format!("it has more than {MAX_STEPS} steps"));
        }
        let mut outputs: BTreeMap<&str, &[String]> = BTreeMap::new();
        for step in &self.steps {
            self.check_step(step, &inputs, &outputs)?;
            outputs.insert(&step.id, &step.outputs);
        }
        Ok(())
    }

    /// The rules for what starts a run: one kind of trigger, with only what its kind has,
    /// filling every input a run needs from what it knows.
    fn check_trigger(&self, trigger: &Trigger, inputs: &BTreeSet<&str>) -> Result<(), String> {
        let kind = trigger.kind()?;
        let word = kind.word();
        trigger.schedule().map_err(|e| format!("a trigger: {e}"))?;
        if kind != TriggerKind::Every
            && (!trigger.with.is_empty() || trigger.at.is_some() || trigger.day.is_some())
        {
            return Err(format!(
                "the trigger on {word} has with, at or day, which only every has"
            ));
        }
        if (kind != TriggerKind::State && trigger.level.is_some())
            || (kind == TriggerKind::Every && !trigger.inputs.is_empty())
        {
            return Err(format!(
                "a trigger with {word} has a level or inputs, which only a state has: \
                 give a time's inputs as with"
            ));
        }
        match kind {
            TriggerKind::State => {
                let state = trigger.state.as_deref().unwrap_or_default();
                if let Some(why) = crate::state::check_name(state) {
                    return Err(why.to_owned());
                }
                if trigger.level.is_none() {
                    return Err(format!("the trigger on {state:?} needs a level"));
                }
            }
            TriggerKind::Said => {
                let said = trigger.said.as_deref().unwrap_or_default();
                if said.trim().is_empty() || said.chars().count() > MAX_SAID || said.contains('\n')
                {
                    return Err(format!(
                        "said is some words on a line, {MAX_SAID} characters at most"
                    ));
                }
            }
            TriggerKind::Reaction => {
                let reaction = trigger.reaction.as_deref().unwrap_or_default();
                if reaction.is_empty() || reaction.chars().count() > 8 || reaction.contains(' ') {
                    return Err(format!("reaction {reaction:?} isn't one reaction"));
                }
                if ["👍", "👎"].contains(&reaction) {
                    return Err("👍 and 👎 answer an approval: choose another reaction".to_owned());
                }
            }
            TriggerKind::Landed => {
                if trigger.landed.len() > MAX_LANDED {
                    return Err(format!("landed names more than {MAX_LANDED} paths"));
                }
                if let Some(bad) = trigger.landed.iter().find(|p| {
                    p.is_empty()
                        || p.starts_with('/')
                        || p.contains(['*', '\n'])
                        || p.contains("..")
                }) {
                    return Err(format!(
                        "landed names {bad:?}, which isn't a file or directory from the \
                         repository's root (no patterns: a directory is everything under it)"
                    ));
                }
            }
            TriggerKind::Every => {}
        }
        let filled = if kind == TriggerKind::Every {
            &trigger.with
        } else {
            &trigger.inputs
        };
        if let Some(input) = filled.keys().find(|i| !inputs.contains(i.as_str())) {
            return Err(format!("a trigger fills {input:?}, which isn't an input"));
        }
        if kind != TriggerKind::Every
            && let Some(field) = trigger
                .inputs
                .values()
                .find(|f| !kind.fields().contains(&f.as_str()))
        {
            return Err(format!(
                "a trigger reads {word}'s {field:?}, which is none of {}",
                kind.fields().join(", ")
            ));
        }
        let what = match &trigger.state {
            Some(state) => format!("on {state:?}"),
            None => format!("with {word}"),
        };
        match self
            .inputs
            .iter()
            .find(|i| !i.optional && !filled.contains_key(&i.name))
        {
            Some(missing) => Err(format!(
                "the trigger {what} doesn't fill the input {:?}",
                missing.name
            )),
            None => Ok(()),
        }
    }

    fn check_step(
        &self,
        step: &Step,
        inputs: &BTreeSet<&str>,
        before: &BTreeMap<&str, &[String]>,
    ) -> Result<(), String> {
        name(&step.id, "a step")?;
        let id = &step.id;
        if before.contains_key(id.as_str()) {
            return Err(format!("two steps are both {id:?}"));
        }
        let mut names = BTreeSet::new();
        for output in &step.outputs {
            name(output, "an output")?;
            if !names.insert(output) {
                return Err(format!("step {id:?} has two outputs {output:?}"));
            }
        }
        let read = |r: &Ref| match r {
            Ref::Input(input) if inputs.contains(input.as_str()) => Ok(()),
            Ref::Input(input) if input == EACH && step.each.is_some() => Ok(()),
            Ref::Input(input) => Err(format!("step {id:?} reads {input:?}, which isn't an input")),
            Ref::Output { step: from, output } => match before.get(from.as_str()) {
                Some(outs) if outs.contains(output) => Ok(()),
                Some(_) => Err(format!(
                    "step {id:?} reads {from}.{output}, which step {from:?} doesn't hand back"
                )),
                None => Err(format!(
                    "step {id:?} reads step {from:?}, which isn't a step before it"
                )),
            },
        };
        if step.says.chars().count() > MAX_SAYS {
            return Err(format!("step {id:?} says more than {MAX_SAYS} characters"));
        }
        for r in refs(&step.says).map_err(|e| format!("step {id:?}: {e}"))? {
            read(&r)?;
        }
        if let Some(until) = &step.until {
            for r in refs(&until.state).map_err(|e| format!("step {id:?}: {e}"))? {
                read(&r)?;
            }
        }
        if let Some(when) = &step.when {
            let condition = Condition::parse(when).map_err(|e| format!("step {id:?}: {e}"))?;
            for c in &condition.all {
                read(&c.left)?;
            }
        }
        self.check_shepherding(step)?;
        Self::check_each(step, inputs)?;
        let has_who = match step.who.as_deref().map(Who::parse).transpose()? {
            Some(Who::Input(input)) if !inputs.contains(input.as_str()) => {
                return Err(format!(
                    "step {id:?} goes to inputs.{input}, which isn't an input"
                ));
            }
            Some(Who::Role(_)) if step.kind == StepKind::Approve => {
                return Err(format!(
                    "step {id:?} is an approval, which a person answers: a role's members \
                     are agent sessions, so name the person"
                ));
            }
            who => who.is_some() || step.each.is_some(),
        };
        match step.kind {
            StepKind::Task | StepKind::Approve if !has_who => {
                Err(format!("step {id:?} needs a who: who does it?"))
            }
            StepKind::Approve if !step.outputs.is_empty() => Err(format!(
                "step {id:?} is an approval, which hands back only yes or no"
            )),
            StepKind::Wait if step.until.is_none() => {
                Err(format!("step {id:?} waits, but not until anything"))
            }
            StepKind::Wait | StepKind::Post if has_who || !step.outputs.is_empty() => Err(format!(
                "step {id:?} is done by the workflow, so it has no who and no outputs"
            )),
            StepKind::Post if step.says.trim().is_empty() => {
                Err(format!("step {id:?} posts, but says nothing"))
            }
            StepKind::Task | StepKind::Approve | StepKind::Post if step.until.is_some() => {
                Err(format!("step {id:?} has an until, which only a wait has"))
            }
            _ if step.irreversible && step.kind != StepKind::Approve => Err(format!(
                "step {id:?} is irreversible, which only an approval may be"
            )),
            _ => Ok(()),
        }
    }
}

impl Workflow {
    /// The rules for a step given to several at once: a task, with no `who` of its own and
    /// no outputs, as each hands back its own and none is the step's.
    fn check_each(step: &Step, inputs: &BTreeSet<&str>) -> Result<(), String> {
        let id = &step.id;
        let Some(each) = &step.each else {
            return Ok(());
        };
        if step.kind != StepKind::Task || step.who.is_some() {
            return Err(format!(
                "step {id:?} has each, which a task has in place of a who"
            ));
        }
        if !step.outputs.is_empty() {
            return Err(format!(
                "step {id:?} goes to each of several, so it has no outputs of its own"
            ));
        }
        match each {
            Each::Input(input) => match Who::parse(input)? {
                Who::Input(input) if inputs.contains(input.as_str()) => Ok(()),
                _ => Err(format!(
                    "step {id:?} goes to each of {input:?}, which isn't inputs.<an input> or a list"
                )),
            },
            Each::Named(names) => {
                let fits = |m: &&String| {
                    m.strip_prefix("role:")
                        .map_or_else(|| is_member(m), |role| name(role, "a role").is_ok())
                };
                if names.is_empty() || names.len() > MAX_EACH {
                    return Err(format!("step {id:?} goes to each of 1 to {MAX_EACH}"));
                }
                match names.iter().find(|m| !fits(m)) {
                    Some(bad) => Err(format!(
                        "step {id:?} goes to each of {bad:?}, which is none of a nick, \
                         machine/session or role:<name>"
                    )),
                    None => Ok(()),
                }
            }
        }
    }

    /// The rules for how `step` is followed: its times, its fallback and its stages.
    fn check_shepherding(&self, step: &Step) -> Result<(), String> {
        let id = &step.id;
        for time in [&step.nudge, &step.escalate].into_iter().flatten() {
            seconds(time).map_err(|e| format!("step {id:?}: {e}"))?;
        }
        if let Some(fallback) = &step.fallback {
            if !self.roles.contains_key(fallback) {
                return Err(format!(
                    "step {id:?} falls back to {fallback:?}, which isn't a role"
                ));
            }
            if step.escalate.is_none() {
                return Err(format!(
                    "step {id:?} has a fallback but no escalate: when does it go there?"
                ));
            }
        }
        if !step.stages.is_empty() && step.kind != StepKind::Task {
            return Err(format!("step {id:?} has stages, which only a task has"));
        }
        let mut names = BTreeSet::new();
        for stage in &step.stages {
            name(&stage.name, "a stage")?;
            if !names.insert(stage.name.as_str()) {
                return Err(format!("step {id:?} has two stages {:?}", stage.name));
            }
        }
        Ok(())
    }
}

/// A workflow's file sent to the server: what [`KIND`] carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Deploy {
    /// The workflow's name, as the file says it.
    pub name: String,
    /// Its nick, as the file says it.
    pub nick: String,
    /// The file, as written, at most [`MAX_SOURCE`] bytes: the server reads the rest from it.
    pub source: String,
    /// Whose deploy it is, when zirc.dev's dashboard says it for the person signed in there, as
    /// [`crate::names::SYSTEM`]: their account. Said by anyone else it's ignored, and the
    /// deploy is whoever's account said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

impl Deploy {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a deploy serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a deploy serializes"),
            text: format!(
                "**Workflow:** `{}`, run by @{}",
                self.name.replace('\n', " "),
                self.nick.replace('\n', " ")
            ),
        }
    }

    /// The deploy `event` carries, if it's one.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_hint_is_said_once_and_briefly() {
        let keys: BTreeSet<&str> = HINTS.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys.len(), HINTS.len());
        assert!(
            HINTS.iter().all(|(_, hint)| hint.len() <= 200),
            "a hint is a few words"
        );
    }

    fn every(every: &str, at: Option<&str>, day: Option<&str>) -> Trigger {
        Trigger {
            state: None,
            level: None,
            inputs: BTreeMap::new(),
            said: None,
            reaction: None,
            landed: Vec::new(),
            every: Some(every.into()),
            at: at.map(Into::into),
            day: day.map(Into::into),
            with: BTreeMap::new(),
        }
    }

    #[test]
    fn a_schedule_comes_round_at_whole_multiples_since_1970_and_its_time_of_day() {
        // 2026-11-16 is a Monday: 20_773 days since 1970.
        let monday = 20_773 * DAY;
        let daily = every("1d", Some("09:00"), None)
            .schedule()
            .unwrap()
            .unwrap();
        assert_eq!(daily.last(monday + 10 * 3600), Some(monday + 9 * 3600));
        assert_eq!(daily.last(monday + 8 * 3600), Some(monday - DAY + 9 * 3600));
        assert_eq!(daily.next(monday + 9 * 3600), monday + DAY + 9 * 3600);
        let weekly = every("7d", Some("09:00"), Some("mon"))
            .schedule()
            .unwrap()
            .unwrap();
        assert_eq!(weekly.last(monday + 2 * DAY), Some(monday + 9 * 3600));
        assert_eq!(weekly.next(monday + 2 * DAY), monday + 7 * DAY + 9 * 3600);
        let six = every("6h", None, None).schedule().unwrap().unwrap();
        assert_eq!(six.last(monday + 7 * 3600), Some(monday + 6 * 3600));
        assert_eq!(
            Schedule {
                every: DAY,
                offset: 3600
            }
            .last(60),
            None
        );
    }

    #[test]
    fn a_landing_is_under_a_path_by_file_directory_or_the_directory_ci_named() {
        let paths = |p: &[&str]| p.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
        let under = |p: &[&str], c: &[&str]| landed_under(&paths(p), &paths(c));
        assert!(under(&["oss/crates/zirc"], &["oss/crates/zirc/src/lib.rs"]));
        assert!(under(
            &["oss/crates/zirc/"],
            &["oss/crates/zirc/src/lib.rs"]
        ));
        assert!(!under(
            &["oss/crates/zirc"],
            &["oss/crates/zirc-core/src/lib.rs"]
        ));
        assert!(under(&["MODULE.bazel"], &["web/a.ts", "MODULE.bazel"]));
        assert!(!under(&["MODULE.bazel"], &["MODULE.bazel.lock"]));
        assert!(under(&["."], &[]));
        assert!(!under(&["web"], &[]));
        // CI named a directory in place of its files: what's under it may have changed.
        assert!(under(&["oss/crates/zirc/src"], &["oss/crates/"]));
        assert!(under(&["oss"], &["oss/crates/"]));
        assert!(!under(&["web"], &["oss/crates/"]));
    }

    #[test]
    fn a_schedule_says_whats_wrong_with_it() {
        let wrong = |t: Trigger| t.schedule().unwrap_err();
        assert!(wrong(every("1m", None, None)).contains("at least 300s"));
        assert!(wrong(every("6h", Some("09:00"), None)).contains("whole days"));
        assert!(wrong(every("1d", Some("9:00"), None)).contains("time of day"));
        assert!(wrong(every("1d", Some("24:00"), None)).contains("time of day"));
        assert!(wrong(every("1d", None, Some("mon"))).contains("7d"));
        assert!(wrong(every("7d", None, Some("monday"))).contains("none of mon"));
    }

    fn step(id: &str, kind: StepKind) -> Step {
        Step {
            id: id.into(),
            kind,
            who: None,
            each: None,
            says: String::new(),
            outputs: Vec::new(),
            when: None,
            nudge: None,
            escalate: None,
            fallback: None,
            stages: Vec::new(),
            irreversible: false,
            until: None,
        }
    }

    fn master() -> Workflow {
        Workflow {
            name: "master".into(),
            nick: "master_bot".into(),
            description: None,
            channel: "#ops".into(),
            follow: vec![],
            one_run_per: None,
            when_one_fails: None,
            inputs: vec![
                Input {
                    name: "owner".into(),
                    about: None,
                    optional: false,
                },
                Input {
                    name: "why".into(),
                    about: None,
                    optional: true,
                },
            ],
            roles: BTreeMap::from([("fixers".into(), vec!["mattias".into()])]),
            on: vec![Trigger {
                state: Some("master".into()),
                level: Some(Level::Broken),
                inputs: BTreeMap::from([("owner".into(), "owner".into())]),
                said: None,
                reaction: None,
                landed: Vec::new(),
                every: None,
                at: None,
                day: None,
                with: BTreeMap::new(),
            }],
            steps: vec![
                Step {
                    who: Some("inputs.owner".into()),
                    says: "Fix master ({why})".into(),
                    outputs: vec!["cause".into()],
                    escalate: Some("2h".into()),
                    fallback: Some("fixers".into()),
                    stages: vec![Stage {
                        name: "working".into(),
                        on: Signal::Push,
                    }],
                    ..step("fix", StepKind::Task)
                },
                Step {
                    when: Some("fix.cause == mine".into()),
                    says: "{owner} fixed it: {fix.cause}".into(),
                    ..step("tell", StepKind::Post)
                },
                Step {
                    until: Some(Until {
                        state: "master".into(),
                        level: Level::Ok,
                    }),
                    ..step("green", StepKind::Wait)
                },
            ],
        }
    }

    #[test]
    fn a_whole_workflow_checks() {
        assert_eq!(master().check(), Ok(()));
    }

    #[test]
    fn steps_read_only_what_is_there_before_them() {
        let mut w = master();
        w.steps[1].says = "{green.thing}".into();
        assert!(w.check().unwrap_err().contains("isn't a step before it"));
        let mut w = master();
        w.steps[1].when = Some("fix.other == 1".into());
        assert!(w.check().unwrap_err().contains("doesn't hand back"));
        let mut w = master();
        w.steps[0].says = "{nobody}".into();
        assert!(w.check().unwrap_err().contains("isn't an input"));
    }

    #[test]
    fn each_kind_has_what_it_needs() {
        let mut w = master();
        w.steps[0].who = None;
        assert!(w.check().unwrap_err().contains("needs a who"));
        let mut w = master();
        w.steps[2].until = None;
        assert!(w.check().unwrap_err().contains("not until anything"));
        let mut w = master();
        w.steps[1].who = Some("mattias".into());
        assert!(w.check().unwrap_err().contains("done by the workflow"));
        let mut w = master();
        w.steps[0].irreversible = true;
        assert!(w.check().unwrap_err().contains("only an approval"));
        let mut w = master();
        w.steps[0].fallback = Some("nobody".into());
        assert!(w.check().unwrap_err().contains("isn't a role"));
        let mut w = master();
        w.steps[0].escalate = None;
        assert!(w.check().unwrap_err().contains("no escalate"));
        let mut w = master();
        w.steps[2].stages = w.steps[0].stages.clone();
        assert!(w.check().unwrap_err().contains("only a task has"));
    }

    #[test]
    fn a_workflow_listens_where_its_file_says_and_where_states_and_roles_are_said() {
        let mut w = master();
        // #ops is its channel and the states' both.
        assert_eq!(w.channels(), ["#ops"]);
        w.channel = "#Fixes".into();
        w.follow = vec!["#monorepo/landings".into(), "#fixes".into()];
        w.steps[0].who = Some("role:fixers".into());
        assert_eq!(
            w.channels(),
            ["#fixes", "#monorepo/landings", "#ops", "#general"]
        );
        w.follow = vec!["landings".into()];
        assert!(w.check().unwrap_err().contains("isn't a channel"));
        // A fallback to a role listens where roles are said, as a role's step does.
        let mut w = master();
        w.roles.insert(
            "fixers".into(),
            vec!["role:master-fixers".into(), "mattias".into()],
        );
        assert!(w.check().is_ok());
        assert_eq!(w.channels(), ["#ops", "#general"]);
        w.roles.insert("fixers".into(), vec!["role:Not One".into()]);
        assert!(w.check().unwrap_err().contains("role:<name>"));
    }

    #[test]
    fn triggers_fill_every_input_a_run_needs() {
        let mut w = master();
        w.on[0].inputs.clear();
        assert!(
            w.check()
                .unwrap_err()
                .contains("doesn't fill the input \"owner\"")
        );
        let mut w = master();
        w.on[0].inputs.insert("why".into(), "colour".into());
        assert!(w.check().unwrap_err().contains("none of repo"));
    }

    #[test]
    fn runs_are_told_apart_by_one_of_its_inputs() {
        let mut w = master();
        w.one_run_per = Some("why".into());
        assert_eq!(w.check(), Ok(()));
        w.one_run_per = Some("test".into());
        assert!(w.check().unwrap_err().contains("isn't one of its inputs"));
    }

    #[test]
    fn nicks_are_never_zircs_own() {
        assert!(is_workflow_nick("master_bot"));
        assert!(!is_workflow_nick("zirc-ci"));
        assert!(!is_workflow_nick("zirc"));
        assert!(!is_workflow_nick("Master"));
        let mut w = master();
        w.nick = "zirc-master".into();
        assert!(w.check().is_err());
    }

    #[test]
    fn who_is_an_input_a_role_or_a_nick() {
        assert_eq!(Who::parse("inputs.owner"), Ok(Who::Input("owner".into())));
        assert_eq!(Who::parse("role:fixers"), Ok(Who::Role("fixers".into())));
        assert_eq!(Who::parse("mattias"), Ok(Who::Named("mattias".into())));
        assert_eq!(
            Who::parse("m3-mac/cc-1a2b3c"),
            Ok(Who::Named("m3-mac/cc-1a2b3c".into()))
        );
        assert!(Who::parse("Not A Nick").is_err());
    }

    #[test]
    fn conditions_compare_numbers_as_numbers() {
        let c = Condition::parse("extract.total > 1000 and vendor == \"acme\"").unwrap();
        let read = |total: &'static str| {
            move |r: &Ref| match r {
                Ref::Output { .. } => Some(total.to_owned()),
                Ref::Input(_) => Some("acme".to_owned()),
            }
        };
        assert!(c.holds(read("1500")));
        assert!(!c.holds(read("999.5")));
        assert!(!c.holds(read("lots")));
        assert!(!c.holds(|_| None));
        assert!(Condition::parse("total").is_err());
        assert!(Condition::parse("total ==").is_err());
        let ne = Condition::parse("fix.cause != mine").unwrap();
        assert!(ne.holds(|_| Some("flaky".into())));
        assert!(ne.holds(|_| None), "an output not handed back is nothing");
    }

    #[test]
    fn says_fills_what_it_reads() {
        let filled = fill("{owner} broke it: {fix.cause} {unknown} {", |r| match r {
            Ref::Input(i) if i == "owner" => Some("cc-1".into()),
            Ref::Output { step, output } if step == "fix" && output == "cause" => {
                Some("a typo".into())
            }
            _ => None,
        });
        assert_eq!(filled, "cc-1 broke it: a typo {unknown} {");
        assert!(refs("a { b").is_err());
    }

    #[test]
    fn times_are_whole_units() {
        assert_eq!(seconds("90s"), Ok(90));
        assert_eq!(seconds("30m"), Ok(1800));
        assert_eq!(seconds("2h"), Ok(7200));
        assert_eq!(seconds("2d"), Ok(172_800));
        assert!(seconds("0m").is_err());
        assert!(seconds("1.5h").is_err());
        assert!(seconds("h").is_err());
        assert!(seconds("").is_err());
    }

    #[test]
    fn a_deploy_travels_as_a_custom_event() {
        let deploy = Deploy {
            name: "master".into(),
            nick: "master_bot".into(),
            source: "name = \"master\"".into(),
            by: None,
        };
        let event = deploy.event();
        assert!(crate::check_kind(KIND).is_none());
        assert_eq!(
            event.human_text(),
            "**Workflow:** `master`, run by @master_bot"
        );
        assert_eq!(Deploy::of(&event), Some(deploy));
    }
}
