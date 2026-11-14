//! What `zirc wait` waits for: a landing or a deploy, named by what CI's events carry
//! ([`zirc_proto::ci`]), or how something the team shares stands ([`zirc_proto::state`]), and
//! how each event answers it. A session that wanted to hear
//! of a deploy once polled a channel's history in a shell loop, grepping CI's lines: a wait
//! is a condition with its failure, judged the same over history and over what comes next.

use zirc_proto::Event;
use zirc_proto::ci::{DEPLOY_KIND, Data, LANDING_KIND};
use zirc_proto::state::{Level, State};

use crate::sessions::{Landing, landing_event};

/// The words CI's line starts with, after whom it's for, once a deploy to production is done
/// (`ops/lib.sh`, `say_promoted`).
const DEPLOYED: &str = "deployed to production: ";

/// What a wait is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// A landing landed: of a branch, a build by its number, or a session's.
    Landed {
        /// The branch, the build or the session.
        on: String,
    },
    /// A landing was deployed to production.
    Deployed {
        /// The landing: its branch, its build by its number, or the commit it landed as,
        /// or the start of one. Production gets the default branch's head as it is by
        /// then, often a later commit, so a commit is followed through its build.
        on: String,
        /// The app that must be among what was deployed, when only one matters.
        app: Option<String>,
        /// The builds learnt to be the landing's, from the line saying what it landed as:
        /// a deploy's failure names its build, never the commit.
        builds: Vec<String>,
        /// The apps its deploys' lines said were deployed so far: with no `app`, the wait
        /// ends once every app the landing deploys ([`Data::deploys`]) is among them.
        seen: Vec<String>,
    },
    /// A state, such as master, stands as wanted.
    Stands {
        /// The channel of the repository it's in; any repository's when unknown.
        repo: Option<String>,
        /// What it's the state of, such as `master`.
        name: String,
        /// How it must stand.
        want: Want,
    },
}

/// How a state must stand to end a wait for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// At this level.
    At(Level),
    /// At any level but this, the one it stood at as the wait started: `None` for a state
    /// never said, which is fine.
    Not(Option<Level>),
}

/// How an event answers a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// What was waited for happened.
    Met,
    /// It failed, and won't happen without someone doing something.
    Failed,
}

impl Condition {
    /// A wait for the landing `on` names to land.
    #[must_use]
    pub fn landed(on: &str) -> Self {
        Self::Landed { on: named(on) }
    }

    /// A wait for the landing `on` names to be deployed to production, `app` among it.
    #[must_use]
    pub fn deployed(on: &str, app: Option<&str>) -> Self {
        Self::Deployed {
            on: named(on),
            app: app.map(str::to_owned),
            builds: Vec::new(),
            seen: Vec::new(),
        }
    }

    /// A wait for the state `name` of the repository in `repo` to stand as `want` says.
    #[must_use]
    pub fn stands(repo: Option<&str>, name: &str, want: Want) -> Self {
        Self::Stands {
            repo: repo.map(str::to_owned),
            name: name.to_owned(),
            want,
        }
    }

    /// Whether a state at `level`, or never said, is what the wait is for. Never for a
    /// landing or a deploy, which are events: nothing stands.
    #[must_use]
    pub fn met_at(&self, level: Option<Level>) -> bool {
        match self {
            Self::Stands {
                want: Want::At(want),
                ..
            } => level.unwrap_or(Level::Ok) == *want,
            Self::Stands {
                want: Want::Not(from),
                ..
            } => level != *from,
            _ => false,
        }
    }

    /// The state `event` says, if it's the one a wait for a state is on.
    #[must_use]
    pub fn state_in(&self, event: &Event) -> Option<State> {
        let Self::Stands { repo, name, .. } = self else {
            return None;
        };
        State::of(event).filter(|state| {
            &state.name == name && repo.as_ref().is_none_or(|repo| &state.repo == repo)
        })
    }

    /// It in a few words, as a line says what is waited for.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Stands {
                name,
                want: Want::At(level),
                ..
            } => {
                format!("{name} to be {}", level.word())
            }
            Self::Stands {
                name,
                want: Want::Not(from),
                ..
            } => format!("{name} to change from {}", from.unwrap_or(Level::Ok).word()),
            Self::Landed { on } => format!("{on} to land"),
            Self::Deployed { on, app: None, .. } => format!("{on} to be deployed to production"),
            Self::Deployed {
                on, app: Some(app), ..
            } => format!("{app} from {on} to be deployed to production"),
        }
    }

    /// How `event` answers the wait, if it does. Events are judged oldest first: a deploy's
    /// wait learns its landing's build from the line saying it landed.
    pub fn judge(&mut self, event: &Event) -> Option<Outcome> {
        if let Some(state) = self.state_in(event) {
            return self.met_at(Some(state.level)).then_some(Outcome::Met);
        }
        let Event::Custom {
            kind, data, text, ..
        } = event
        else {
            return None;
        };
        let said = Data::of(data);
        match self {
            Self::Landed { on } if kind == LANDING_KIND => {
                let (session, landing) = landing_event(text, data);
                let landing = landing?;
                let (branch, build) = match &landing {
                    Landing::Queued { .. } => return None,
                    Landing::Landed { branch, build, .. } => (Some(branch.clone()), *build),
                    Landing::Failed { branch, build, .. } => (branch.clone(), *build),
                    Landing::Conflicts { branch } => (Some(branch.clone()), None),
                };
                let build = build.map(|n| n.to_string()).or(said.build);
                let names = [branch, said.branch, build, session, said.session];
                if !names.iter().flatten().any(|name| name == on) {
                    return None;
                }
                Some(match landing {
                    Landing::Landed { .. } => Outcome::Met,
                    _ => Outcome::Failed,
                })
            }
            Self::Deployed { on, builds, .. } if kind == LANDING_KIND => {
                // Not an answer: which build the commit waited on landed in.
                if let (Some(commit), Some(build)) = (&said.commit, said.build)
                    && commit.starts_with(on.as_str())
                    && !builds.contains(&build)
                {
                    builds.push(build);
                }
                None
            }
            Self::Deployed {
                on,
                app,
                builds,
                seen,
            } if kind == DEPLOY_KIND => {
                let ours = said.branch.as_deref() == Some(on.as_str())
                    || said
                        .build
                        .as_ref()
                        .is_some_and(|build| build == on || builds.contains(build))
                    || said
                        .commit
                        .as_ref()
                        .is_some_and(|commit| commit.starts_with(on.as_str()));
                if !ours {
                    return None;
                }
                if said.state.as_deref() == Some("failed") {
                    return Some(Outcome::Failed);
                }
                // All of it: a line names several apps on lines of their own.
                let (_, what) = text.split_once(DEPLOYED)?;
                let what = what.rsplit_once(" at ").map_or(what, |(what, _)| what);
                if let Some(app) = app {
                    return (said.deployed.contains(app) || among(app, what))
                        .then_some(Outcome::Met);
                }
                // A landing's apps deploy in steps of their own (web, then the system apps),
                // each said on its line: the wait is for them all. A line that doesn't say
                // what the landing deploys, as CI's did before, ends it.
                for deployed in said.deployed {
                    if !seen.contains(&deployed) {
                        seen.push(deployed);
                    }
                }
                said.deploys
                    .iter()
                    .all(|app| seen.contains(app))
                    .then_some(Outcome::Met)
            }
            _ => None,
        }
    }
}

/// What a wait names, as CI's data has it: `build 1299` and `build-1299` are the build 1299.
fn named(on: &str) -> String {
    let on = on.trim();
    let build = on
        .strip_prefix("build-")
        .or_else(|| on.strip_prefix("build "))
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    build.unwrap_or(on).to_owned()
}

/// Whether `app` is among `what`, the apps a deploy's line names (`ops/lib.sh`, `named`):
/// by name, or as one of `every app`, unless it's the one the line leaves out.
fn among(app: &str, what: &str) -> bool {
    let mut words = what.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'));
    if what.starts_with("every app") {
        return what.strip_prefix("every app but ") != Some(app);
    }
    words.any(|word| word == app)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(kind: &str, text: &str, data: serde_json::Value) -> Event {
        Event::Custom {
            kind: kind.to_owned(),
            data,
            text: text.to_owned(),
        }
    }

    fn landing(text: &str, data: serde_json::Value) -> Event {
        event(LANDING_KIND, text, data)
    }

    fn deploy(text: &str, data: serde_json::Value) -> Event {
        event(DEPLOY_KIND, text, data)
    }

    #[test]
    fn a_landing_is_met_by_its_branch_build_or_session_landing() {
        let landed = landing(
            "cc-2: landed login as 1a2b3c4 (build 1299)",
            json!({"branch": "login", "build": "1299", "session": "cc-2", "state": "passed"}),
        );
        for on in ["login", "1299", "build 1299", "build-1299", "cc-2"] {
            assert_eq!(
                Condition::landed(on).judge(&landed),
                Some(Outcome::Met),
                "{on}"
            );
        }
        assert_eq!(Condition::landed("signup").judge(&landed), None);
        // A post that only talks of it isn't CI's word.
        let talk = Event::Text {
            body: "landed login as 1a2b3c4".into(),
        };
        assert_eq!(Condition::landed("login").judge(&talk), None);
    }

    #[test]
    fn a_landing_that_fails_or_conflicts_ends_the_wait_as_failed() {
        let failed = landing(
            "cc-2: build 1299 failed: //web:unit",
            json!({"branch": "login", "build": "1299", "state": "failed"}),
        );
        let conflicts = landing(
            "cc-2: login conflicts with master: rebase, resolve, land again",
            json!({"branch": "login", "state": "warning"}),
        );
        let queued = landing("cc-2: queued login (build 1299)", json!({"build": "1299"}));
        for on in ["login", "1299", "cc-2"] {
            assert_eq!(
                Condition::landed(on).judge(&failed),
                Some(Outcome::Failed),
                "{on}"
            );
        }
        assert_eq!(
            Condition::landed("login").judge(&conflicts),
            Some(Outcome::Failed)
        );
        assert_eq!(Condition::landed("login").judge(&queued), None);
    }

    #[test]
    fn a_deploy_is_met_by_its_commit_branch_or_build_reaching_production() {
        let full = "9426072aa0c1e5a7d5f1c7d9a4b3e2f1a0b9c8d7";
        let deployed = deploy(
            "ci-token (cc-5): deployed to production: controller at 9426072 ✓",
            json!({"branch": "ci-token", "build": "2811", "commit": full, "state": "passed"}),
        );
        for on in ["9426072", full, "ci-token", "2811"] {
            assert_eq!(
                Condition::deployed(on, None).judge(&deployed),
                Some(Outcome::Met),
                "{on}"
            );
        }
        assert_eq!(Condition::deployed("1234567", None).judge(&deployed), None);
        // Staging's isn't production's, nor is waiting for approval.
        let staged = deploy(
            "deployed to staging: 9426072 from ci-token (cc-5)",
            json!({"branch": "ci-token", "commit": full}),
        );
        let waiting = deploy(
            "cc-5: approve for production: controller in build 2811",
            json!({"branch": "ci-token", "build": "2811", "state": "pending"}),
        );
        assert_eq!(Condition::deployed("ci-token", None).judge(&staged), None);
        assert_eq!(Condition::deployed("ci-token", None).judge(&waiting), None);
    }

    #[test]
    fn a_deploy_of_one_app_waits_for_a_line_that_has_it() {
        let line = |what: &str| {
            deploy(
                &format!("cc-5: deployed to production: {what} at 9426072 ✓"),
                json!({"branch": "ci-token", "state": "passed"}),
            )
        };
        let mut controller = Condition::deployed("ci-token", Some("controller"));
        assert_eq!(controller.judge(&line("web")), None);
        assert_eq!(controller.judge(&line("the edge (deploy/edge)")), None);
        assert_eq!(controller.judge(&line("every app but controller")), None);
        assert_eq!(
            controller.judge(&line("web controller")),
            Some(Outcome::Met)
        );
        let several = line("web\ncontroller\ntest-results");
        assert_eq!(controller.judge(&several), Some(Outcome::Met));
        assert_eq!(controller.judge(&line("every app")), Some(Outcome::Met));
        assert_eq!(
            controller.judge(&line("every app but web")),
            Some(Outcome::Met)
        );
    }

    #[test]
    fn a_landing_is_deployed_once_every_app_it_deploys_is() {
        let line = |what: &str, deployed: &[&str]| {
            deploy(
                &format!("cc-5: deployed to production: {what} at 9426072 ✓"),
                json!({"branch": "ci-token", "state": "passed",
                       "deploys": ["web", "controller", "edge"], "deployed": deployed}),
            )
        };
        let mut wait = Condition::deployed("ci-token", None);
        assert_eq!(wait.judge(&line("web", &["web"])), None);
        assert_eq!(wait.judge(&line("the edge (deploy/edge)", &["edge"])), None);
        assert_eq!(
            wait.judge(&line("controller", &["controller"])),
            Some(Outcome::Met)
        );
        // One app of them is met by its own line, by what the line says it deployed.
        let mut controller = Condition::deployed("ci-token", Some("controller"));
        assert_eq!(
            controller.judge(&line("the system apps", &["controller"])),
            Some(Outcome::Met)
        );
    }

    #[test]
    fn a_deploy_named_by_its_commit_hears_its_builds_failure() {
        let mut wait = Condition::deployed("9426072", None);
        let failed = deploy(
            "cc-5: deploy failed: controller in build 2811",
            json!({"build": "2811", "state": "failed"}),
        );
        // Another build's failure, for all the wait knows.
        assert_eq!(wait.judge(&failed), None);
        let landed = landing(
            "cc-5: landed ci-token as 9426072 (build 2811)",
            json!({"build": "2811", "commit": "9426072aa0c1", "state": "passed"}),
        );
        assert_eq!(wait.judge(&landed), None);
        assert_eq!(wait.judge(&failed), Some(Outcome::Failed));
        // And its deploy, at the default branch's head by then: a later commit with it in.
        let deployed = deploy(
            "cc-5: deployed to production: controller at 5cf88da \u{2713}",
            json!({"build": "2811", "commit": "5cf88da30a67", "state": "passed"}),
        );
        assert_eq!(wait.judge(&deployed), Some(Outcome::Met));
    }

    #[test]
    fn a_state_is_met_once_it_stands_as_wanted() {
        let said = |repo: &str, name: &str, level| {
            State {
                repo: repo.into(),
                name: name.into(),
                level,
                why: None,
                owner: None,
                link: None,
            }
            .event()
        };
        let mut fine = Condition::stands(Some("#api"), "master", Want::At(Level::Ok));
        assert_eq!(fine.judge(&said("#api", "master", Level::Broken)), None);
        assert_eq!(
            fine.judge(&said("#web", "master", Level::Ok)),
            None,
            "another's"
        );
        assert_eq!(fine.judge(&said("#api", "staging", Level::Ok)), None);
        assert_eq!(
            fine.judge(&said("#api", "master", Level::Ok)),
            Some(Outcome::Met)
        );
        // Never said is fine.
        assert!(fine.met_at(None));
        assert!(!fine.met_at(Some(Level::Warning)));

        let mut changes = Condition::stands(None, "master", Want::Not(Some(Level::Broken)));
        assert_eq!(changes.judge(&said("#api", "master", Level::Broken)), None);
        assert_eq!(
            changes.judge(&said("#api", "master", Level::Warning)),
            Some(Outcome::Met)
        );
        assert!(!Condition::landed("master").met_at(Some(Level::Ok)));
    }

    #[test]
    fn says_what_it_waits_for() {
        assert_eq!(
            Condition::stands(None, "master", Want::At(Level::Ok)).describe(),
            "master to be ok"
        );
        assert_eq!(
            Condition::stands(None, "master", Want::Not(None)).describe(),
            "master to change from ok"
        );
        assert_eq!(Condition::landed("build 1299").describe(), "1299 to land");
        assert_eq!(
            Condition::deployed("9426072", Some("controller")).describe(),
            "controller from 9426072 to be deployed to production"
        );
    }
}
