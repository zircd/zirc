//! What CI says in zirc, as [`crate::Event::Custom`] events of zirc's own kinds: how a
//! landing went ([`LANDING_KIND`]: landed, failed, conflicts, the default branch broken) and
//! how a deploy goes ([`DEPLOY_KIND`]: deploying, deployed, failed, waiting for approval).
//! The text says it for people and names the session it's for, which wakes that session's
//! `zirc watch`; the data, a [`Data`], says it for tools and the apps.
//!
//! CI's scripts name the kinds by their [`short`] names (`ops/say.sh landing …`), from
//! `ops/kinds.sh`, which `//ops:kinds` writes from here.

use serde::{Deserialize, Serialize};

use crate::card::{Action, Card, Field, Link, State, Who};

/// How a landing went: queued through CI, tested, landed, failed or conflicted.
pub const LANDING_KIND: &str = "x-zirc/landing";

/// How a deploy goes: to staging or production, waiting for approval, done or failed.
pub const DEPLOY_KIND: &str = "x-zirc/deploy";

/// A check of the repository that isn't a landing: the default branch's head, a daily full
/// run, a flaky test.
pub const CHECK_KIND: &str = "x-zirc/check";

/// The nick CI posts as on an organisation's server, with the organisation's CI token. One of
/// [`crate::names::RESERVED_INTEGRATION_NICKS`], so no team's hook can post as it.
pub const NICK: &str = "zirc-ci";

/// The session CI's posts come from, which `zirc watch` shows after its nick.
pub const SESSION: &str = "ci";

/// Whether CI may say an event of `kind`: its own ([`LANDING_KIND`], [`DEPLOY_KIND`],
/// [`CHECK_KIND`]), how something in the repository stands ([`crate::state::KIND`]), or a
/// team's own. Never zirc's other kinds, which say who is changing what.
#[must_use]
pub fn may_say(kind: &str) -> bool {
    KINDS.iter().any(|(_, own)| *own == kind) || !kind.starts_with("x-zirc/")
}

/// Whether an event of `kind` is one only CI says: how a landing, a deploy or a check of the
/// repository went. Sessions act on these without asking (a landing ends their task, a broken
/// default branch names who fixes it), so whose word one is matters: see [`speaks_for_ci`].
#[must_use]
pub fn only_cis(kind: &str) -> bool {
    matches!(kind, LANDING_KIND | DEPLOY_KIND | CHECK_KIND)
}

/// Whether `account`, the account the server says sent something, is one CI's word comes
/// from: an integration's ([`crate::names::is_integration_nick`]), as [`NICK`] is and the
/// alerts' that says how the Apple apps built. Only an organisation's owners and admins set
/// those up, and no person, machine or guest can hold one, so a member's machine saying a
/// landing landed, or that the default branch broke and who broke it, isn't CI saying so.
#[must_use]
pub fn speaks_for_ci(account: Option<&str>) -> bool {
    account.is_some_and(|account| crate::names::is_integration_nick(&account.to_ascii_lowercase()))
}

/// What an event CI says asks of its readers: the level CI `gave`; else, for how something
/// stands ([`crate::state::KIND`]), the level it's at: `ok` is info, `warning` warn, `broken`
/// an error. Else `None`, and its readers take the level the state in its data says
/// ([`crate::Message::declared_level`]).
#[must_use]
pub fn level(
    kind: &str,
    data: &serde_json::Value,
    gave: Option<crate::Level>,
) -> Option<crate::Level> {
    gave.or_else(|| {
        if kind != crate::state::KIND {
            return None;
        }
        match data.get("level").and_then(serde_json::Value::as_str)? {
            "ok" => Some(crate::Level::Info),
            "warning" => Some(crate::Level::Warn),
            "broken" => Some(crate::Level::Error),
            _ => None,
        }
    })
}

/// The most CI's text is, in characters: a failure names its targets on lines of their own.
pub const MAX_TEXT: usize = 2000;

/// Each kind CI says with the short name CI's scripts use for it.
pub const KINDS: [(&str, &str); 4] = [
    ("landing", LANDING_KIND),
    ("deploy", DEPLOY_KIND),
    ("check", CHECK_KIND),
    ("state", crate::state::KIND),
];

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript() -> Vec<String> {
    let kinds: Vec<String> = KINDS
        .iter()
        .map(|(short, kind)| format!("{short}: {kind:?}"))
        .collect();
    vec![
        format!("const CI_NICK = {NICK:?};"),
        format!("const CI_MAX_TEXT = {MAX_TEXT};"),
        format!(
            "const CI_KINDS: Readonly<Record<string, string>> = {{ {} }};",
            kinds.join(", ")
        ),
    ]
}

/// The kind CI's scripts call `name` (`landing`, `deploy`, `check`, `state`), if it's one.
#[must_use]
pub fn short(name: &str) -> Option<&'static str> {
    KINDS
        .iter()
        .find(|(short, _)| *short == name)
        .map(|(_, kind)| *kind)
}

/// What a landing's or deploy's event carries (`ops/say.sh`), each field left out when CI
/// doesn't know it. Every event of one landing's build has the same [`Data::landing`], so the
/// apps show them as one landing that updates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "CiData")]
pub struct Data {
    /// The landing's id, `<pipeline>/<build>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<String>,
    /// CI's pipeline, such as `monorepo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<String>,
    /// The build's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// The branch landing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// What the landing is about: its head commit's subject, which apps show as its headline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The session that queued it, whom the text names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The repository's web address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The full id of the commit the text names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The build's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// How it stands: `pending`, `running`, `passed`, `failed`, `cancelled` or `warning`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The targets that failed, each with where its result is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<Failed>,
    /// The task the landing finishes or is part of ([`crate::Message::task`]), when its
    /// session was on one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Full ids of the commits on the default branch the landing undoes, from its commits'
    /// `Reverts:` trailers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reverts: Vec<String>,
    /// What the landing repairs, from its commits' `Fixes:` trailers: full ids of commits on
    /// the default branch, or CI builds as `build N`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<String>,
    /// On a landing that passed: the paths it changed, from the repository's root, for what
    /// follows landings by where they land (a workflow's `landed` trigger). Where it changed
    /// many files in a directory, CI may name the directory, ending in `/`, in their place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// On a deploy's event: every app the landing deploys to production, of every kind, so a
    /// wait for the landing to be deployed ends once all of them are, not on the first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deploys: Vec<String>,
    /// On a deploy's event: the apps this event says were deployed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deployed: Vec<String>,
}

/// A target that failed in a landing's build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "CiFailed")]
pub struct Failed {
    /// The Bazel target, such as `//web:e2e`.
    pub target: String,
    /// Where its result in this build is, on zirc.dev/test-results.
    pub results: String,
}

impl Data {
    /// What `data`, a landing's or deploy's event's data, says; fields it doesn't have, or
    /// has in another shape, are left out. Empty strings count as unknown, as CI leaves them.
    #[must_use]
    pub fn of(data: &serde_json::Value) -> Self {
        let mut it: Self = serde_json::from_value(data.clone()).unwrap_or_default();
        for field in [
            &mut it.landing,
            &mut it.pipeline,
            &mut it.build,
            &mut it.branch,
            &mut it.subject,
            &mut it.session,
            &mut it.repo,
            &mut it.commit,
            &mut it.url,
            &mut it.state,
            &mut it.task,
        ] {
            if field.as_deref() == Some("") {
                *field = None;
            }
        }
        it
    }
}

/// A landing as a card (`docs/cards.md`), from what CI's events carry (`ops/say.sh`):
/// its id (`landing`, `<pipeline>/<build>`), branch, build, session and `state`; the build's
/// `url`, the targets that `failed` with their results, and the `commit` in the `repo`. One
/// card for every event of a landing, landed, failed, deploying or retried, each replacing
/// the last, with Retry while it's failed, for its owner (ops/src/cards.rs answers it). None
/// for an event that says no landing or state, or a card apps couldn't draw.
#[must_use]
pub fn landing_card(data: &serde_json::Value, text: &str) -> Option<Card> {
    let data = Data::of(data);
    let id = data.landing.as_deref()?;
    let state = match data.state.as_deref()? {
        "pending" => State::Pending,
        "running" => State::Running,
        "passed" => State::Passed,
        "failed" => State::Failed,
        "cancelled" => State::Cancelled,
        "warning" => State::Warning,
        _ => State::Info,
    };
    let build = data.build.as_deref().unwrap_or_default();
    // Its headline is what it's about, the commit's subject, with its branch and build beside
    // it; without one, its branch and build.
    let title = match (&data.subject, &data.branch) {
        (Some(subject), _) => subject.clone(),
        (None, Some(branch)) => format!("{branch} · build {build}"),
        (None, None) => format!("build {build}"),
    };
    let mut links = Vec::new();
    if let Some(url) = &data.url {
        links.push(Link {
            label: "Log".into(),
            url: url.clone(),
        });
    }
    for failed in &data.failed {
        links.push(Link {
            label: crate::clip(&failed.target, 24),
            url: failed.results.clone(),
        });
    }
    let mut fields = Vec::new();
    if data.subject.is_some() {
        if let Some(branch) = &data.branch {
            fields.push(Field {
                label: "branch".into(),
                value: crate::clip(branch, 40),
            });
        }
        if !build.is_empty() {
            fields.push(Field {
                label: "build".into(),
                value: build.to_owned(),
            });
        }
    }
    if let Some(commit) = &data.commit {
        fields.push(Field {
            label: "commit".into(),
            value: commit.chars().take(7).collect(),
        });
    }
    if let (Some(repo), Some(commit)) = (&data.repo, &data.commit) {
        links.push(Link {
            label: "Commit".into(),
            url: format!("{repo}/commit/{commit}"),
        });
    }
    links.truncate(4);
    let actions = if state == State::Failed {
        vec![Action {
            id: "retry".into(),
            label: "Retry".into(),
            who: Who::About,
            confirm: None,
        }]
    } else {
        Vec::new()
    };
    // The line, without whom it's for, which the card shows as its `about`.
    let line = text.lines().next().unwrap_or_default();
    let session = data.session.as_deref();
    let note = session
        .and_then(|s| line.strip_prefix(s))
        .and_then(|rest| rest.strip_prefix(": "))
        .unwrap_or(line);
    // Without addresses: apps draw a note as plain text, and the card links the build's page.
    let note = note
        .split(' ')
        .filter(|word| !word.starts_with("https://") && !word.starts_with("http://"))
        .collect::<Vec<_>>()
        .join(" ");
    let note = note.trim_end_matches([' ', ':']);
    let card = Card {
        id: id.to_owned(),
        title: crate::clip(&title, 80),
        state,
        about: session.map(str::to_owned),
        fields,
        links,
        actions,
        note: (!note.is_empty()).then(|| crate::clip(note, 120)),
    };
    card.validate().ok().map(|()| card)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_integrations_account_speaks_for_ci() {
        assert!(only_cis(LANDING_KIND) && only_cis(DEPLOY_KIND) && only_cis(CHECK_KIND));
        assert!(!only_cis(crate::state::KIND) && !only_cis("acme/build"));
        assert!(speaks_for_ci(Some(NICK)) && speaks_for_ci(Some("Zirc-Alerts")));
        // A member, their machine, a guest, a nick with no account, or `zirc` itself.
        for other in ["ada", "ada-mbp", "ci-3", "guest-7k2q", "zirc", "zirc-"] {
            assert!(!speaks_for_ci(Some(other)), "{other}");
        }
        assert!(!speaks_for_ci(None));
    }

    #[test]
    fn names_each_kind_once() {
        assert_eq!(short("landing"), Some(LANDING_KIND));
        assert_eq!(short("deploy"), Some(DEPLOY_KIND));
        assert_eq!(short("x-zirc/landing"), None);
        for (_, kind) in KINDS {
            assert_eq!(crate::check_kind(kind), None, "{kind}");
        }
    }

    #[test]
    fn ci_says_its_own_kinds_and_a_teams_but_no_other_of_zircs() {
        for kind in [
            LANDING_KIND,
            DEPLOY_KIND,
            CHECK_KIND,
            crate::state::KIND,
            "x-acme/deploy",
        ] {
            assert!(may_say(kind), "{kind}");
        }
        for kind in ["x-zirc/session", "x-zirc/decision", "x-zirc/request"] {
            assert!(!may_say(kind), "{kind}");
        }
        assert!(crate::names::RESERVED_INTEGRATION_NICKS.contains(&NICK));
    }

    #[test]
    fn a_state_s_level_is_how_it_stands_unless_ci_gave_one() {
        let broken = serde_json::json!({"level": "broken"});
        assert_eq!(
            level(crate::state::KIND, &broken, None),
            Some(crate::Level::Error)
        );
        assert_eq!(
            level(crate::state::KIND, &broken, Some(crate::Level::Warn)),
            Some(crate::Level::Warn)
        );
        assert_eq!(level(LANDING_KIND, &broken, None), None);
    }

    #[test]
    fn reads_what_ci_says() {
        let data = serde_json::json!({
            "landing": "monorepo/7", "build": "7", "state": "failed", "branch": "",
            "failed": [{"target": "//web:e2e", "results": "https://zirc.dev/test-results/x"}],
            "later": true,
        });
        let it = Data::of(&data);
        assert_eq!(it.landing.as_deref(), Some("monorepo/7"));
        assert_eq!(it.branch, None, "an empty field is unknown");
        assert_eq!(it.failed[0].target, "//web:e2e");
        assert_eq!(
            Data::of(&serde_json::json!("not an object")),
            Data::default()
        );
    }

    /// With its commit's subject, the card's headline is what the landing is about, and its
    /// branch and build are fields beside it.
    #[test]
    fn a_landing_with_a_subject_is_headed_by_it() {
        let data = serde_json::json!({
            "landing": "monorepo/1620", "build": "1620", "state": "running", "session": "cc-1",
            "branch": "report-block-core", "subject": "Reports and blocks in zirc-core",
        });
        let card = landing_card(&data, "build 1620: mattias retried it").unwrap();
        assert_eq!(card.title, "Reports and blocks in zirc-core");
        assert_eq!(
            card.fields
                .iter()
                .map(|f| (f.label.as_str(), f.value.as_str()))
                .collect::<Vec<_>>(),
            [("branch", "report-block-core"), ("build", "1620")]
        );
    }

    /// Landed, it's a passed card with no buttons; an event without a landing or state, such
    /// as an alert, is no card at all.
    #[test]
    fn only_landings_with_a_state_are_cards() {
        let data = serde_json::json!({"landing": "monorepo/7", "build": "7", "state": "passed", "session": "cc-1"});
        let card = landing_card(&data, "cc-1: landed x as abc1234 (build 7)").unwrap();
        assert_eq!(card.state, State::Passed);
        assert!(card.actions.is_empty());
        assert_eq!(card.title, "build 7");
        assert!(landing_card(&serde_json::json!({"landing": "monorepo/7"}), "x").is_none());
        assert!(landing_card(&serde_json::json!({"state": "passed"}), "x").is_none());
        // One apps can't draw, here an id with a space, is left out rather than sent.
        assert!(
            landing_card(
                &serde_json::json!({"landing": "a b", "state": "passed"}),
                "x"
            )
            .is_none()
        );
    }
}
