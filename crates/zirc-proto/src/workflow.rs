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

/// A workflow, as its file says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

/// Something a run is given when it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

/// A state ([`crate::state`]) that starts a run when it's said: the state's fields become the
/// run's inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    /// The state's name, such as `master`.
    pub state: String,
    /// The level that starts a run.
    pub level: Level,
    /// Which input each of the state's fields fills: `input = "field"`, where the field is
    /// one of [`STATE_FIELDS`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
}

/// The fields of a state a trigger can pass on as inputs.
pub const STATE_FIELDS: [&str; 5] = ["repo", "name", "why", "owner", "link"];

/// A step of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
    /// it, without their saying so. Reaching the last is the task done; a failed landing goes
    /// back to the stage before the one its landing reached.
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    /// What the card calls it, such as `working`.
    pub name: String,
    /// What reaches it.
    pub on: Signal,
}

/// What the workflow sees of whoever has a task, wherever it's said in the organisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Until {
    /// The state's name.
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
/// ([`crate::names::RESERVED_HANDLES`], [`crate::names::SYSTEM`]), no integration's and no
/// guest's. Whether a person or machine already has it is the server's to say.
#[must_use]
pub fn is_workflow_nick(nick: &str) -> bool {
    crate::names::is_account_name(nick)
        && !nick.starts_with("zirc")
        && !crate::names::RESERVED_HANDLES.contains(&nick)
        && !crate::names::is_guest_account(nick)
}

impl Workflow {
    /// The channels it listens in from the start, lowercase: its own, those it follows, the
    /// states' ([`crate::state::CHANNEL`]) when a trigger or a wait reads a state, and the
    /// organisation's ([`crate::ORG_CHANNEL`]), whose machines say their sessions' roles,
    /// when a step or a fallback goes to a role.
    #[must_use]
    pub fn channels(&self) -> Vec<String> {
        let states = !self.on.is_empty() || self.steps.iter().any(|s| s.until.is_some());
        let roles = self.steps.iter().any(|s| {
            s.who
                .as_deref()
                .is_some_and(|who| matches!(Who::parse(who), Ok(Who::Role(_))))
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
            if !inputs.insert(input.name.as_str()) {
                return Err(format!("two inputs are both {:?}", input.name));
            }
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
            if let Some(why) = crate::state::check_name(&trigger.state) {
                return Err(why.to_owned());
            }
            for (input, field) in &trigger.inputs {
                if !inputs.contains(input.as_str()) {
                    return Err(format!("a trigger fills {input:?}, which isn't an input"));
                }
                if !STATE_FIELDS.contains(&field.as_str()) {
                    return Err(format!(
                        "a trigger reads the state's {field:?}, which is none of {}",
                        STATE_FIELDS.join(", ")
                    ));
                }
            }
            if let Some(missing) = self
                .inputs
                .iter()
                .find(|i| !i.optional && !trigger.inputs.contains_key(&i.name))
            {
                return Err(format!(
                    "the trigger on {:?} doesn't fill the input {:?}",
                    trigger.state, missing.name
                ));
            }
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
        if let Some(when) = &step.when {
            let condition = Condition::parse(when).map_err(|e| format!("step {id:?}: {e}"))?;
            for c in &condition.all {
                read(&c.left)?;
            }
        }
        self.check_shepherding(step)?;
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
            who => who.is_some(),
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

    fn step(id: &str, kind: StepKind) -> Step {
        Step {
            id: id.into(),
            kind,
            who: None,
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
                state: "master".into(),
                level: Level::Broken,
                inputs: BTreeMap::from([("owner".into(), "owner".into())]),
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
