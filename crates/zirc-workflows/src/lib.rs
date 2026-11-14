//! Running a workflow ([`zirc_proto::workflow`]): reading its file, and the [`Engine`] that
//! shepherds each run along.
//!
//! The engine does no I/O and reads no clock. What happens is handed to it as an [`Input`]
//! (a mention of its nick, a state said, a worker's [`StepAction`], what it sees someone do,
//! or the time passing) with the time, and it hands back what its nick says: [`RunEvent`]s,
//! each carrying the run's card, and replies to whoever asked for something it won't do. It
//! applies its own events as it makes them, and the same events read back from the channel's
//! history ([`Engine::restore`]) rebuild it, so the sidecar that hosts it keeps nothing else.
//!
//! It follows a task rather than waiting to be told: what it sees of whoever has it
//! ([`Did`]: what they say, their intents, pushes and landings) moves the task through its
//! stages, and keeps it from being quiet. A step quiet past its `nudge` gets a mention asking
//! how it goes; quiet past its `escalate` after that, it goes to its fallback, and once
//! there, the run fails.
//!
//! Whoever acts is an [`Actor`], as the server says: an account, the session it says it's
//! from, and whether it's a person's own account. A step goes to names ([`zirc_proto::run::
//! Change::Assigned`]), and an actor may act on it when one of its names is one of them: its
//! account, `account/session`, or the bare session. Only a person may answer an approval.

use std::collections::BTreeMap;

use zirc_proto::Event;
use zirc_proto::card::{Card, Field, State as CardState};
use zirc_proto::run::{Act, Change, MAX_OUTPUTS, Outcome, RunEvent, StepAction};
use zirc_proto::state::{Level, State};
use zirc_proto::workflow::{
    Condition, EACH, EVERY_GRACE, Each, MAX_EACH, MAX_ENDED, MAX_SOURCE, OneFails, Ref, Signal,
    Step, StepKind, Trigger, TriggerKind, Who, Workflow, fill, landed_under, seconds,
};

/// The workflow `source` says, once it keeps every rule.
///
/// # Errors
///
/// What's wrong with it: too long, not TOML of a workflow's shape, or a rule it breaks
/// ([`Workflow::check`]).
pub fn parse(source: &str) -> Result<Workflow, String> {
    if source.len() > MAX_SOURCE {
        return Err(format!(
            "it's {} bytes, more than {MAX_SOURCE}",
            source.len()
        ));
    }
    let workflow: Workflow = toml::from_str(source).map_err(|e| e.message().to_owned())?;
    workflow.check()?;
    Ok(workflow)
}

/// The version of a workflow's file: the first 12 hex digits of its SHA-256.
#[must_use]
pub fn version(source: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, source.as_bytes());
    zirc_proto::hex(&digest.as_ref()[..6])
}

/// Who did something, as the server says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// Their account; empty when only their session is known, as when CI names it.
    pub account: String,
    /// The session the message says it's from, if a machine's.
    pub session: Option<String>,
    /// Whether it's a person's own account, not a machine's.
    pub person: bool,
}

impl Actor {
    /// A person, on their own account.
    #[must_use]
    pub fn person(account: &str) -> Self {
        Self {
            account: account.to_owned(),
            session: None,
            person: true,
        }
    }

    /// A machine's session.
    #[must_use]
    pub fn session(machine: &str, session: &str) -> Self {
        Self {
            account: machine.to_owned(),
            session: Some(session.to_owned()),
            person: false,
        }
    }

    /// A session known only by its id, as CI names the session a landing is for.
    #[must_use]
    pub fn named_session(session: &str) -> Self {
        Self::session("", session)
    }

    /// How a step names them: `machine/session` for a session, else the account.
    #[must_use]
    pub fn name(&self) -> String {
        match &self.session {
            Some(session) if self.account.is_empty() => session.clone(),
            Some(session) => format!("{}/{session}", self.account),
            None => self.account.clone(),
        }
    }

    /// Whether `to` names them.
    fn is_in(&self, to: &[String]) -> bool {
        to.iter().any(|name| {
            (!self.account.is_empty() && *name == self.account)
                || self.session.as_ref().is_some_and(|session| {
                    name == session
                        || *name == format!("{}/{session}", self.account)
                        || (self.account.is_empty() && name.ends_with(&format!("/{session}")))
                })
        })
    }
}

/// What the engine sees someone do, which moves a task they have through its stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Did {
    /// Said something.
    Said,
    /// Said what they're about to change.
    Intent,
    /// Pushed a branch.
    Push,
    /// Had a landing queued.
    LandingQueued,
    /// Had a landing land.
    Landed,
    /// Had a landing fail.
    LandingFailed,
}

impl Did {
    /// The stage signal it is; none for a failed landing, which goes back.
    #[must_use]
    pub fn signal(self) -> Option<Signal> {
        match self {
            Self::Said => Some(Signal::Said),
            Self::Intent => Some(Signal::Intent),
            Self::Push => Some(Signal::Push),
            Self::LandingQueued => Some(Signal::LandingQueued),
            Self::Landed => Some(Signal::Landed),
            Self::LandingFailed => None,
        }
    }
}

/// A live member of a role, as the host knows them across its workflows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// How a step names them: `machine/session`.
    pub name: String,
    /// How many workflows' steps they have now.
    pub busy: usize,
    /// When they were last given one, in unix seconds; 0 if never.
    pub last: u64,
}

/// What happened, for the engine to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Someone wrote to the workflow's nick: `run key=value…` or `cancel <run>`.
    Mention {
        /// Who.
        by: Actor,
        /// What, after the nick.
        text: String,
    },
    /// A state was said ([`zirc_proto::state`]).
    State(State),
    /// A worker said what they did with a step.
    Action {
        /// Who.
        by: Actor,
        /// What.
        action: StepAction,
    },
    /// Someone did something the engine follows tasks by.
    Activity {
        /// Who.
        by: Actor,
        /// What.
        did: Did,
        /// What with: the branch pushed, the landing's build.
        detail: Option<String>,
        /// The branch it was on, for a push or a landing: once a task's assignee has pushed
        /// one, only that branch's landings move it.
        branch: Option<String>,
        /// What they said, for a post or an intent.
        text: Option<String>,
    },
    /// A person or an integration said something where the workflow listens, not to its
    /// nick: what a `said` trigger starts a run on.
    Said {
        /// Who, as the run says started it.
        by: String,
        /// Where.
        channel: String,
        /// The message's id on the server.
        id: String,
        /// What they said.
        text: String,
    },
    /// A person reacted to a message the workflow heard: what a `reaction` trigger starts a
    /// run on.
    Reacted {
        /// Who reacted.
        by: String,
        /// With what.
        reaction: String,
        /// Where.
        channel: String,
        /// The message's id on the server.
        id: String,
        /// What it says, when the workflow still has it.
        text: Option<String>,
        /// Who wrote it, when it does.
        author: Option<String>,
    },
    /// CI landed something: what a `landed` trigger starts a run on.
    Landed {
        /// What CI says of the landing, by [`zirc_proto::workflow::LANDED_FIELDS`].
        landing: BTreeMap<String, String>,
        /// The paths it changed, as CI says them.
        changed: Vec<String>,
    },
    /// Time passed.
    Tick,
}

/// Who started a run a time started ([`Change::Started`]'s `by`): `every:`, the trigger's
/// place among `on`, and the time that came round.
const SCHEDULE: &str = "every:";

fn scheduled_by(trigger: usize, at: u64) -> String {
    format!("{SCHEDULE}{trigger}:{at}")
}

/// The trigger and the time `by` says started a run, if a time did.
fn scheduled(by: &str) -> Option<(usize, u64)> {
    let (trigger, at) = by.strip_prefix(SCHEDULE)?.split_once(':')?;
    Some((trigger.parse().ok()?, at.parse().ok()?))
}

/// The fields known of something that happened, those there are.
fn fields<const N: usize>(
    known: [(&'static str, Option<String>); N],
) -> impl Iterator<Item = (String, String)> {
    known
        .into_iter()
        .filter_map(|(field, value)| Some((field.to_owned(), value?)))
}

/// What the workflow's nick says.
#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    /// A run changed: post this in the workflow's channel.
    Run(Event),
    /// A reply to someone who asked for something it won't do, naming them.
    Reply(String),
}

/// A step given out, and how it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Work {
    /// Who may do it.
    to: Vec<String>,
    /// Who took it on, by claiming it or moving it.
    by: Option<String>,
    /// The stage it's at, among its step's.
    stage: Option<usize>,
    /// What moved it there.
    detail: Option<String>,
    /// The branch its assignee pushed for it, whose landing it follows.
    branch: Option<String>,
    /// When it last moved, or anything was heard of whoever has it, or they were nudged.
    since: u64,
    /// Whether they were nudged since.
    nudged: bool,
    /// Everyone it went to so far, a role's members one after another.
    tried: Vec<String>,
}

impl Work {
    /// Whether `by` may do it: it went to them, or they took it on.
    fn has(&self, by: &Actor) -> bool {
        by.is_in(&self.to) || self.by.as_deref() == Some(by.name().as_str())
    }
}

/// Something someone did, as [`Input::Activity`] says it.
#[derive(Clone, Copy)]
struct Seen<'a> {
    by: &'a Actor,
    did: Did,
    detail: Option<&'a str>,
    branch: Option<&'a str>,
    text: Option<&'a str>,
}

impl Seen<'_> {
    /// Whether it names run `id`, by its branch or its words, the id whole: `fix-flaky-7`
    /// names `flaky-7`, and `flaky-17` doesn't.
    fn names(&self, id: &str) -> bool {
        [self.branch, self.text]
            .into_iter()
            .flatten()
            .any(|said| names_whole(said, id))
    }
}

/// Whether `id` stands in `said` as a word of its own, not inside a longer name.
fn names_whole(said: &str, id: &str) -> bool {
    let apart = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    said.match_indices(id).any(|(at, _)| {
        apart(said[..at].chars().next_back()) && apart(said[at + id.len()..].chars().next())
    })
}

/// Notes that something was heard of `by` at `at`: when `run`'s step given out is theirs,
/// it isn't quiet.
fn heard(run: &mut Run, by: &Actor, at: u64) {
    let Some(i) = run.current() else { return };
    if let Status::Working(work) = &mut run.steps[i]
        && work.has(by)
    {
        work.since = work.since.max(at);
        work.nudged = false;
    }
}

/// How a step of a run stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    /// Not reached yet.
    Ahead,
    /// Given out: a task or approval to someone, a wait to nobody.
    Working(Work),
    /// Answered yes, counting at `counts_at`.
    Held { counts_at: u64, to: Vec<String> },
    /// Done, with its outputs.
    Done(BTreeMap<String, String>),
    /// Didn't run.
    Skipped,
}

/// A run, as its events say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    id: String,
    by: String,
    inputs: BTreeMap<String, String>,
    steps: Vec<Status>,
    escalated: Vec<bool>,
    /// Who last did a step, whom a wait's nudge asks.
    last: Option<String>,
    finished: Option<(Outcome, Option<String>)>,
    /// When it started, and when it ended, in unix seconds.
    started: u64,
    ended: Option<u64>,
    /// For a run that is one holder's part of a step given to several ([`Step::each`]).
    part: Option<Part>,
}

/// One holder's part of a step given to several: a run of its own under the run the step is
/// of, which does that step alone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Part {
    /// The run the step is of.
    parent: String,
    /// The step, by its place.
    step: usize,
    /// Whose part it is, as the step's `each` names them.
    each: String,
}

/// Who started a part ([`Change::Started`]'s `by`): `each:`, the run it's part of, the
/// step's id, and whose it is.
const PART: &str = "each:";

fn part_by(parent: &str, step: &str, each: &str) -> String {
    format!("{PART}{parent}:{step}:{each}")
}

/// The run, step and holder `by` says a part is of, if it's a part's.
fn part_of(by: &str) -> Option<(&str, &str, &str)> {
    let mut parts = by.strip_prefix(PART)?.splitn(3, ':');
    Some((parts.next()?, parts.next()?, parts.next()?))
}

impl Run {
    /// The step it's on: the first neither done nor skipped; for a part, its one step.
    fn current(&self) -> Option<usize> {
        let open = |s: &Status| !matches!(s, Status::Done(_) | Status::Skipped);
        match &self.part {
            Some(part) => open(&self.steps[part.step]).then_some(part.step),
            None => self.steps.iter().position(open),
        }
    }

    fn value(&self, workflow: &Workflow, r: &Ref) -> Option<String> {
        match r {
            Ref::Input(name) => self.inputs.get(name).cloned(),
            Ref::Output { step, output } => {
                let i = workflow.steps.iter().position(|s| s.id == *step)?;
                match self.steps.get(i)? {
                    Status::Done(outputs) => outputs.get(output).cloned(),
                    _ => None,
                }
            }
        }
    }

    /// The step it's on and how it goes, if it's given out.
    fn work(&self) -> Option<(usize, &Work)> {
        let i = self.current()?;
        match &self.steps[i] {
            Status::Working(work) => Some((i, work)),
            _ => None,
        }
    }
}

/// A workflow's runs, and what decides their next step.
#[derive(Debug, Clone)]
pub struct Engine {
    workflow: Workflow,
    version: String,
    runs: BTreeMap<String, Run>,
    numbers: u64,
    states: BTreeMap<String, Level>,
    /// The time of what it's answering.
    now: u64,
    /// Each role's live members, as the host last said.
    members: BTreeMap<String, Vec<Member>>,
    /// The time each `every` trigger last came round and was dealt with: a run started, or
    /// the time passed over.
    fired: BTreeMap<usize, u64>,
    /// Whether its own channel's history has been read back ([`Engine::caught_up`]): until
    /// then it can't tell which times it dealt with already, and starts no run for one.
    caught_up: bool,
}

impl Engine {
    /// An engine for `workflow`, whose file is at `version`, with no runs yet.
    #[must_use]
    pub fn new(workflow: Workflow, version: String) -> Self {
        Self {
            workflow,
            version,
            runs: BTreeMap::new(),
            numbers: 0,
            states: BTreeMap::new(),
            now: 0,
            members: BTreeMap::new(),
            fired: BTreeMap::new(),
            caught_up: false,
        }
    }

    /// The time each of its `every` triggers last came round and was dealt with, by the
    /// trigger's place among `on`: what a deploy of the workflow again carries over
    /// ([`Engine::restore_fired`]), so the time it's in isn't dealt with twice.
    #[must_use]
    pub fn fired(&self) -> BTreeMap<usize, u64> {
        self.fired.clone()
    }

    /// Notes that its channel's history has been read back, runs a time started among it:
    /// from now, a time that comes round starts a run.
    pub const fn caught_up(&mut self) {
        self.caught_up = true;
    }

    /// Whether it has ([`Engine::caught_up`]).
    #[must_use]
    pub const fn is_caught_up(&self) -> bool {
        self.caught_up
    }

    /// Takes over what the engine before it had dealt with ([`Engine::fired`]).
    pub fn restore_fired(&mut self, fired: &BTreeMap<usize, u64>) {
        for (trigger, at) in fired {
            let known = self.fired.entry(*trigger).or_default();
            *known = (*known).max(*at);
        }
    }

    /// Each role's live members, as the host knows them now: what its role steps go to.
    pub fn set_members(&mut self, members: BTreeMap<String, Vec<Member>>) {
        self.members = members;
    }

    /// Who has one of its steps now: what makes a member busy.
    #[must_use]
    pub fn holders(&self) -> Vec<String> {
        self.runs
            .values()
            .filter(|r| r.finished.is_none())
            .filter_map(|r| match r.work()?.1 {
                Work { by: Some(by), .. } => Some(by.clone()),
                Work { to, .. } if to.len() == 1 => Some(to[0].clone()),
                _ => None,
            })
            .collect()
    }

    /// The live member of `role` to give a step to next, but those in `tried`: the least
    /// busy, then the one longest since their last.
    fn pick(&self, role: &str, tried: &[String]) -> Option<String> {
        self.members
            .get(role)?
            .iter()
            .filter(|m| !tried.contains(&m.name))
            .min_by(|a, b| (a.busy, a.last, &a.name).cmp(&(b.busy, b.last, &b.name)))
            .map(|m| m.name.clone())
    }

    /// Who `role`'s people are now: each person it names, and for a `role:<name>` one of
    /// that role's live members not tried yet; the people after a `role:` stand in for it only
    /// while it has nobody.
    fn fallback(&self, role: &str, tried: &[String]) -> Vec<String> {
        let mut people = Vec::new();
        let mut covered = false;
        for member in self.workflow.roles.get(role).into_iter().flatten() {
            match member.strip_prefix("role:") {
                Some(live) => match self.pick(live, tried) {
                    Some(picked) => {
                        people.push(picked);
                        covered = true;
                    }
                    None => covered = false,
                },
                None if covered => {}
                None => people.push(member.clone()),
            }
        }
        people
    }

    /// Step `i` as `run` does it: for a part of a step given to several, that step going to
    /// the one whose part it is.
    fn step(&self, run: &Run, i: usize) -> Step {
        let mut step = self.workflow.steps[i].clone();
        if let Some(part) = run.part.as_ref().filter(|part| part.step == i) {
            step.who = Some(part.each.clone());
            step.each = None;
        }
        step
    }

    /// Whether `run` waits at step `i` for its parts: the step is given to several, and this
    /// is the run it's of.
    fn waits_on_parts(&self, run: &Run, i: usize) -> bool {
        run.part.is_none() && self.workflow.steps[i].each.is_some()
    }

    /// The parts of step `i` of run `id`, in the order they were given out.
    fn parts(&self, id: &str, i: usize) -> Vec<&Run> {
        let mut parts: Vec<&Run> = self
            .runs
            .values()
            .filter(|r| {
                r.part
                    .as_ref()
                    .is_some_and(|p| p.parent == id && p.step == i)
            })
            .collect();
        parts.sort_by_key(|r| r.id.rsplit('.').next().and_then(|n| n.parse::<u64>().ok()));
        parts
    }

    /// Everyone step `step` of `run` goes to at once: those its `each` names.
    fn each_of(run: &Run, step: &Step) -> Vec<String> {
        let mut names: Vec<String> = match &step.each {
            Some(Each::Named(names)) => names.clone(),
            Some(Each::Input(input)) => who(
                run,
                &Step {
                    who: Some(input.clone()),
                    ..step.clone()
                },
            ),
            None => Vec::new(),
        };
        let mut seen = std::collections::BTreeSet::new();
        names.retain(|name| seen.insert(name.clone()));
        names.truncate(MAX_EACH);
        names
    }

    /// The workflow it runs.
    #[must_use]
    pub fn workflow(&self) -> &Workflow {
        &self.workflow
    }

    /// Reads back a run's event its nick posted before, oldest first, as from the channel's
    /// history: it changes the engine as it did when it was made.
    pub fn restore(&mut self, event: &RunEvent) {
        self.restore_change(&event.run, &event.change, event.at);
    }

    /// Reads back a state said before, oldest first, so a wait that starts later knows how
    /// things stand.
    pub fn restore_state(&mut self, state: &State) {
        self.states.insert(state.name.clone(), state.level);
    }

    /// Reads back something `by` did at `at`, oldest first: whatever moved a task was said
    /// in its own events, so this only says they weren't quiet.
    pub fn restore_activity(&mut self, by: &Actor, at: u64) {
        for run in self.runs.values_mut().filter(|r| r.finished.is_none()) {
            heard(run, by, at);
        }
    }

    /// Reads back a change its nick posted before, without the card, as kept to rebuild the
    /// engine when its workflow is deployed again.
    pub fn restore_change(&mut self, run: &str, change: &Change, at: u64) {
        // A run it has already: read back again, as after a reconnect's catch-up, its start
        // would put it back to its first step and give that again.
        if matches!(change, Change::Started { .. }) && self.runs.contains_key(run) {
            return;
        }
        self.apply(run, change, at);
    }

    /// The runs not finished yet, by id.
    #[must_use]
    pub fn running(&self) -> Vec<String> {
        self.runs
            .values()
            .filter(|r| r.finished.is_none())
            .map(|r| r.id.clone())
            .collect()
    }

    /// The runs not finished yet and where each stands, oldest first: what a dashboard lists.
    #[must_use]
    pub fn going(&self) -> Vec<zirc_proto::workflow::Going> {
        let mut going: Vec<_> = self
            .runs
            .values()
            .filter(|r| r.finished.is_none())
            .filter_map(|r| Some((r, r.current()?)))
            .map(|(r, i)| {
                let step = &self.step(r, i);
                let (to, stage, since) = match &r.steps[i] {
                    Status::Working(work) => (
                        work.by
                            .clone()
                            .map_or_else(|| work.to.clone(), |by| vec![by]),
                        work.stage
                            .and_then(|at| step.stages.get(at))
                            .map(|stage| stage.name.clone()),
                        Some(work.since),
                    ),
                    Status::Held { to, .. } => (to.clone(), None, None),
                    _ => (Vec::new(), None, None),
                };
                zirc_proto::workflow::Going {
                    run: r.id.clone(),
                    step: step.id.clone(),
                    to,
                    stage,
                    since,
                }
            })
            .collect();
        // By number, not by name: master-10 after master-9, and its parts after it.
        going.sort_by_key(|g| {
            let number = g.run.rsplit('-').next().unwrap_or_default();
            let (run, part) = number.split_once('.').unwrap_or((number, "0"));
            (
                run.parse::<u64>().ok(),
                part.parse::<u64>().ok(),
                g.run.clone(),
            )
        });
        going
    }

    /// Its runs that have ended, the latest first, at most [`MAX_ENDED`]: what a dashboard
    /// lists as its recent runs. A part of a step given to several is its run's, not one of
    /// its own.
    #[must_use]
    pub fn ended(&self) -> Vec<zirc_proto::workflow::Ended> {
        let mut ended: Vec<_> = self
            .runs
            .values()
            .filter(|r| r.part.is_none())
            .filter_map(|r| {
                let (outcome, why) = r.finished.clone()?;
                Some(zirc_proto::workflow::Ended {
                    run: r.id.clone(),
                    outcome,
                    why,
                    started: r.started,
                    ended: r.ended?,
                })
            })
            .collect();
        ended.sort_by(|a, b| b.ended.cmp(&a.ended).then_with(|| b.run.cmp(&a.run)));
        ended.truncate(MAX_ENDED);
        ended
    }

    /// The run and step answering 👍 or 👎 on a message of `run` would answer: its approval
    /// step, if that's where it stands.
    #[must_use]
    pub fn approval(&self, run: &str) -> Option<String> {
        let run = self.runs.get(run).filter(|r| r.finished.is_none())?;
        let i = run.current()?;
        let step = &self.workflow.steps[i];
        (step.kind == StepKind::Approve).then(|| step.id.clone())
    }

    /// When the time passing next moves a run on, after `after`: a held yes counting, or a
    /// quiet step's nudge or escalation. `None` while nothing waits on the time, so whoever
    /// runs the engine sleeps until then rather than ticking.
    #[must_use]
    pub fn next_due(&self, after: u64) -> Option<u64> {
        let time = |t: &Option<String>| t.as_deref().and_then(|t| seconds(t).ok());
        self.runs
            .values()
            .filter(|run| run.finished.is_none())
            .flat_map(|run| {
                let Some(i) = run.current() else {
                    return Vec::new();
                };
                let step = &self.workflow.steps[i];
                if self.waits_on_parts(run, i) {
                    return Vec::new();
                }
                match &run.steps[i] {
                    Status::Held { counts_at, .. } => vec![*counts_at],
                    Status::Working(work) => [time(&step.nudge), time(&step.escalate)]
                        .into_iter()
                        .flatten()
                        .map(|wait| work.since + wait)
                        .collect(),
                    _ => Vec::new(),
                }
            })
            .chain(
                self.workflow
                    .on
                    .iter()
                    .filter_map(|t| t.schedule().ok().flatten())
                    .map(|schedule| schedule.next(after)),
            )
            .filter(|&due| due > after)
            .min()
    }

    /// What its nick says to `input`, at `now` (unix seconds).
    pub fn handle(&mut self, input: Input, now: u64) -> Vec<Said> {
        self.now = now;
        let mut said = Vec::new();
        match input {
            Input::Mention { by, text } => self.mention(&by, &text, now, &mut said),
            Input::State(state) => {
                self.states.insert(state.name.clone(), state.level);
                self.triggered(&state, now, &mut said);
                for id in self.running() {
                    self.advance(&id, now, &mut said);
                }
            }
            Input::Action { by, action } => self.act(&by, action, now, &mut said),
            Input::Activity {
                by,
                did,
                detail,
                branch,
                text,
            } => {
                let seen = Seen {
                    by: &by,
                    did,
                    detail: detail.as_deref(),
                    branch: branch.as_deref(),
                    text: text.as_deref(),
                };
                let (moved, heard_only) = self.concerned(&seen);
                for id in heard_only {
                    if let Some(run) = self.runs.get_mut(&id) {
                        heard(run, &by, now);
                    }
                }
                for id in moved {
                    self.follow(&id, &seen, now, &mut said);
                }
            }
            Input::Said {
                by,
                channel,
                id,
                text,
            } => {
                let lower = text.to_lowercase();
                let known = fields([("text", Some(text)), ("by", Some(by.clone()))])
                    .chain(fields([("channel", Some(channel)), ("id", Some(id))]))
                    .collect();
                let hears = |t: &Trigger| {
                    t.said
                        .as_deref()
                        .is_some_and(|said| names_whole(&lower, &said.to_lowercase()))
                };
                self.happened(TriggerKind::Said, &hears, &by, &known, now, &mut said);
            }
            Input::Reacted {
                by,
                reaction,
                channel,
                id,
                text,
                author,
            } => {
                let known = fields([("text", text), ("author", author), ("by", Some(by.clone()))])
                    .chain(fields([("channel", Some(channel)), ("id", Some(id))]))
                    .collect();
                let is = |t: &Trigger| t.reaction.as_deref() == Some(reaction.as_str());
                self.happened(TriggerKind::Reaction, &is, &by, &known, now, &mut said);
            }
            Input::Landed { landing, changed } => {
                let commit = landing.get("commit").map_or("", String::as_str);
                let by = format!("landing:{commit}");
                let under = |t: &Trigger| landed_under(&t.landed, &changed);
                self.happened(TriggerKind::Landed, &under, &by, &landing, now, &mut said);
            }
            Input::Tick => {
                self.scheduled(now, &mut said);
                for id in self.running() {
                    self.dispatch(&id, &mut said);
                    self.stall(&id, now, &mut said);
                    self.advance(&id, now, &mut said);
                }
            }
        }
        said
    }

    fn mention(&mut self, by: &Actor, text: &str, now: u64, said: &mut Vec<Said>) {
        let text = text.trim();
        let (word, rest) = text.split_once(' ').unwrap_or((text, ""));
        match word {
            "run" | "start" => match self.inputs_of(rest) {
                Ok(inputs) => self.start(by.name(), inputs, now, said),
                Err(why) => said.push(Said::Reply(format!("{}: {why}", by.name()))),
            },
            "cancel" => {
                let id = rest.trim();
                if self.runs.get(id).is_some_and(|r| r.finished.is_none()) {
                    self.finish(
                        id,
                        Outcome::Cancelled,
                        Some(format!("cancelled by {}", by.name())),
                        said,
                    );
                } else {
                    said.push(Said::Reply(format!(
                        "{}: there's no run {id:?} going",
                        by.name()
                    )));
                }
            }
            _ => said.push(Said::Reply(format!(
                "{}: I'm the `{}` workflow. Say `run` with its inputs as name=value ({}), \
                 or `cancel <run>`.",
                by.name(),
                self.workflow.name,
                self.input_names()
            ))),
        }
    }

    fn input_names(&self) -> String {
        if self.workflow.inputs.is_empty() {
            return "it takes none".to_owned();
        }
        self.workflow
            .inputs
            .iter()
            .map(|i| {
                if i.optional {
                    format!("{}, optional", i.name)
                } else {
                    i.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The inputs `text` gives as `name=value` or `name="a value"`.
    fn inputs_of(&self, text: &str) -> Result<BTreeMap<String, String>, String> {
        let mut inputs = BTreeMap::new();
        let mut rest = text.trim();
        while !rest.is_empty() {
            let (name, after) = rest
                .split_once('=')
                .ok_or_else(|| format!("{rest:?} isn't name=value"))?;
            let name = name.trim();
            if !self.workflow.inputs.iter().any(|i| i.name == name) {
                return Err(format!(
                    "{name:?} isn't one of its inputs ({})",
                    self.input_names()
                ));
            }
            let (value, after) = match after.strip_prefix('"') {
                Some(quoted) => {
                    let end = quoted
                        .find('"')
                        .ok_or_else(|| format!("the quote after {name}= isn't closed"))?;
                    (&quoted[..end], &quoted[end + 1..])
                }
                None => after.split_once(' ').unwrap_or((after, "")),
            };
            inputs.insert(name.to_owned(), value.to_owned());
            rest = after.trim_start();
        }
        if let Some(missing) = self
            .workflow
            .inputs
            .iter()
            .find(|i| !i.optional && inputs.get(&i.name).is_none_or(String::is_empty))
        {
            return Err(format!("it needs {}=…", missing.name));
        }
        Ok(inputs)
    }

    fn triggered(&mut self, state: &State, now: u64, said: &mut Vec<Said>) {
        let by = format!("state:{}", state.name);
        let starts: Vec<BTreeMap<String, String>> = self
            .workflow
            .on
            .iter()
            .filter(|t| t.state.as_deref() == Some(&state.name) && t.level == Some(state.level))
            .map(|t| {
                t.inputs
                    .iter()
                    .filter_map(|(input, field)| {
                        let value = match field.as_str() {
                            "repo" => Some(state.repo.clone()),
                            "name" => Some(state.name.clone()),
                            "why" => state.why.clone(),
                            "owner" => state.owner.clone(),
                            "link" => state.link.clone(),
                            _ => None,
                        };
                        value.map(|v| (input.clone(), v))
                    })
                    .collect()
            })
            .collect();
        for inputs in starts {
            // A state said again while its run goes on starts nothing new.
            if self
                .runs
                .values()
                .any(|r| r.finished.is_none() && r.by == by)
            {
                continue;
            }
            if let Some(missing) = self
                .workflow
                .inputs
                .iter()
                .find(|i| !i.optional && !inputs.contains_key(&i.name))
            {
                said.push(Said::Reply(format!(
                    "the {} state is {}, but it doesn't say the {} this workflow needs",
                    state.name,
                    state.level.word(),
                    missing.name
                )));
                continue;
            }
            self.start(by.clone(), inputs, now, said);
        }
    }

    /// Starts a run for each trigger of `kind` that `matches`, started by `by` and given
    /// what's `known` of what happened, as the trigger's `inputs` read it. One whose run needs
    /// an input that isn't known, as the text of a message too old to have, starts nothing.
    fn happened(
        &mut self,
        kind: TriggerKind,
        matches: &dyn Fn(&Trigger) -> bool,
        by: &str,
        known: &BTreeMap<String, String>,
        now: u64,
        said: &mut Vec<Said>,
    ) {
        for trigger in self.workflow.on.clone() {
            if trigger.kind() != Ok(kind) || !matches(&trigger) {
                continue;
            }
            let inputs: BTreeMap<String, String> = trigger
                .inputs
                .iter()
                .filter_map(|(input, field)| Some((input.clone(), known.get(field)?.clone())))
                .collect();
            let missing = self
                .workflow
                .inputs
                .iter()
                .find(|i| !i.optional && !inputs.contains_key(&i.name));
            if let Some(missing) = missing {
                said.push(Said::Reply(format!(
                    "{by}: that starts no run, as its {} isn't known any more",
                    missing.name
                )));
                continue;
            }
            self.start(by.to_owned(), inputs, now, said);
        }
    }

    /// Starts a run for each `every` trigger whose time has come round since it was last
    /// dealt with, if not more than [`EVERY_GRACE`] ago: a time the server was away for
    /// longer is passed over, never made up for.
    fn scheduled(&mut self, now: u64, said: &mut Vec<Said>) {
        if !self.caught_up {
            return;
        }
        for (i, trigger) in self.workflow.on.clone().iter().enumerate() {
            let Some(at) = trigger
                .schedule()
                .ok()
                .flatten()
                .and_then(|schedule| schedule.last(now))
            else {
                continue;
            };
            if self.fired.get(&i).is_some_and(|dealt| *dealt >= at) {
                continue;
            }
            self.fired.insert(i, at);
            if now - at <= EVERY_GRACE {
                self.start(scheduled_by(i, at), trigger.with.clone(), now, said);
            }
        }
    }

    fn start(
        &mut self,
        by: String,
        inputs: BTreeMap<String, String>,
        now: u64,
        said: &mut Vec<Said>,
    ) {
        // Asked again for what a run goes on with, as CI saying a flaky test failed again
        // while it's being fixed: that run is said, and nothing new starts.
        if let Some(input) = &self.workflow.one_run_per
            && let Some(value) = inputs.get(input)
            && let Some(run) = self
                .runs
                .values()
                .find(|r| r.finished.is_none() && r.inputs.get(input) == Some(value))
        {
            let text = format!("`{}` is on {input}={value} already", run.id);
            said.push(Said::Reply(text));
            return;
        }
        let id = format!("{}-{}", self.workflow.name, self.numbers + 1);
        let text = format!(
            "started `{id}`{}",
            if ["state:", SCHEDULE, "landing:"]
                .iter()
                .any(|what| by.starts_with(what))
            {
                String::new()
            } else {
                format!(" for {by}")
            }
        );
        self.say(
            &id,
            Change::Started {
                by,
                inputs,
                version: self.version.clone(),
            },
            text,
            said,
        );
        self.advance(&id, now, said);
    }

    fn act(&mut self, by: &Actor, action: StepAction, now: u64, said: &mut Vec<Said>) {
        let refuse = |said: &mut Vec<Said>, why: String| {
            said.push(Said::Reply(format!("{}: {why}", by.name())));
        };
        let Some(run) = self.runs.get(&action.run).filter(|r| r.finished.is_none()) else {
            return refuse(said, format!("there's no run {:?} going", action.run));
        };
        if action.act == Act::Cancel {
            let why = Some(format!("cancelled by {}", by.name()));
            return self.finish(&action.run, Outcome::Cancelled, why, said);
        }
        let Some(i) = run.current() else {
            return refuse(said, format!("{} has nothing left to do", action.run));
        };
        let step = self.step(run, i);
        let step = &step;
        if step.id != action.step {
            return refuse(
                said,
                format!("{} is on `{}`, not `{}`", action.run, step.id, action.step),
            );
        }
        let (to, claimed) = match &run.steps[i] {
            Status::Working(work) => (work.to.clone(), work.by.clone()),
            Status::Held { to, .. } => (to.clone(), None),
            _ => return refuse(said, format!("`{}` isn't waiting on anyone", step.id)),
        };
        if !by.is_in(&to) && claimed.as_deref() != Some(by.name().as_str()) {
            return refuse(
                said,
                format!("`{}` of {} is for {}", step.id, action.run, to.join(", ")),
            );
        }
        let run_id = action.run.clone();
        let step_id = step.id.clone();
        match action.act {
            Act::Claim if step.kind == StepKind::Task => {
                if let Some(other) = claimed.filter(|c| *c != by.name()) {
                    return refuse(said, format!("{other} has `{step_id}` already"));
                }
                let text = format!("{} took `{step_id}` of `{run_id}`", by.name());
                let change = Change::Claimed {
                    step: step_id,
                    by: by.name(),
                };
                self.say(&run_id, change, text, said);
            }
            Act::Done { outputs } if step.kind == StepKind::Task => {
                if let Err(why) = check_outputs(step, &outputs) {
                    return refuse(said, why);
                }
                let text = format!("{} finished `{step_id}` of `{run_id}`", by.name());
                let change = Change::Done {
                    step: step_id,
                    by: by.name(),
                    outputs,
                };
                self.say(&run_id, change, text, said);
            }
            Act::Answer { yes } if step.kind == StepKind::Approve => {
                if !by.person {
                    return refuse(said, "only a person answers an approval".to_owned());
                }
                if !self.answer(by, &run_id, step, yes, now, said) {
                    return;
                }
            }
            Act::Fail { why } => {
                let why = Some(format!("`{step_id}` failed: {why}"));
                return self.finish(&run_id, Outcome::Failed, why, said);
            }
            _ => {
                return refuse(
                    said,
                    format!(
                        "`{step_id}` is a step of kind {}, which that doesn't answer",
                        step.kind.word()
                    ),
                );
            }
        }
        self.advance(&run_id, now, said);
    }

    /// Says `by`'s answer to approval `step` of `run_id`; whether the run goes on.
    fn answer(
        &mut self,
        by: &Actor,
        run_id: &str,
        step: &Step,
        yes: bool,
        now: u64,
        said: &mut Vec<Said>,
    ) -> bool {
        let hold = zirc_proto::request::HOLD_SECONDS;
        let counts_at = (yes && step.irreversible).then(|| now + hold);
        let text = format!(
            "{} said {} to `{}` of `{run_id}`{}",
            by.name(),
            if yes { "yes" } else { "no" },
            step.id,
            if counts_at.is_some() {
                format!(", which counts in {hold}s unless they say no")
            } else {
                String::new()
            }
        );
        let change = Change::Answered {
            step: step.id.clone(),
            by: by.name(),
            yes,
            counts_at,
        };
        self.say(run_id, change, text, said);
        if !yes {
            let why = Some(format!("{} said no to `{}`", by.name(), step.id));
            self.finish(run_id, Outcome::Cancelled, why, said);
        }
        yes
    }

    /// Which running runs what `seen` did moves on, and which it only shows they're not
    /// quiet in. Someone with one task has everything they do counted for it. With several,
    /// only what names a run counts for it, by its branch or its words, else for the one
    /// they took on if there's one: a fixer given six flakes at once once had its post, push
    /// and landing for one counted for all six, and five closed as fixed with nothing done.
    fn concerned(&self, seen: &Seen<'_>) -> (Vec<String>, Vec<String>) {
        let theirs: Vec<String> = self
            .running()
            .into_iter()
            .filter(|id| {
                self.runs[id].work().is_some_and(|(i, work)| {
                    self.workflow.steps[i].kind == StepKind::Task && work.has(seen.by)
                })
            })
            .collect();
        if theirs.len() < 2 {
            return (self.running(), Vec::new());
        }
        let mut named: Vec<String> = theirs.iter().filter(|id| seen.names(id)).cloned().collect();
        if named.is_empty() {
            let me = seen.by.name();
            let took: Vec<&String> = theirs
                .iter()
                .filter(|id| {
                    self.runs[*id]
                        .work()
                        .is_some_and(|(_, work)| work.by.as_deref() == Some(me.as_str()))
                })
                .collect();
            if let [only] = took[..] {
                named.push(only.clone());
            }
        }
        let others = theirs
            .into_iter()
            .filter(|id| !named.contains(id))
            .collect();
        (named, others)
    }

    /// Follows what `by` did on run `id`'s task, if it's theirs: on to the next stage it
    /// reaches, back before its landing if that failed, or at least not quiet.
    fn follow(&mut self, id: &str, seen: &Seen<'_>, now: u64, said: &mut Vec<Said>) {
        let Seen {
            by,
            did,
            detail,
            branch,
            ..
        } = *seen;
        let Some(run) = self.runs.get(id) else { return };
        let Some((i, work)) = run.work() else { return };
        let step = self.step(run, i);
        if step.kind != StepKind::Task || !work.has(by) {
            return;
        }
        // A landing of another branch than the one pushed for it is other work of theirs.
        let landing_did = matches!(did, Did::LandingQueued | Did::Landed | Did::LandingFailed);
        if landing_did
            && let (Some(pushed), Some(branch)) = (&work.branch, branch)
            && pushed != branch
        {
            return;
        }
        let from = work.stage.map_or(0, |s| s + 1);
        let current = work.stage;
        let reached = did.signal().and_then(|signal| {
            step.stages
                .iter()
                .skip(from)
                .position(|s| s.on == signal)
                .map(|j| j + from)
        });
        let landing = step
            .stages
            .iter()
            .position(|s| s.on == Signal::LandingQueued);
        let detail = detail.map(str::to_owned);
        if let Some(j) = reached {
            if j + 1 == step.stages.len() {
                let text = format!(
                    "`{id}` · `{}`: {}{}, so it's done",
                    step.id,
                    step.stages[j].name,
                    bracketed(detail.as_deref())
                );
                let change = Change::Done {
                    step: step.id.clone(),
                    by: by.name(),
                    outputs: BTreeMap::new(),
                };
                self.say(id, change, text, said);
                return self.advance(id, now, said);
            }
            let stage = step.stages[j].name.clone();
            let text = format!(
                "`{id}` · `{}`: {stage}{}",
                step.id,
                bracketed(detail.as_deref())
            );
            let change = Change::Stage {
                step: step.id.clone(),
                stage: Some(stage),
                by: by.name(),
                detail,
            };
            return self.say(id, change, text, said);
        }
        if did == Did::LandingFailed
            && let Some(q) = landing.filter(|q| current.is_some_and(|s| s >= *q))
        {
            let back = q.checked_sub(1).map(|s| step.stages[s].name.clone());
            let why = format!("landing failed{}", bracketed(detail.as_deref()));
            let text = format!(
                "`{id}` · `{}`: {why}, back to {}",
                step.id,
                back.as_deref().unwrap_or("the start")
            );
            let change = Change::Stage {
                step: step.id.clone(),
                stage: back,
                by: by.name(),
                detail: Some(why),
            };
            return self.say(id, change, text, said);
        }
        // Nothing new, but they're on it: not quiet.
        if let Some(run) = self.runs.get_mut(id) {
            heard(run, by, now);
        }
    }

    /// Gives run `id`'s step waiting for a role's first member to one, once there is one.
    fn dispatch(&mut self, id: &str, said: &mut Vec<Said>) {
        let Some(run) = self.runs.get(id) else { return };
        let Some((i, work)) = run.work() else { return };
        let step = self.step(run, i);
        if !work.to.is_empty() || run.escalated[i] || self.waits_on_parts(run, i) {
            return;
        }
        let Some(role) = role_of(&step) else { return };
        let Some(member) = self.pick(&role, &work.tried) else {
            return;
        };
        let to = vec![member];
        let text = given(id, &step, &to, &self.says(id, &step));
        let change = Change::Assigned {
            step: step.id.clone(),
            to,
        };
        self.say(id, change, text, said);
    }

    /// Nudges run `id`'s step when it's been quiet past its `nudge`, and gives it to its
    /// fallback, or fails the run, when it's been quiet past its `escalate` after that.
    fn stall(&mut self, id: &str, now: u64, said: &mut Vec<Said>) {
        let Some(run) = self.runs.get(id) else { return };
        let Some((i, work)) = run.work() else { return };
        // A step given to several is followed in its parts, each a run of its own.
        if self.waits_on_parts(run, i) {
            return;
        }
        let work = work.clone();
        let step = self.step(run, i);
        let time = |t: &Option<String>| t.as_deref().and_then(|t| seconds(t).ok());
        let (nudge, escalate) = (time(&step.nudge), time(&step.escalate));
        let at = stage_words(&step, &work);
        let whom = match (step.kind, &work.by, &run.last) {
            (StepKind::Wait, _, Some(last)) => vec![last.clone()],
            (_, Some(by), _) => vec![by.clone()],
            _ => work.to.clone(),
        };
        // A role's member quiet past the nudge: over to the next, while there is one.
        if let Some(n) = nudge
            && now >= work.since + n
            && !run.escalated[i]
            && !work.to.is_empty()
            && let Some(role) = role_of(&step)
            && let Some(next) = self.pick(&role, &work.tried)
        {
            let to = vec![next];
            let text = format!(
                "`{id}` · `{}` was quiet with {} {at}: over to {}. {}",
                step.id,
                mentions(&work.to),
                mentions(&to),
                given(id, &step, &to, &self.says(id, &step))
            );
            let change = Change::Assigned {
                step: step.id.clone(),
                to,
            };
            return self.say(id, change, text, said);
        }
        if let Some(n) = nudge
            && !work.nudged
            && !whom.is_empty()
            && now >= work.since + n
        {
            let text = format!(
                "`{id}` · `{}` has been quiet for {} {at}: {}, how's it going?",
                step.id,
                step.nudge.as_deref().unwrap_or_default(),
                mentions(&whom)
            );
            return self.say(id, Change::Nudged { step: step.id }, text, said);
        }
        // Nobody to nudge, as a step waiting for a role's first member: straight on.
        let quiet_past =
            |e: u64| (work.nudged || nudge.is_none() || whom.is_empty()) && now >= work.since + e;
        if !escalate.is_some_and(quiet_past) {
            return;
        }
        let Some(role) = step.fallback.as_ref().filter(|_| !run.escalated[i]) else {
            let why = Some(format!("`{}` stalled {at}", step.id));
            return self.finish(id, Outcome::Failed, why, said);
        };
        // The fallback joins whoever had it, who may still finish it.
        let fallback = self.fallback(role, &work.tried);
        let mut to = work.to.clone();
        to.extend(work.by.clone());
        to.extend(fallback.iter().cloned());
        let mut seen = std::collections::BTreeSet::new();
        to.retain(|name| seen.insert(name.clone()));
        let text = format!(
            "`{id}` · `{}` hasn't moved {at}: {}, over to you. {}",
            step.id,
            mentions(&fallback),
            self.says(id, &step)
        );
        let change = Change::Escalated {
            step: step.id.clone(),
            to,
        };
        self.say(id, change, text.trim_end().to_owned(), said);
    }

    /// Moves `id` on as far as it goes without anyone: skipping steps whose `when` doesn't
    /// hold, posting, finishing waits that are met, counting held yeses, and giving the next
    /// step to whoever does it.
    fn advance(&mut self, id: &str, now: u64, said: &mut Vec<Said>) {
        loop {
            let Some(run) = self.runs.get(id).filter(|r| r.finished.is_none()).cloned() else {
                return;
            };
            let run = &run;
            let Some(i) = run.current() else {
                return self.finish(id, Outcome::Done, None, said);
            };
            let step = self.step(run, i);
            let nick = self.workflow.nick.clone();
            match &run.steps[i] {
                Status::Ahead => {}
                Status::Working(_) if self.waits_on_parts(run, i) => {
                    if self.parts_ended(id, i, &step, said) {
                        continue;
                    }
                    return;
                }
                Status::Held { counts_at, .. } if now >= *counts_at => {
                    let text = format!("the yes to `{}` of `{id}` counts now", step.id);
                    let done = done(&step.id, &nick);
                    self.say(id, done, text, said);
                    continue;
                }
                Status::Working(_) if step.kind == StepKind::Wait => {
                    if self.met(&step) {
                        let text = format!("`{}` of `{id}`: it is", step.id);
                        self.say(id, done(&step.id, &nick), text, said);
                        continue;
                    }
                    return;
                }
                _ => return,
            }
            let holds = step.when.as_deref().is_none_or(|when| {
                Condition::parse(when).is_ok_and(|c| c.holds(|r| run.value(&self.workflow, r)))
            });
            if !holds {
                let text = format!("skipped `{}` of `{id}`", step.id);
                self.say(id, Change::Skipped { step: step.id }, text, said);
                continue;
            }
            let says = self.says(id, &step);
            match step.kind {
                StepKind::Post => {
                    self.say(id, done(&step.id, &nick), says, said);
                }
                StepKind::Wait => {
                    let until = step.until.as_ref().expect("a checked wait has an until");
                    let text = format!(
                        "`{id}` waits until {} is {}",
                        until.state,
                        until.level.word()
                    );
                    let change = Change::Assigned {
                        step: step.id.clone(),
                        to: Vec::new(),
                    };
                    self.say(id, change, text, said);
                }
                StepKind::Task if step.each.is_some() => {
                    return self.give_each(id, run, &step, &says, now, said);
                }
                StepKind::Task | StepKind::Approve => {
                    if let Some(role) = role_of(&step) {
                        let to: Vec<String> = self.pick(&role, &[]).into_iter().collect();
                        let text = if to.is_empty() {
                            waits_for(id, &step, &role)
                        } else {
                            given(id, &step, &to, &says)
                        };
                        let change = Change::Assigned {
                            step: step.id.clone(),
                            to,
                        };
                        return self.say(id, change, text, said);
                    }
                    return self.give_named(id, run, &step, &says, said);
                }
            }
        }
    }

    /// Gives `step` of run `id` to those it names, an input's or a nick; to its fallback when
    /// it names nobody; and fails the run when there's nobody at all.
    fn give_named(&mut self, id: &str, run: &Run, step: &Step, says: &str, said: &mut Vec<Said>) {
        let to = who(run, step);
        // Nobody named, as CI with too many suspects to name: straight to the fallback,
        // which is there for whoever named doesn't do it.
        let fallback = match &step.fallback {
            Some(role) if to.is_empty() => self.fallback(role, &[]),
            _ => Vec::new(),
        };
        if !fallback.is_empty() {
            let text = format!(
                "`{id}` · `{}` names nobody: {}, over to you. {says}",
                step.id,
                mentions(&fallback),
            );
            let change = Change::Escalated {
                step: step.id.clone(),
                to: fallback,
            };
            return self.say(id, change, text, said);
        }
        if to.is_empty() {
            let why = Some(format!("`{}` has nobody to do it", step.id));
            return self.finish(id, Outcome::Failed, why, said);
        }
        let text = given(id, step, &to, says);
        let change = Change::Assigned {
            step: step.id.clone(),
            to,
        };
        self.say(id, change, text, said);
    }

    /// Gives `step` of run `id` to each of those it names at once: a part for each, a run of
    /// its own that does this step alone, numbered under this one.
    fn give_each(
        &mut self,
        id: &str,
        run: &Run,
        step: &Step,
        says: &str,
        now: u64,
        said: &mut Vec<Said>,
    ) {
        let each = Self::each_of(run, step);
        if each.is_empty() {
            let why = Some(format!("`{}` has nobody to do it", step.id));
            return self.finish(id, Outcome::Failed, why, said);
        }
        let text = format!(
            "`{id}` · `{}` goes to each of {}, a run of its own for each: {}",
            step.id,
            each.join(", "),
            clip(says, 200)
        );
        // To nobody itself: its parts have its holders, and are followed for it.
        let change = Change::Assigned {
            step: step.id.clone(),
            to: Vec::new(),
        };
        self.say(id, change, text, said);
        for (n, whose) in each.iter().enumerate() {
            let part = format!("{id}.{}", n + 1);
            let mut inputs = run.inputs.clone();
            inputs.insert(EACH.to_owned(), whose.clone());
            let change = Change::Started {
                by: part_by(id, &step.id, whose),
                inputs,
                version: self.version.clone(),
            };
            let text = format!("started `{part}`: `{}` of `{id}` for {whose}", step.id);
            self.say(&part, change, text, said);
            self.advance(&part, now, said);
        }
    }

    /// Whether the parts of `step`, step `i` of run `id`, have all ended, and it's done:
    /// said so, for the run to go on. If one failed, the run fails instead, once all have
    /// ended or at once ([`Workflow::when_one_fails`]), and `false` is what's left to say.
    fn parts_ended(&mut self, id: &str, i: usize, step: &Step, said: &mut Vec<Said>) -> bool {
        let all = self.parts(id, i);
        let going = all.iter().filter(|r| r.finished.is_none()).count();
        let failed: Vec<String> = all
            .iter()
            .filter(|r| matches!(r.finished, Some((Outcome::Failed | Outcome::Cancelled, _))))
            .filter_map(|r| r.part.as_ref().map(|whose| whose.each.clone()))
            .collect();
        let at_once = self.workflow.when_one_fails == Some(OneFails::Stop);
        if !failed.is_empty() && (going == 0 || at_once) {
            let why = Some(format!("`{}` failed for {}", step.id, failed.join(", ")));
            self.finish(id, Outcome::Failed, why, said);
            return false;
        }
        if going > 0 {
            return false;
        }
        let text = format!("`{}` of `{id}`: all {} are done", step.id, all.len());
        let nick = self.workflow.nick.clone();
        self.say(id, done(&step.id, &nick), text, said);
        true
    }

    /// Whether a wait's state is as it waits for.
    fn met(&self, step: &Step) -> bool {
        step.until
            .as_ref()
            .is_some_and(|u| self.states.get(&u.state) == Some(&u.level))
    }

    fn says(&self, id: &str, step: &Step) -> String {
        let Some(run) = self.runs.get(id) else {
            return step.says.clone();
        };
        // An optional input a run wasn't given reads as nothing.
        fill(&step.says, |r| {
            run.value(&self.workflow, r)
                .or_else(|| matches!(r, Ref::Input(_)).then(String::new))
        })
    }

    fn finish(&mut self, id: &str, outcome: Outcome, why: Option<String>, said: &mut Vec<Said>) {
        let text = format!(
            "`{id}` {}{}",
            match outcome {
                Outcome::Done => "is done",
                Outcome::Failed => "failed",
                Outcome::Cancelled => "was cancelled",
            },
            why.as_deref().map(|w| format!(": {w}")).unwrap_or_default()
        );
        self.say(id, Change::Finished { outcome, why }, text, said);
        // A run that ends takes its parts still going with it; a part that ends may be the
        // last its run waited for.
        let parts: Vec<String> = self
            .runs
            .values()
            .filter(|r| r.finished.is_none())
            .filter(|r| r.part.as_ref().is_some_and(|part| part.parent == id))
            .map(|r| r.id.clone())
            .collect();
        for part in parts {
            let why = Some(format!("`{id}` ended"));
            self.finish(&part, Outcome::Cancelled, why, said);
        }
        if let Some(parent) = self.runs.get(id).and_then(|r| r.part.as_ref()) {
            let (parent, now) = (parent.parent.clone(), self.now);
            self.advance(&parent, now, said);
        }
    }

    /// Applies `change` to run `id` and says it, with the run's card as it stands after.
    fn say(&mut self, id: &str, change: Change, text: String, said: &mut Vec<Said>) {
        let at = self.now;
        self.apply(id, &change, at);
        let card = self.card(id);
        let event = RunEvent {
            run: id.to_owned(),
            at,
            change,
            card,
        };
        said.push(Said::Run(event.event(text)));
    }

    fn apply(&mut self, id: &str, change: &Change, at: u64) {
        self.numbered(id);
        if let Change::Started { by, inputs, .. } = change {
            // A run a time started: that time is dealt with, read back as much as said.
            if let Some((trigger, at)) = scheduled(by) {
                let dealt = self.fired.entry(trigger).or_default();
                *dealt = (*dealt).max(at);
            }
            self.started(id, by, inputs, at);
            return;
        }
        let i =
            step_of(change).and_then(|step| self.workflow.steps.iter().position(|s| s.id == step));
        let nick = self.workflow.nick.clone();
        let stages = i.map_or_else(Vec::new, |i| self.workflow.steps[i].stages.clone());
        let Some(run) = self.runs.get_mut(id) else {
            return;
        };
        if let Change::Finished { outcome, why } = change {
            run.finished = Some((*outcome, why.clone()));
            run.ended = Some(at);
            return;
        }
        let Some(i) = i else { return };
        let fresh = |to: &[String]| Work {
            to: to.to_vec(),
            by: None,
            stage: None,
            detail: None,
            branch: None,
            since: at,
            nudged: false,
            tried: Vec::new(),
        };
        let status = &mut run.steps[i];
        match (change, status) {
            (Change::Assigned { to, .. }, status) => {
                let mut work = fresh(to);
                if let Status::Working(before) = status {
                    work.tried.clone_from(&before.tried);
                }
                work.tried.extend(to.iter().cloned());
                *status = Status::Working(work);
            }
            (Change::Escalated { to, .. }, status) => {
                run.escalated[i] = true;
                let mut work = fresh(to);
                if let Status::Working(before) = status {
                    work.stage = before.stage;
                    work.detail.clone_from(&before.detail);
                    work.branch.clone_from(&before.branch);
                    work.tried.clone_from(&before.tried);
                }
                *status = Status::Working(work);
            }
            (Change::Claimed { by, .. }, Status::Working(work)) => {
                work.by = Some(by.clone());
                work.since = at;
                work.nudged = false;
            }
            (
                Change::Stage {
                    stage, by, detail, ..
                },
                Status::Working(work),
            ) => {
                work.stage = stage
                    .as_ref()
                    .and_then(|s| stages.iter().position(|n| n.name == *s));
                // A push names the branch whose landing the task follows from then on.
                if work.stage.is_some_and(|s| stages[s].on == Signal::Push) {
                    work.branch.clone_from(detail);
                }
                work.by = Some(by.clone());
                work.detail.clone_from(detail);
                work.since = at;
                work.nudged = false;
            }
            (Change::Nudged { .. }, Status::Working(work)) => {
                work.nudged = true;
                work.since = at;
            }
            (Change::Done { by, outputs, .. }, status) => {
                if *by != nick {
                    run.last = Some(by.clone());
                }
                *status = Status::Done(outputs.clone());
            }
            (Change::Answered { yes, counts_at, .. }, status) => {
                let to = match status {
                    Status::Working(work) => work.to.clone(),
                    Status::Held { to, .. } => to.clone(),
                    _ => Vec::new(),
                };
                match (yes, counts_at) {
                    (true, Some(at)) => *status = Status::Held { counts_at: *at, to },
                    (true, None) => *status = Status::Done(BTreeMap::new()),
                    (false, _) => {}
                }
            }
            (Change::Skipped { .. }, status) => *status = Status::Skipped,
            _ => {}
        }
    }

    /// Numbers the next run after `id`: any of a run's events does, though its start may be
    /// older than what was read back.
    fn numbered(&mut self, id: &str) {
        if let Some(n) = id
            .strip_prefix(&format!("{}-", self.workflow.name))
            .and_then(|n| n.parse::<u64>().ok())
        {
            self.numbers = self.numbers.max(n);
        }
    }

    fn started(&mut self, id: &str, by: &str, inputs: &BTreeMap<String, String>, at: u64) {
        let count = self.workflow.steps.len();
        let mut steps = vec![Status::Ahead; count];
        // A part of a step given to several: that step alone, knowing what the run it's
        // part of had done before it, whose outputs its words may read.
        let part = part_of(by).and_then(|(parent, step, each)| {
            let step = self.workflow.steps.iter().position(|s| s.id == step)?;
            steps = match self.runs.get(parent) {
                Some(parent) => parent.steps.clone(),
                None => vec![Status::Skipped; count],
            };
            steps[step] = Status::Ahead;
            Some(Part {
                parent: parent.to_owned(),
                step,
                each: each.to_owned(),
            })
        });
        self.runs.insert(
            id.to_owned(),
            Run {
                id: id.to_owned(),
                by: by.to_owned(),
                inputs: inputs.clone(),
                steps,
                escalated: vec![false; count],
                last: None,
                finished: None,
                started: at,
                ended: None,
                part,
            },
        );
    }

    /// Run `id`'s card, as it stands.
    fn card(&self, id: &str) -> Card {
        let run = &self.runs[id];
        let state = match run.finished {
            Some((Outcome::Done, _)) => CardState::Passed,
            Some((Outcome::Failed, _)) => CardState::Failed,
            Some((Outcome::Cancelled, _)) => CardState::Cancelled,
            None if run.steps.iter().all(|s| *s == Status::Ahead) => CardState::Pending,
            None if run.work().is_some_and(|(_, w)| w.nudged) => CardState::Warning,
            None => CardState::Running,
        };
        let current = run.current().unwrap_or(run.steps.len());
        let shown = 8.min(self.workflow.steps.len());
        let first = current
            .saturating_sub(shown / 2)
            .min(self.workflow.steps.len() - shown);
        let field = |i: usize| {
            let step = self.step(run, i);
            let value = if self.waits_on_parts(run, i) && matches!(run.steps[i], Status::Working(_))
            {
                let parts = self.parts(id, i);
                let done = parts.iter().filter(|r| r.finished.is_some()).count();
                format!("{done} of {} done", parts.len())
            } else {
                status_words(&run.steps[i], &step)
            };
            Field {
                label: clip(&step.id, 24),
                value: clip(&value, 120),
            }
        };
        let fields = match &run.part {
            Some(part) => vec![field(part.step)],
            None => (first..first + shown).map(field).collect(),
        };
        let title = match (&run.part, &self.workflow.description) {
            (Some(part), _) => format!(
                "{id}: {} for {}",
                self.workflow.steps[part.step].id, part.each
            ),
            (None, Some(about)) => format!("{id}: {about}"),
            (None, None) => format!("{id}: {}", self.workflow.name),
        };
        let about = match run.work() {
            Some((_, Work { by: Some(by), .. })) => Some(by.clone()),
            Some((_, Work { to, .. })) if to.len() == 1 => Some(to[0].clone()),
            _ => None,
        };
        fit(Card {
            id: clip(id, 64),
            title: clip(&title, 80),
            state,
            about: about.map(|a| clip(&a, 64)),
            fields,
            links: Vec::new(),
            actions: Vec::new(),
            steps: Vec::new(),
            note: run
                .finished
                .as_ref()
                .and_then(|(_, why)| why.as_deref())
                .map(|why| clip(why, 120)),
        })
    }
}

/// `card` as apps draw it ([`Card::validate`]): within [`zirc_proto::card::MAX_BYTES`] as
/// JSON, its fields' values shortened, and then its last fields dropped, until it is.
fn fit(mut card: Card) -> Card {
    if card.validate().is_ok() {
        return card;
    }
    for field in &mut card.fields {
        field.value = clip(&field.value, 40);
    }
    while card.validate().is_err() && card.fields.pop().is_some() {}
    card
}

/// A step done by the workflow itself.
fn done(step: &str, nick: &str) -> Change {
    Change::Done {
        step: step.to_owned(),
        by: nick.to_owned(),
        outputs: BTreeMap::new(),
    }
}

/// Who `step` of `run` goes to, unless a role's members do: an input's names or a nick.
fn who(run: &Run, step: &Step) -> Vec<String> {
    match step.who.as_deref().map(Who::parse) {
        // An input may name several, as CI names every landing it suspects.
        Some(Ok(Who::Input(input))) => run
            .inputs
            .get(&input)
            .map(|v| {
                v.split([' ', ','])
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        Some(Ok(Who::Named(name))) => vec![name],
        _ => Vec::new(),
    }
}

/// The step `change` is about, if it's about one.
fn step_of(change: &Change) -> Option<&str> {
    match change {
        Change::Assigned { step, .. }
        | Change::Claimed { step, .. }
        | Change::Done { step, .. }
        | Change::Answered { step, .. }
        | Change::Skipped { step }
        | Change::Stage { step, .. }
        | Change::Nudged { step }
        | Change::Escalated { step, .. } => Some(step),
        Change::Started { .. } | Change::Finished { .. } => None,
    }
}

/// The role whose members `step` goes to, if it's one's.
fn role_of(step: &Step) -> Option<String> {
    match step.who.as_deref().map(Who::parse) {
        Some(Ok(Who::Role(role))) => Some(role),
        _ => None,
    }
}

/// What the nick says giving run `id`'s `step` to `to`.
fn given(id: &str, step: &Step, to: &[String], says: &str) -> String {
    format!(
        "`{id}` · `{}` → {}: {says}{}",
        step.id,
        mentions(to),
        how_to_answer(id, step)
    )
}

/// What the nick says when run `id`'s `step` waits for `role`'s first member.
fn waits_for(id: &str, step: &Step, role: &str) -> String {
    format!(
        "`{id}` · `{}` waits for someone in {role}: an agent session that runs `zirc role \
         join {role}` gets it",
        step.id
    )
}

/// What a step's assignee is told to do once they've done it.
fn how_to_answer(id: &str, step: &Step) -> String {
    match step.kind {
        StepKind::Approve => format!(
            " Answer with 👍 or 👎, or `zirc step answer {id} {} yes`.",
            step.id
        ),
        _ if !step.stages.is_empty() => format!(
            " I'll follow along from what you say, push and land; `zirc step done {id} {}` \
             finishes it another way.",
            step.id
        ),
        _ => format!(" Take it with `zirc step claim {id} {}`.", step.id),
    }
}

/// Why `outputs` aren't what `step` may hand back, if they aren't: any of its outputs, none
/// it doesn't name.
fn check_outputs(step: &Step, outputs: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(extra) = outputs.keys().find(|o| !step.outputs.contains(o)) {
        let has = if step.outputs.is_empty() {
            "none".to_owned()
        } else {
            step.outputs.join(", ")
        };
        return Err(format!(
            "`{}` has no output {extra}: its outputs are {has}",
            step.id
        ));
    }
    let bytes = serde_json::to_string(outputs).map_or(usize::MAX, |j| j.len());
    if bytes > MAX_OUTPUTS {
        return Err(format!(
            "its outputs are {bytes} bytes, more than {MAX_OUTPUTS}"
        ));
    }
    Ok(())
}

/// Where a step given out stands, as a nudge or an escalation says it: `since working
/// (fix-web-unit)`, or `since it was given out`.
fn stage_words(step: &Step, work: &Work) -> String {
    match work.stage.and_then(|s| step.stages.get(s)) {
        Some(stage) => format!("since {}{}", stage.name, bracketed(work.detail.as_deref())),
        None if step.kind == StepKind::Wait => "since it started waiting".to_owned(),
        None => "since it was given out".to_owned(),
    }
}

/// How a step stands, in a few words, for its run's card.
fn status_words(status: &Status, step: &Step) -> String {
    match status {
        Status::Ahead => "not yet".to_owned(),
        Status::Working(work) => {
            let asked = if work.nudged {
                ", asked how it goes"
            } else {
                ""
            };
            if step.kind == StepKind::Wait {
                return format!("waiting{asked}");
            }
            if work.to.is_empty() && work.by.is_none() {
                let role = role_of(step).unwrap_or_default();
                return format!("waiting for someone in {role}");
            }
            let who = work.by.clone().unwrap_or_else(|| work.to.join(", "));
            let stage = work
                .stage
                .and_then(|s| step.stages.get(s))
                .map_or("given out", |s| s.name.as_str());
            let detail = bracketed(work.detail.as_deref());
            format!("{who}: {stage}{detail}{asked}")
        }
        Status::Held { .. } => "yes, counting soon".to_owned(),
        Status::Done(outputs) if outputs.is_empty() => "done".to_owned(),
        Status::Done(outputs) => format!(
            "done: {}",
            outputs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Status::Skipped => "skipped".to_owned(),
    }
}

/// `names` as mentions that wake whoever they name.
fn mentions(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("@{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` on one line, in at most `max` characters, cut with `…`.
fn clip(text: &str, max: usize) -> String {
    zirc_proto::clip(&text.replace('\n', " "), max)
}

/// ` (detail)`, or nothing without one.
fn bracketed(detail: Option<&str>) -> String {
    detail.map(|d| format!(" ({d})")).unwrap_or_default()
}

#[cfg(test)]
mod tests;
