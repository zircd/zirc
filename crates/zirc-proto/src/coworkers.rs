//! Coworkers and jobs: what a role ([`crate::roles`]) splits into (the user's choices, 26 and
//! 27 Nov 2026). A [`Coworker`] is a named agent with a desk: the name it's shown by, `John`,
//! and the nick made from it, `john` ([`crate::names::nick_for`]), its picture and colour, and
//! the repository it works in. A [`Job`] is what a
//! coworker does and what wakes it: a brief, triggers and the kinds of report it takes.
//! A coworker holds jobs by name, and may hold none: it's then woken by a direct message
//! alone, and still gets what every session does, such as being told of an overlap.
//!
//! A job has no repository: it's defined once in an organisation and taken on in any, by
//! any number of coworkers, each in the repository it works in. The product offers a coworker
//! [`JOBS_OFFERED`] job; the data allows more, so offering more is no migration.
//!
//! An organisation's coworkers and jobs together are its [`Staff`]. To a reader that knows
//! roles, a staff is the roles it amounts to ([`Staff::roles`]): a role for each coworker that
//! has a repository, named as the coworker and doing what its job says.
//!
//! A server keeps its organisation's staff itself: zirc.dev gives it whole ([`Given`], `PUT`
//! [`ACCOUNTS_PATH`]), and the server sets a coworker's repository when a session first sits
//! at its desk ([`Sitting`], [`DESK_PATH`]), saying the staff back with it filled in
//! ([`crate::roles::Standing::staff`]). A coworker's desk is the seat a role had, held as a
//! role's is ([`crate::roles::lease`] of its name), so a server and a machine of either age
//! agree who sits there.
//!
//! The jobs zirc.dev offers ready-made are [`JobKind`]s: what each says of itself, here so every app says it alike. What a ready-made job asks before it's added
//! is still zirc.dev's (`web/src/lib/role-templates.ts`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use std::collections::BTreeSet;

use crate::inbound::Tool;
use crate::role_report::Subject;
use crate::roles::{self, Address, Definition};
use crate::trigger::Trigger;

/// The most coworkers an organisation has: as many as it had roles.
pub const MAX_COWORKERS: usize = roles::MAX_ROLES;

/// The most jobs an organisation has.
pub const MAX_JOBS: usize = roles::MAX_ROLES;

/// How many jobs the product gives one coworker (the user's choice, 27 Nov 2026: "One only for
/// now"). [`Staff::check`] allows more.
pub const JOBS_OFFERED: usize = 1;

/// Where zirc.dev gives a server its organisation's staff, whole ([`Given`], `PUT`), on the
/// sidecar's accounts API, which only zirc.dev reaches. How it stands is read where roles'
/// is ([`roles::ACCOUNTS_PATH`], `GET`), which says the staff too.
pub const ACCOUNTS_PATH: &str = "/v1/staff";

/// Where a session's machine says it sits at a coworker's desk for the first time
/// ([`Sitting`], `POST`), under the server's content API: answered with the role the coworker
/// amounts to now ([`Staff::role_of`]), or refused in words when it works elsewhere
/// ([`works_elsewhere`]).
pub const DESK_PATH: &str = "/coworkers/{name}/desk";

/// The command that sits a session at a coworker's desk, before the coworker's name: `zirc
/// desk take`. `zirc role take` is the same command.
pub const DESK_COMMAND: &str = "zirc desk take";

/// What a person tells a session to sit it at a coworker's desk, `{name}` the name it's shown
/// by ([`Coworker::shown`]): what the agent guide teaches ([`tell_a_session`]). Provisional: the sentence isn't the
/// user's own yet.
pub const TELL_A_SESSION: &str = "You're {name}. Take your desk.";

/// What a person tells a session to sit it at the desk of the coworker shown as `name`:
/// `You're John. Take your desk.`
#[must_use]
pub fn tell_a_session(name: &str) -> String {
    TELL_A_SESSION.replace("{name}", name)
}

/// The command that sits a session at coworker `name`'s desk.
#[must_use]
pub fn desk_command(name: &str) -> String {
    format!("{DESK_COMMAND} {name}")
}

/// Why a session in another checkout can't sit at coworker `name`'s desk, which works in
/// `repository`.
#[must_use]
pub fn works_elsewhere(name: &str, repository: &str) -> String {
    format!("{name} works in {repository}: run this in a checkout of it")
}

/// What a session says to sit at a coworker's desk for the first time ([`DESK_PATH`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Sitting {
    /// The repository of the checkout the session is in, by name as zirc spells it: the
    /// coworker's from now on, if it had none.
    pub repository: String,
}

/// An organisation's staff as zirc.dev gives it to its server: whole each time, with each
/// job's address and a version that changes whenever any of it does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "StaffGiven")]
pub struct Given {
    /// The coworkers and jobs. A coworker zirc.dev gives no repository keeps the one the
    /// server set when a session first sat at its desk.
    pub staff: Staff,
    /// Each job's webhook address ([`roles::INBOUND_PATH`]), its [`Address::role`] the job's
    /// name: a tool posts to the job, whoever holds it, so it isn't set up again when the job
    /// moves. Never said back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<Address>,
    /// What it's at: anything that changes when it does, a new secret too.
    pub version: String,
}

impl Given {
    /// The rules it keeps: the staff's and each address's.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for whoever changed it.
    pub fn check(&self) -> Result<(), String> {
        self.staff.check()?;
        self.addresses.iter().try_for_each(|address| {
            address
                .check()
                .map_err(|why| format!("{}: {why}", address.role))
        })
    }
}

/// What a coworker with no job does, as the brief of the role it amounts to
/// ([`Staff::roles`]), which a role can't be without.
pub const NO_JOB_BRIEF: &str = "No job yet: does what it's asked in a direct message.";

/// A job zirc.dev offers ready-made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "JobKind")]
pub enum JobKind {
    /// Owns a broken default branch and the failures nobody else owns.
    Fixer,
    /// Owns errors and incidents in production.
    Oncall,
    /// Reads each new issue and hands it on.
    Triage,
    /// Reviews landings.
    Reviewer,
    /// Cuts and checks releases.
    Release,
}

impl JobKind {
    /// Every kind, in the order they're offered: the fixer first, as the usual start (the
    /// user's choice, 26 Nov 2026).
    pub const ALL: [Self; 5] = [
        Self::Fixer,
        Self::Oncall,
        Self::Triage,
        Self::Reviewer,
        Self::Release,
    ];

    /// How it's written, and the name a job of this kind is given: `fixer`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fixer => "fixer",
            Self::Oncall => "oncall",
            Self::Triage => "triage",
            Self::Reviewer => "reviewer",
            Self::Release => "release",
        }
    }

    /// The kind a job named `name` is, if it's named as one.
    #[must_use]
    pub fn of_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// What people call it: `On-call`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fixer => "Fixer",
            Self::Oncall => "On-call",
            Self::Triage => "Triage",
            Self::Reviewer => "Reviewer",
            Self::Release => "Release",
        }
    }

    /// What it does, in a line.
    #[must_use]
    pub const fn line(self) -> &'static str {
        match self {
            Self::Fixer => {
                "Owns a red master and failures nobody else owns. Never silences a test."
            }
            Self::Oncall => {
                "Owns errors and incidents in production: finds the cause, fixes it or says who should."
            }
            Self::Triage => "Reads each new issue, labels it, finds a likely owner and asks them.",
            Self::Reviewer => {
                "Reviews each landing and answers its author; best from another vendor's agent."
            }
            Self::Release => "Cuts and checks releases when you ask, and keeps their notes.",
        }
    }

    /// The kinds of report it takes as it's offered, before its questions change them.
    #[must_use]
    pub const fn reports(self) -> &'static [Subject] {
        match self {
            Self::Fixer => &[Subject::Flaky, Subject::Warning],
            Self::Oncall => &[Subject::Error, Subject::Incident],
            Self::Triage => &[Subject::Issue],
            Self::Reviewer | Self::Release => &[],
        }
    }

    /// The tools it's woken by, any one of which makes it useful: those that send what it
    /// takes ([`tools_of`]); none for a kind CI wakes.
    #[must_use]
    pub fn tools(self) -> Vec<Tool> {
        tools_of(self.reports())
    }

    /// What wakes it that no report is, said first.
    const fn wakes_first(self) -> Option<&'static str> {
        match self {
            Self::Fixer => Some("master breaking"),
            Self::Reviewer => Some("a landing"),
            Self::Release => Some("a message, or a schedule"),
            Self::Oncall | Self::Triage => None,
        }
    }

    /// What wakes it, in a line: its tools' own words where it has tools, else its reports'.
    #[must_use]
    pub fn wakes(self) -> String {
        let tools = self.tools();
        let reported: Vec<&str> = if tools.is_empty() {
            self.reports()
                .iter()
                .map(|subject| subject.label())
                .collect()
        } else {
            tools.iter().map(|tool| tool.wakes_on()).collect()
        };
        self.wakes_first()
            .into_iter()
            .chain(reported)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The tools that send reports of the kinds `reports` names, in the order zirc.dev shows tools.
#[must_use]
pub fn tools_of(reports: &[Subject]) -> Vec<Tool> {
    Tool::ALL
        .into_iter()
        .filter(|tool| {
            tool.subject()
                .is_some_and(|subject| reports.contains(&subject))
        })
        .collect()
}

/// What a coworker does, and what wakes whoever holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "Job")]
pub struct Job {
    /// Its name, unique among the organisation's jobs, which coworkers hold it by: `fixer`.
    pub name: String,
    /// What it does, given to whoever holds it.
    pub brief: String,
    /// What wakes its holder, besides a direct message. A trigger this reader can't read is
    /// left out ([`crate::lenient`]).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "crate::lenient::known_ones"
    )]
    #[ts(as = "Vec<Trigger>")]
    #[schemars(with = "Vec<Trigger>")]
    pub triggers: Vec<Trigger>,
    /// The kinds of report it takes. A kind this reader doesn't know is left out
    /// ([`crate::lenient`]).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "crate::lenient::known_ones"
    )]
    #[ts(as = "Vec<Subject>")]
    #[schemars(with = "Vec<Subject>")]
    pub reports: Vec<Subject>,
}

impl Job {
    /// The ready-made kind it's named as, if any.
    #[must_use]
    pub fn kind(&self) -> Option<JobKind> {
        JobKind::of_name(&self.name)
    }

    /// The tools that send what it takes: each whose reports are of a kind it takes. What an
    /// organisation is offered to connect for it (the user's choice, 27 Nov 2026: only what
    /// jobs need).
    #[must_use]
    pub fn tools(&self) -> Vec<Tool> {
        tools_of(&self.reports)
    }

    /// The rules a job keeps that need nothing but itself.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for whoever wrote it.
    pub fn check(&self) -> Result<(), String> {
        if let Some(why) = crate::names::job_problem(&self.name) {
            return Err(why);
        }
        if self.brief.trim().is_empty() || self.brief.chars().count() > roles::MAX_BRIEF {
            return Err(format!(
                "what a job does is some words, {} characters at most",
                roles::MAX_BRIEF
            ));
        }
        if self.triggers.len() > roles::MAX_TRIGGERS {
            return Err(format!(
                "a job wakes on at most {} things",
                roles::MAX_TRIGGERS
            ));
        }
        self.triggers
            .iter()
            .try_for_each(|trigger| trigger.check().map(drop))
    }
}

/// What a coworker's badge says it is, beside its office's name (the user's choice, 27 Nov
/// 2026: a coworker's card is its office badge, clipped on).
pub const STAFF: &str = "Staff";

/// What the badge of a session at a hot desk says it is, beside its office's name.
pub const HOT_DESK: &str = "Hot desk";

/// What a badge says under the job, `{nick}` the coworker's and `{hired}` the day it was
/// hired, as each app words a day (`27 Nov`, `27 Nov 2025` before this year): `@john · hired
/// 27 Nov`.
pub const BADGE_HANDLE: &str = "@{nick} · hired {hired}";

/// What a badge says under coworker `nick`'s job: [`BADGE_HANDLE`] with the day `hired`, as
/// the app words it, or `@john` alone while it's unknown.
#[must_use]
pub fn badge_handle(nick: &str, hired: Option<&str>) -> String {
    match hired {
        Some(hired) => BADGE_HANDLE
            .replace("{nick}", nick)
            .replace("{hired}", hired),
        None => format!("@{nick}"),
    }
}

/// A named agent with a desk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "Coworker")]
pub struct Coworker {
    /// Its nick on the server, `john`: unique in the organisation, made from the name it's
    /// shown by when it was hired ([`Staff::nick_for`]), and kept when that changes, as what
    /// it said is said by it. What every reader keys it by.
    pub name: String,
    /// The name it's shown by, as whoever hired it typed it: `John`. None for one hired
    /// before coworkers had one, which is shown by its nick ([`Coworker::shown`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shown_name: Option<String>,
    /// The jobs it holds, by name: none, or as the product offers, [`JOBS_OFFERED`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<String>,
    /// The repository it works in, by name as zirc spells it: set when a session first sits
    /// at its desk, from the checkout that session is in. None until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repository: Option<String>,
    /// Its picture, one of [`crate::characters::CHARACTERS`] (or, picked before them, one of
    /// [`crate::AGENT_AVATARS`]' bots); without one, the bot its name tells.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub picture: Option<String>,
    /// Its colour, a hue in degrees under 360, one of [`roles::HUES`] as zirc.dev offers
    /// them; without one, its name's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hue: Option<u16>,
    /// The account of the member who added it, who may change or remove it as an admin may
    /// (the user's choice, 27 Nov 2026). None for one that was a role before members could.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub added_by: Option<String>,
    /// When it was hired, unix seconds, as zirc.dev keeps it: what its badge's `hired 27 Nov`
    /// says. None for one zirc.dev hasn't said it of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hired: Option<u64>,
}

impl Coworker {
    /// The name it's shown by: `John`, or its nick for one with none.
    #[must_use]
    pub fn shown(&self) -> &str {
        self.shown_name.as_deref().unwrap_or(&self.name)
    }

    /// The rules a coworker keeps that need nothing but itself.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for whoever added it.
    pub fn check(&self) -> Result<(), String> {
        if let Some(why) = crate::names::coworker_problem(&self.name) {
            return Err(why);
        }
        if let Some(why) = self
            .shown_name
            .as_deref()
            .and_then(crate::names::shown_name_problem)
        {
            return Err(why);
        }
        if self
            .repository
            .as_deref()
            .is_some_and(|repository| repository.trim().is_empty())
        {
            return Err("a coworker's repository has a name".to_owned());
        }
        if let Some(picture) = &self.picture
            && !crate::characters::is_picture(picture)
        {
            return Err(format!("{picture} isn't a picture a coworker can have"));
        }
        if self.hue.is_some_and(|hue| hue >= 360) {
            return Err("a coworker's colour is a hue under 360 degrees".to_owned());
        }
        Ok(())
    }
}

/// An organisation's coworkers and the jobs they hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "Staff")]
pub struct Staff {
    /// Every coworker, in the order they were added.
    #[serde(default)]
    pub coworkers: Vec<Coworker>,
    /// Every job, held or not.
    #[serde(default)]
    pub jobs: Vec<Job>,
}

impl Staff {
    /// The job named `name`.
    #[must_use]
    pub fn job(&self, name: &str) -> Option<&Job> {
        self.jobs.iter().find(|job| job.name == name)
    }

    /// The coworkers that hold job `name`.
    pub fn holders<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Coworker> {
        self.coworkers
            .iter()
            .filter(move |coworker| coworker.jobs.iter().any(|job| job == name))
    }

    /// The coworker that takes what comes for job `name` about `repository`: the one that
    /// holds it there, the first added if several do (the user's choice, 27 Nov 2026: one
    /// holder, by repository). Who has been idle longest among several is the server's to
    /// say, as it knows.
    #[must_use]
    pub fn takes<'a>(&'a self, name: &'a str, repository: &str) -> Option<&'a Coworker> {
        self.holders(name)
            .find(|coworker| coworker.repository.as_deref() == Some(repository))
    }

    /// The coworkers that may take what comes for job `name`: those that hold it, in
    /// `repository` when what came is about one, and every holder when it's about none, as a
    /// time coming round is. Which of several takes it is the server's to say: the one
    /// longest idle (the user's choice, 27 Nov 2026).
    pub fn takers<'a>(
        &'a self,
        name: &'a str,
        repository: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Coworker> {
        self.holders(name).filter(move |coworker| {
            repository.is_none_or(|repository| coworker.repository.as_deref() == Some(repository))
        })
    }

    /// The coworker named `name`.
    #[must_use]
    pub fn coworker(&self, name: &str) -> Option<&Coworker> {
        self.coworkers.iter().find(|coworker| coworker.name == name)
    }

    /// The coworker a person means by `said`: the one whose nick it is, else the one shown by
    /// it, whatever its case (`John` and `john`, as a person says the sentence
    /// [`TELL_A_SESSION`] and a session runs [`DESK_COMMAND`] with it).
    #[must_use]
    pub fn called(&self, said: &str) -> Option<&Coworker> {
        let said = said.trim();
        self.coworker(said).or_else(|| {
            self.coworkers
                .iter()
                .find(|coworker| same_name(coworker.shown(), said))
        })
    }

    /// The nick a coworker shown as `shown` is hired with ([`crate::names::nick_for`]): free
    /// among its coworkers and those `taken` says (the office's members and machines).
    ///
    /// # Errors
    ///
    /// Why it can't be hired so: a name it can't have, or one another coworker is shown by.
    pub fn nick_for(&self, shown: &str, taken: impl Fn(&str) -> bool) -> Result<String, String> {
        if let Some(other) = self
            .coworkers
            .iter()
            .find(|coworker| same_name(coworker.shown(), shown))
        {
            return Err(format!(
                "There's a coworker called {} already: choose another name.",
                other.shown()
            ));
        }
        crate::names::nick_for(shown, |nick| taken(nick) || self.coworker(nick).is_some())
    }

    /// The role `coworker` amounts to, for a reader that knows roles: named as it is, doing
    /// what its first job says, or [`NO_JOB_BRIEF`] with no job, in its repository, which is
    /// empty until a session has sat at its desk.
    #[must_use]
    pub fn role_of(&self, coworker: &Coworker) -> Definition {
        let job = coworker.jobs.first().and_then(|name| self.job(name));
        Definition {
            name: coworker.name.clone(),
            shown_name: coworker.shown_name.clone(),
            repository: coworker.repository.clone().unwrap_or_default(),
            brief: job.map_or_else(|| NO_JOB_BRIEF.to_owned(), |job| job.brief.clone()),
            triggers: job.map(|job| job.triggers.clone()).unwrap_or_default(),
            seats: roles::SEATS,
            reports: job.map(|job| job.reports.clone()).unwrap_or_default(),
            picture: coworker.picture.clone(),
            hue: coworker.hue,
        }
    }

    /// Every coworker as the role it amounts to ([`Staff::role_of`]), one nobody has sat at
    /// too: what a server says to readers that know roles.
    #[must_use]
    pub fn desks(&self) -> Vec<Definition> {
        self.coworkers
            .iter()
            .map(|coworker| self.role_of(coworker))
            .collect()
    }

    /// The tools its jobs need, each once, in the order zirc.dev shows tools: those a held
    /// job takes reports from. CI is every organisation's and isn't one of them.
    #[must_use]
    pub fn tools(&self) -> Vec<Tool> {
        let needed: BTreeSet<Tool> = self
            .jobs
            .iter()
            .filter(|job| self.holders(&job.name).next().is_some())
            .flat_map(Job::tools)
            .collect();
        Tool::ALL
            .into_iter()
            .filter(|tool| needed.contains(tool))
            .collect()
    }

    /// The rules a staff keeps: each coworker's and job's own, no name twice, and no coworker
    /// holding a job there isn't.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for whoever changed it.
    pub fn check(&self) -> Result<(), String> {
        if self.coworkers.len() > MAX_COWORKERS {
            return Err(format!("an office has at most {MAX_COWORKERS} coworkers"));
        }
        if self.jobs.len() > MAX_JOBS {
            return Err(format!("an office has at most {MAX_JOBS} jobs"));
        }
        let mut jobs = BTreeSet::new();
        for job in &self.jobs {
            job.check()?;
            if !jobs.insert(job.name.as_str()) {
                return Err(format!("there are two jobs named {}", job.name));
            }
        }
        let mut names = BTreeSet::new();
        let mut shown = BTreeSet::new();
        for coworker in &self.coworkers {
            coworker.check()?;
            if !names.insert(coworker.name.as_str()) {
                return Err(format!("there are two coworkers named {}", coworker.name));
            }
            if !shown.insert(coworker.shown().trim().to_lowercase()) {
                return Err(format!(
                    "there are two coworkers called {}",
                    coworker.shown()
                ));
            }
            if let Some(missing) = coworker
                .jobs
                .iter()
                .find(|job| !jobs.contains(job.as_str()))
            {
                return Err(format!(
                    "{} holds {missing}, which isn't one of the office's jobs",
                    coworker.name
                ));
            }
        }
        Ok(())
    }

    /// The roles it amounts to, for a server that keeps roles: one for each coworker that has
    /// a repository, named as the coworker, doing what its first job says, or
    /// [`NO_JOB_BRIEF`] with no job. A coworker no session has sat at yet has no repository,
    /// and so no role.
    #[must_use]
    pub fn roles(&self) -> Vec<Definition> {
        self.coworkers
            .iter()
            .filter(|coworker| coworker.repository.is_some())
            .map(|coworker| self.role_of(coworker))
            .collect()
    }
}

/// Whether `a` and `b` are one name to a person: alike but for case and the spaces around.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The TypeScript the web app reads coworkers and jobs as.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        JobKind::decl(config),
        Job::decl(config),
        Coworker::decl(config),
        Staff::decl(config),
        Given::decl(config),
        format!("const STAFF_ACCOUNTS_PATH = {ACCOUNTS_PATH:?};"),
        format!("const JOBS_OFFERED = {JOBS_OFFERED};"),
        format!("const MAX_COWORKERS = {MAX_COWORKERS};"),
        format!("const DESK_COMMAND = {DESK_COMMAND:?};"),
        format!("const TELL_A_SESSION = {TELL_A_SESSION:?};"),
        format!("const BADGE_STAFF = {STAFF:?};"),
        format!("const BADGE_HOT_DESK = {HOT_DESK:?};"),
        format!("const BADGE_HANDLE = {BADGE_HANDLE:?};"),
        format!(
            "const JOB_KINDS: readonly {{ id: JobKind; label: string; line: string; wakes: string; tools: readonly InboundTool[] }}[] = [{}];",
            JobKind::ALL
                .map(|kind| format!(
                    "{{ id: {:?}, label: {:?}, line: {:?}, wakes: {:?}, tools: [{}] }}",
                    kind.as_str(),
                    kind.label(),
                    kind.line(),
                    kind.wakes(),
                    kind.tools()
                        .iter()
                        .map(|tool| format!("{:?}", tool.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .join(", ")
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coworker(name: &str, job: Option<&str>, repository: Option<&str>) -> Coworker {
        Coworker {
            name: name.to_owned(),
            shown_name: None,
            jobs: job.into_iter().map(str::to_owned).collect(),
            repository: repository.map(str::to_owned),
            picture: None,
            hue: None,
            added_by: None,
            hired: None,
        }
    }

    fn job(name: &str, reports: &[Subject]) -> Job {
        Job {
            name: name.to_owned(),
            brief: format!("Does what {name} does."),
            triggers: Vec::new(),
            reports: reports.to_vec(),
        }
    }

    #[test]
    fn two_coworkers_hold_one_job_each_in_its_own_repository() {
        let staff = Staff {
            coworkers: vec![
                coworker("ada", Some("fixer"), Some("web")),
                coworker("grace", Some("fixer"), Some("api")),
                coworker("linus", None, Some("web")),
                coworker("hedy", Some("fixer"), None),
            ],
            jobs: vec![job("fixer", &[Subject::Flaky])],
        };
        staff.check().unwrap();
        assert_eq!(staff.takes("fixer", "api").unwrap().name, "grace");
        assert_eq!(staff.takes("fixer", "web").unwrap().name, "ada");
        assert!(staff.takes("fixer", "docs").is_none());
        // Each with a repository is a role of its own name; the one with no job says so, and
        // the one nobody sat at yet isn't one.
        let roles = staff.roles();
        let names: Vec<&str> = roles.iter().map(|role| role.name.as_str()).collect();
        assert_eq!(names, ["ada", "grace", "linus"]);
        assert_eq!(roles[0].brief, roles[1].brief);
        assert_eq!(roles[1].repository, "api");
        assert_eq!(roles[2].brief, NO_JOB_BRIEF);
        for role in &roles {
            role.check().unwrap();
        }
    }

    #[test]
    fn what_comes_for_a_job_is_for_its_holders_in_that_repository_or_all_of_them() {
        let staff = Staff {
            coworkers: vec![
                coworker("ada", Some("fixer"), Some("web")),
                coworker("grace", Some("fixer"), Some("api")),
                coworker("hedy", Some("fixer"), None),
                coworker("linus", None, Some("web")),
            ],
            jobs: vec![job("fixer", &[])],
        };
        let names = |repository| -> Vec<&str> {
            staff
                .takers("fixer", repository)
                .map(|coworker| coworker.name.as_str())
                .collect()
        };
        assert_eq!(names(Some("web")), ["ada"]);
        assert_eq!(names(Some("docs")), [] as [&str; 0]);
        assert_eq!(names(None), ["ada", "grace", "hedy"]);
        // One nobody has sat at is a desk with no repository yet, and no role.
        let desks = staff.desks();
        assert_eq!(desks.len(), 4);
        assert_eq!(desks[2].repository, "");
        assert_eq!(staff.roles().len(), 3);
        assert_eq!(tell_a_session("John"), "You're John. Take your desk.");
        assert_eq!(desk_command("ada"), "zirc desk take ada");
    }

    #[test]
    fn a_staff_needs_only_the_tools_its_held_jobs_take_reports_from() {
        let mut staff = Staff {
            coworkers: vec![coworker("ada", None, None)],
            jobs: vec![job("oncall", &[Subject::Error]), job("fixer", &[])],
        };
        // Nobody holds a job: no tool, though one is defined.
        assert_eq!(staff.tools(), []);
        staff.coworkers[0].jobs = vec!["oncall".to_owned()];
        assert_eq!(staff.tools(), [Tool::Sentry]);
        staff.jobs[0].reports.push(Subject::Incident);
        assert_eq!(staff.tools(), [Tool::Sentry, Tool::Pagerduty]);
    }

    #[test]
    fn a_staff_refuses_a_name_twice_and_a_job_that_isnt_there() {
        let twice = Staff {
            coworkers: vec![coworker("ada", None, None), coworker("ada", None, None)],
            jobs: Vec::new(),
        };
        assert_eq!(
            twice.check().unwrap_err(),
            "there are two coworkers named ada"
        );
        let missing = Staff {
            coworkers: vec![coworker("ada", Some("fixer"), None)],
            jobs: Vec::new(),
        };
        assert_eq!(
            missing.check().unwrap_err(),
            "ada holds fixer, which isn't one of the office's jobs"
        );
        let unnamed = Staff {
            coworkers: vec![coworker("Ada Lovelace", None, None)],
            jobs: Vec::new(),
        };
        assert!(unnamed.check().unwrap_err().starts_with("Name a coworker"));
        let pictured = Coworker {
            picture: Some("unicorn".to_owned()),
            ..coworker("ada", None, None)
        };
        assert_eq!(
            pictured.check().unwrap_err(),
            "unicorn isn't a picture a coworker can have"
        );
    }

    #[test]
    fn every_ready_made_job_is_named_as_a_job_may_be() {
        for kind in JobKind::ALL {
            assert_eq!(JobKind::of_name(kind.as_str()), Some(kind));
            assert!(
                crate::names::job_problem(kind.as_str()).is_none(),
                "{kind:?}"
            );
            assert!(!kind.wakes().is_empty() && !kind.line().is_empty());
        }
        assert_eq!(JobKind::ALL[0], JobKind::Fixer);
        assert_eq!(
            JobKind::Oncall.wakes(),
            "a Sentry error, a PagerDuty incident"
        );
        assert_eq!(
            JobKind::Fixer.wakes(),
            "master breaking, a flaky test, a new warning"
        );
        assert_eq!(JobKind::Triage.tools(), [Tool::Github, Tool::Linear]);
    }

    #[test]
    fn a_coworker_is_shown_by_its_name_and_found_by_it_or_its_nick() {
        let john = Coworker {
            shown_name: Some("John".to_owned()),
            ..coworker("john", None, None)
        };
        let mut staff = Staff {
            coworkers: vec![john, coworker("ada", None, None)],
            jobs: Vec::new(),
        };
        staff.check().unwrap();
        assert_eq!(staff.coworkers[0].shown(), "John");
        assert_eq!(staff.coworkers[1].shown(), "ada");
        for said in ["john", "John", " JOHN "] {
            assert_eq!(staff.called(said).unwrap().name, "john", "{said}");
        }
        assert!(staff.called("grace").is_none());
        // A second John is refused; another whose nick would be john gets the next.
        assert_eq!(
            staff.nick_for("john", |_| false).unwrap_err(),
            "There's a coworker called John already: choose another name."
        );
        assert_eq!(staff.nick_for("Jöhn", |_| false).as_deref(), Ok("john2"));
        assert_eq!(
            staff
                .nick_for("Mary Jane", |nick| nick == "mary_jane")
                .as_deref(),
            Ok("mary_jane2")
        );
        // Shown names are one per office whatever their case, as nicks are.
        staff.coworkers[1].shown_name = Some("JOHN".to_owned());
        assert_eq!(
            staff.check().unwrap_err(),
            "there are two coworkers called JOHN"
        );
        staff.coworkers[1].shown_name = Some("李明".to_owned());
        assert!(staff.check().unwrap_err().contains("letters from a to z"));
        // The role a coworker amounts to is shown by its name too.
        staff.coworkers[1].shown_name = None;
        assert_eq!(
            staff.role_of(&staff.coworkers[0]).shown_name.as_deref(),
            Some("John")
        );
    }

    #[test]
    fn a_badge_says_its_coworkers_handle_and_the_day_it_was_hired() {
        assert_eq!(badge_handle("john", Some("27 Nov")), "@john · hired 27 Nov");
        assert_eq!(badge_handle("john", None), "@john");
        let hired: Coworker =
            serde_json::from_str(r#"{"name": "john", "hired": 1795800000}"#).unwrap();
        assert_eq!(hired.hired, Some(1_795_800_000));
        let before: Coworker = serde_json::from_str(r#"{"name": "john"}"#).unwrap();
        assert_eq!(before.hired, None);
        assert!(!serde_json::to_string(&before).unwrap().contains("hired"));
    }
}
