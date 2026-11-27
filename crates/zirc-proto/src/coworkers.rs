//! Coworkers and jobs: what a role ([`crate::roles`]) splits into (the user's choices, 26 and
//! 27 Nov 2026). A [`Coworker`] is a named agent with a desk: its name, which is its nick on
//! the server, its picture and colour, and the repository it works in. A [`Job`] is what a
//! coworker does and what wakes it: a brief, triggers and the kinds of report it takes.
//! A coworker holds jobs by name, and may hold none: it's then woken by a direct message
//! alone, and still gets what every session does, such as being told of an overlap.
//!
//! A job has no repository: it's defined once in an organisation and taken on in any, by
//! any number of coworkers, each in the repository it works in. The product offers a coworker
//! [`JOBS_OFFERED`] job; the data allows more, so offering more is no migration.
//!
//! An organisation's coworkers and jobs together are its [`Staff`]. Until servers keep
//! coworkers themselves, a staff is given to one as the roles it amounts to
//! ([`Staff::roles`]): a role for each coworker that has a repository, named as the coworker
//! and doing what its job says. Today's roles read as a staff the other way
//! ([`Staff::of_roles`]), each a coworker and a job of the role's name, so nothing is lost
//! either way.
//!
//! The jobs zirc.dev offers ready-made are [`JobKind`]s: how each is pictured and what it says
//! of itself, here so every app says it alike. What a ready-made job asks before it's added
//! is still zirc.dev's (`web/src/lib/role-templates.ts`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use std::collections::BTreeSet;

use crate::inbound::Tool;
use crate::role_report::Subject;
use crate::roles::{self, Definition};
use crate::trigger::Trigger;

/// The most coworkers an organisation has: as many as it had roles.
pub const MAX_COWORKERS: usize = roles::MAX_ROLES;

/// The most jobs an organisation has.
pub const MAX_JOBS: usize = roles::MAX_ROLES;

/// How many jobs the product gives one coworker (the user's choice, 27 Nov 2026: "One only for
/// now"). [`Staff::check`] allows more.
pub const JOBS_OFFERED: usize = 1;

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

    /// The picture a coworker doing it starts with, one of [`crate::AGENT_AVATARS`].
    #[must_use]
    pub const fn picture(self) -> &'static str {
        match self {
            Self::Fixer => "fixer",
            Self::Oncall => "ops",
            Self::Triage => "coordinator",
            Self::Reviewer => "reviewer",
            Self::Release => "release",
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

    /// The tools it's woken by, any one of which makes it useful; none for a kind CI wakes.
    #[must_use]
    pub const fn tools(self) -> &'static [Tool] {
        match self {
            Self::Oncall => &[Tool::Sentry, Tool::Pagerduty],
            Self::Triage => &[Tool::Github, Tool::Linear],
            Self::Fixer | Self::Reviewer | Self::Release => &[],
        }
    }

    /// What wakes it, in a line: its tools' own words where it has tools.
    #[must_use]
    pub fn wakes(self) -> String {
        match self {
            Self::Fixer => "master breaking, a flaky test, a new warning".to_owned(),
            Self::Reviewer => "a landing".to_owned(),
            Self::Release => "a message, or a schedule".to_owned(),
            Self::Oncall | Self::Triage => self
                .tools()
                .iter()
                .map(|tool| tool.wakes_on())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// What a coworker does, and what wakes whoever holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    pub triggers: Vec<Trigger>,
    /// The kinds of report it takes. A kind this reader doesn't know is left out
    /// ([`crate::lenient`]).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "crate::lenient::known_ones"
    )]
    #[ts(as = "Vec<Subject>")]
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
        Tool::ALL
            .into_iter()
            .filter(|tool| {
                tool.subject()
                    .is_some_and(|subject| self.reports.contains(&subject))
            })
            .collect()
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

/// A named agent with a desk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "Coworker")]
pub struct Coworker {
    /// Its name, which is its nick on the server: unique in the organisation, as whoever
    /// added it typed it.
    pub name: String,
    /// The jobs it holds, by name: none, or as the product offers, [`JOBS_OFFERED`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<String>,
    /// The repository it works in, by name as zirc spells it: set when a session first sits
    /// at its desk, from the checkout that session is in. None until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repository: Option<String>,
    /// Its picture, one of [`crate::AGENT_AVATARS`]; without one, the one its name tells.
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
}

impl Coworker {
    /// The rules a coworker keeps that need nothing but itself.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for whoever added it.
    pub fn check(&self) -> Result<(), String> {
        if let Some(why) = crate::names::coworker_problem(&self.name) {
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
            && !crate::AGENT_AVATARS.iter().any(|(id, _)| id == picture)
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
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
            return Err(format!("a team has at most {MAX_COWORKERS} coworkers"));
        }
        if self.jobs.len() > MAX_JOBS {
            return Err(format!("a team has at most {MAX_JOBS} jobs"));
        }
        let mut jobs = BTreeSet::new();
        for job in &self.jobs {
            job.check()?;
            if !jobs.insert(job.name.as_str()) {
                return Err(format!("there are two jobs named {}", job.name));
            }
        }
        let mut names = BTreeSet::new();
        for coworker in &self.coworkers {
            coworker.check()?;
            if !names.insert(coworker.name.as_str()) {
                return Err(format!("there are two coworkers named {}", coworker.name));
            }
            if let Some(missing) = coworker
                .jobs
                .iter()
                .find(|job| !jobs.contains(job.as_str()))
            {
                return Err(format!(
                    "{} holds {missing}, which isn't one of the team's jobs",
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
            .filter_map(|coworker| {
                let repository = coworker.repository.clone()?;
                let job = coworker.jobs.first().and_then(|name| self.job(name));
                Some(Definition {
                    name: coworker.name.clone(),
                    repository,
                    brief: job.map_or_else(|| NO_JOB_BRIEF.to_owned(), |job| job.brief.clone()),
                    triggers: job.map(|job| job.triggers.clone()).unwrap_or_default(),
                    seats: roles::SEATS,
                    reports: job.map(|job| job.reports.clone()).unwrap_or_default(),
                    picture: coworker.picture.clone(),
                    hue: coworker.hue,
                })
            })
            .collect()
    }

    /// The staff `roles` amount to: each a coworker of its name, in its repository, holding
    /// a job of its name that does what the role did.
    #[must_use]
    pub fn of_roles(roles: &[Definition]) -> Self {
        Self {
            coworkers: roles
                .iter()
                .map(|role| Coworker {
                    name: role.name.clone(),
                    jobs: vec![role.name.clone()],
                    repository: Some(role.repository.clone()),
                    picture: role.picture.clone(),
                    hue: role.hue,
                    added_by: None,
                })
                .collect(),
            jobs: roles
                .iter()
                .map(|role| Job {
                    name: role.name.clone(),
                    brief: role.brief.clone(),
                    triggers: role.triggers.clone(),
                    reports: role.reports.clone(),
                })
                .collect(),
        }
    }
}

/// The TypeScript the web app reads coworkers and jobs as.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        JobKind::decl(config),
        Job::decl(config),
        Coworker::decl(config),
        Staff::decl(config),
        format!("const JOBS_OFFERED = {JOBS_OFFERED};"),
        format!("const MAX_COWORKERS = {MAX_COWORKERS};"),
        format!(
            "const JOB_KINDS: readonly {{ id: JobKind; label: string; picture: string; line: string; wakes: string; tools: readonly InboundTool[] }}[] = [{}];",
            JobKind::ALL
                .map(|kind| format!(
                    "{{ id: {:?}, label: {:?}, picture: {:?}, line: {:?}, wakes: {:?}, tools: [{}] }}",
                    kind.as_str(),
                    kind.label(),
                    kind.picture(),
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

    fn role(name: &str, repository: &str) -> Definition {
        Definition {
            name: name.to_owned(),
            repository: repository.to_owned(),
            brief: format!("Does what {name} does."),
            triggers: Vec::new(),
            seats: roles::SEATS,
            reports: Vec::new(),
            picture: None,
            hue: None,
        }
    }

    fn coworker(name: &str, job: Option<&str>, repository: Option<&str>) -> Coworker {
        Coworker {
            name: name.to_owned(),
            jobs: job.into_iter().map(str::to_owned).collect(),
            repository: repository.map(str::to_owned),
            picture: None,
            hue: None,
            added_by: None,
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
    fn todays_roles_are_a_staff_and_back_with_nothing_lost() {
        let roles = vec![
            Definition {
                reports: vec![Subject::Flaky],
                picture: Some("fixer".to_owned()),
                hue: Some(210),
                ..role("fixer", "monorepo")
            },
            role("oncall", "monorepo"),
        ];
        let staff = Staff::of_roles(&roles);
        staff.check().unwrap();
        assert_eq!(staff.roles(), roles);
        for role in &roles {
            role.check().unwrap();
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
            "ada holds fixer, which isn't one of the team's jobs"
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
    fn every_ready_made_job_is_pictured_and_named_as_a_job_may_be() {
        for kind in JobKind::ALL {
            assert!(
                crate::AGENT_AVATARS
                    .iter()
                    .any(|(id, _)| *id == kind.picture()),
                "{kind:?}"
            );
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
    }
}
