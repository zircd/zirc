//! Landings, CI lines and the branches that landed: whom they wake.

use zirc_core::ipc::{Request, Response, Target, Watching, Why};
use zirc_proto::Event;
use zirc_test_ergo::git::git;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn a_landing_wakes_the_sessions_it_changes_files_under_but_not_one_already_at_it() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    git(
        &main_root,
        &[
            "remote",
            "set-url",
            "origin",
            remote.path().to_str().unwrap(),
        ],
    );
    std::fs::write(main_root.join("lines.txt"), ten_lines(0, "")).unwrap();
    git(&main_root, &["add", "lines.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Lines"]);
    git(&main_root, &["push", "--quiet", "origin", "main"]);
    let mut login_hears = machine.watch(Some("cc-login")).await;
    // Once zircd has seen main pushed, the next push says what it changed.
    until_who(
        &machine,
        |channels| channels.iter().any(|c| !c.landings.is_empty()),
        "the first push",
    )
    .await;

    // The login session changes lines.txt; main lands a change to it.
    std::fs::write(login_root.join("lines.txt"), ten_lines(9, "login's")).unwrap();
    until_who(
        &machine,
        |channels| {
            channels
                .iter()
                .flat_map(|c| &c.members)
                .any(|m| m.session.as_deref() == Some("cc-login") && m.paths == ["lines.txt"])
        },
        "the login worktree's change",
    )
    .await;
    std::fs::write(main_root.join("lines.txt"), ten_lines(1, "main's")).unwrap();
    git(
        &main_root,
        &["commit", "--quiet", "--all", "--message", "Main's line"],
    );
    git(&main_root, &["push", "--quiet", "origin", "main"]);
    let (event, why) = next_concerning(&mut login_hears).await;
    assert!(matches!(event, Event::Push { .. }), "{event:?}");
    assert_eq!(why, Why::Landing);

    // Main makes another change, the login worktree catches up to it before it's pushed,
    // and then it lands: the login session is at it already.
    std::fs::write(main_root.join("lines.txt"), ten_lines(2, "main's again")).unwrap();
    git(
        &main_root,
        &["commit", "--quiet", "--all", "--message", "Again"],
    );
    std::fs::remove_file(login_root.join("lines.txt")).unwrap();
    git(&login_root, &["merge", "--quiet", "--ff-only", "main"]);
    std::fs::write(login_root.join("lines.txt"), ten_lines(9, "login's")).unwrap();
    // Changed after catching up, so the scan that lists it has seen the new HEAD too.
    std::fs::write(login_root.join("notes.md"), "login's notes\n").unwrap();
    until_who(
        &machine,
        |channels| {
            channels.iter().flat_map(|c| &c.members).any(|m| {
                m.session.as_deref() == Some("cc-login") && m.paths == ["lines.txt", "notes.md"]
            })
        },
        "the login worktree caught up",
    )
    .await;
    git(&main_root, &["push", "--quiet", "origin", "main"]);
    let push = loop {
        let delivery = next(&mut login_hears).await;
        if matches!(&delivery.message.event, Event::Push { subjects, .. } if subjects == &["Again"])
        {
            break delivery;
        }
    };
    assert_eq!(push.why, None);
}

#[tokio::test]
async fn a_sessions_own_ci_success_waits_and_its_failure_wakes_it() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), _] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    // CI, as one of zirc's own accounts, in the repository's channel.
    let (ci, mut ci_in) = zirc_test_irc::integration(&ergo, "zirc-ci").await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#api").await;
    let landed = "cc-1: landed login as 1a2b3c4 (build 7)";
    ci.privmsg("#api", landed).unwrap();
    assert_eq!(next_post_why(&mut cc1).await, (landed.into(), None));
    // A failure wakes it, with what waited first.
    let failed = "cc-1: build 8 of login failed in test: //api:unit";
    ci.privmsg("#api", failed).unwrap();
    let (intro, why) = next_post_why(&mut cc1).await;
    assert_eq!(
        (intro.as_str(), why),
        (
            "cc-1: meanwhile, for you and needing nothing yet:",
            Some(Why::Mention)
        )
    );
    assert_eq!(
        next_post_why(&mut cc1).await,
        (landed.into(), Some(Why::Mention))
    );
    assert_eq!(
        next_post_why(&mut cc1).await,
        (failed.into(), Some(Why::Mention))
    );
    // Said again, as CI says a landing in more than one channel, it's in no second digest.
    ci.privmsg("#api", landed).unwrap();
    assert_eq!(next_post_why(&mut cc1).await, (landed.into(), None));
    let again = "cc-1: build 9 of login failed in test: //api:unit";
    ci.privmsg("#api", again).unwrap();
    assert_eq!(
        next_post_why(&mut cc1).await,
        (again.into(), Some(Why::Mention))
    );
}

#[tokio::test]
async fn landings_and_a_broken_state_are_said_with_a_sessions_next_wake() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), _] = two_machines(&ergo).await;
    let add = Request::RepoAdd {
        path: alpha_clone.path().to_owned(),
        session: Some("cc-1".into()),
    };
    alpha.request(add).await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let (ci, mut ci_in) = zirc_test_irc::integration(&ergo, "zirc-ci").await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#api").await;
    // Two landings of another session's, as ops/say.sh says them: neither wakes cc-1.
    for (commit, subject) in [("c", "Fix login"), ("d", "Add logout")] {
        let event = Event::Custom {
            kind: "x-zirc/landing".into(),
            data: serde_json::json!({
                "state": "passed", "session": "cc-9", "branch": "login",
                "commit": commit.repeat(40), "subject": subject,
            }),
            text: format!("cc-9: landed login as {} (build 7)", commit.repeat(7)),
        };
        let line = zirc_proto::encode(&zirc_proto::Message::new("ci", event)).unwrap();
        ci.privmsg("#api", &line).unwrap();
        assert_eq!(next(&mut cc1).await.why, None);
    }
    // Nor does master said broken, which names nobody here; broken twice is one note.
    let state = |level| Request::SetState {
        repo: alpha_clone.path().to_owned(),
        session: "ci".into(),
        name: "master".into(),
        level,
        why: Some("//api:unit fails".into()),
        owner: None,
        link: None,
        until: None,
    };
    alpha.request(state(zirc_proto::state::Level::Broken)).await;
    alpha.request(state(zirc_proto::state::Level::Broken)).await;
    // What wakes it brings them first: one line for both landings, and the state.
    let failed = "cc-1: build 8 of login failed in test: //api:unit";
    ci.privmsg("#api", failed).unwrap();
    let mut said = Vec::new();
    while said.last().is_none_or(|(text, _)| text != failed) {
        let delivery = next(&mut cc1).await;
        if let Some(why) = delivery.why {
            said.push((delivery.message.event.human_text(), why));
        }
    }
    let texts: Vec<&str> = said.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(
        texts[0],
        "cc-1: meanwhile, for you and needing nothing yet:"
    );
    assert_eq!(
        (texts[1], said[1].1),
        (
            "cc-1: main moved: 2 landings; the newest is ddddddd \"Add logout\"",
            Why::DefaultMoved
        )
    );
    assert!(
        texts[2].contains("master") && texts[2].contains("broken"),
        "{texts:?}"
    );
    assert_eq!((said[2].1, said.len()), (Why::StateChanged, 4));
}

#[tokio::test]
async fn the_apple_apps_passing_with_a_landing_waits_for_the_next_wake() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), _] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    // //ops:land, for a change to the Apple apps, waits on the Mac's build of master too.
    alpha.request(wait_for("cc-1", &["apple/login"])).await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    // CI's account, saying it as ops/say.sh does: a landing event from its session `ci`.
    let (ci, mut ci_in) = zirc_test_irc::integration(&ergo, "zirc-alerts").await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#api").await;
    let landing = |text: &str| {
        zirc_proto::encode(&zirc_proto::Message::new(
            "ci",
            Event::Custom {
                kind: "x-zirc/landing".into(),
                data: serde_json::json!({ "branch": "apple/login" }),
                text: text.into(),
            },
        ))
        .unwrap()
    };
    let next_landing = async |watching: &mut Watching| loop {
        let delivery = next(watching).await;
        if let Event::Custom { text, .. } = delivery.message.event {
            return (text, delivery.why);
        }
    };
    // They compile: awaited, but needing nothing, so it wakes nobody yet.
    let pass = "cc-1 (apple/login): build 7's Apple apps compile on 1a2b3c4";
    ci.privmsg("#api", &landing(pass)).unwrap();
    assert_eq!(next_landing(&mut cc1).await, (pass.into(), None));
    // The next line that wakes it brings it first.
    let failed = "cc-1: build 8 of login failed in test: //api:unit";
    ci.privmsg("#api", failed).unwrap();
    assert_eq!(
        next_post_why(&mut cc1).await,
        (
            "cc-1: meanwhile, for you and needing nothing yet:".into(),
            Some(Why::Mention)
        )
    );
    assert_eq!(next_landing(&mut cc1).await.0, pass);
    assert_eq!(
        next_post_why(&mut cc1).await,
        (failed.into(), Some(Why::Mention))
    );
    // Their failure wakes it at once.
    let broke = "cc-1 (apple/login): build 9's Apple apps failed to compile on 1a2b3c4";
    ci.privmsg("#api", &landing(broke)).unwrap();
    assert_eq!(
        next_landing(&mut cc1).await,
        (broke.into(), Some(Why::Awaited))
    );
}

#[tokio::test]
async fn a_landing_a_session_waits_on_wakes_it() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    for (machine, clone, session) in [
        (&alpha, &alpha_clone, Some("cc-1")),
        (&beta, &beta_clone, None),
    ] {
        machine
            .request(Request::Join {
                repo: clone.path().to_owned(),
                topic: "landings".into(),
                org: false,
                session: session.map(Into::into),
            })
            .await;
        machine
            .eventually(
                |status| status.channels.iter().any(|c| c == "#api/landings"),
                "joining #api/landings",
            )
            .await;
    }
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    // CI's line, as //ops:land's are: an x-zirc/landing event, from CI's own account.
    let (ci, mut ci_in) = zirc_test_irc::integration(&ergo, "zirc-ci").await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#api/landings").await;
    let says = |text: &str| {
        let event = Event::Custom {
            kind: "x-zirc/landing".into(),
            data: serde_json::Value::Null,
            text: text.into(),
        };
        let line = zirc_proto::encode(&zirc_proto::Message::new("ci", event)).unwrap();
        ci.privmsg("#api/landings", &line).unwrap();
    };
    let next_landing = async |watching: &mut Watching| loop {
        let delivery = next(watching).await;
        if let Event::Custom { text, .. } = delivery.message.event {
            return (text, delivery.why);
        }
    };
    let landed = "cc-2: landed fix-login as 1a2b3c4 (build 1299)";
    says(landed);
    assert_eq!(
        next_landing(&mut cc1).await,
        (landed.into(), None),
        "another's landing"
    );
    // Said it waits on build 1300, in a post: CI's line on it wakes it, once.
    alpha
        .request(Request::Post {
            repo: alpha_clone.path().to_owned(),
            session: "cc-1".into(),
            text: "waiting for build 1300 before I land".into(),
            to: Target::Default,
            app: None,
            everyone: false,
            force: true,
            reply: None,
        })
        .await;
    let landed = "cc-3: landed api-keys as 5d6e7f8 (build 1300)";
    for why in [Some(Why::Awaited), None] {
        says(landed);
        assert_eq!(next_landing(&mut cc1).await, (landed.into(), why));
    }
    // And with zirc wait-for, on a session.
    let Response::Awaits { on } = alpha.request(wait_for("cc-1", &["cc-4"])).await else {
        panic!("what cc-1 waits on");
    };
    assert_eq!(on, ["cc-4"]);
    let failed = "cc-4: build 1301 failed: //web:e2e";
    says(failed);
    assert_eq!(
        next_landing(&mut cc1).await,
        (failed.into(), Some(Why::Awaited))
    );
}

/// Waiting on a build, as //ops:land does: how its steps stand while it runs, its start and
/// each step's live update (ops/land.sh's step), wakes nobody; how it ends does.
#[tokio::test]
async fn a_landings_steps_never_wake_a_session_waiting_on_it() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), _] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    alpha
        .request(Request::Join {
            repo: alpha_clone.path().to_owned(),
            topic: "landings".into(),
            org: false,
            session: Some("cc-1".into()),
        })
        .await;
    alpha
        .eventually(
            |status| status.channels.iter().any(|c| c == "#api/landings"),
            "joining #api/landings",
        )
        .await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let (ci, mut ci_in) = zirc_test_irc::integration(&ergo, "zirc-ci").await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#api/landings").await;
    let next_landing = async |watching: &mut Watching| loop {
        let delivery = next(watching).await;
        if let Event::Custom { text, .. } = delivery.message.event {
            return (text, delivery.why);
        }
    };
    alpha.request(wait_for("cc-1", &["1302"])).await;
    let says = |text: &str, state: &str, level: Option<zirc_proto::Level>| {
        let steps = serde_json::json!([{ "name": "Build and test", "state": "running" }]);
        let mut message = zirc_proto::Message::new(
            "ci",
            Event::Custom {
                kind: "x-zirc/landing".into(),
                data: serde_json::json!({ "build": "1302", "state": state, "steps": steps }),
                text: text.into(),
            },
        );
        message.level = level;
        let line = zirc_proto::encode_up_to(&message, 4096).unwrap();
        ci.privmsg("#api/landings", &line).unwrap();
    };
    for (text, level) in [
        ("build 1302 started", None),
        ("build 1302: Build and test", Some(zirc_proto::Level::Debug)),
    ] {
        says(text, "running", level);
        assert_eq!(next_landing(&mut cc1).await, (text.into(), None), "{text}");
    }
    let failed = "cc-4: build 1302 failed: //web:unit";
    says(failed, "failed", None);
    assert_eq!(
        next_landing(&mut cc1).await,
        (failed.into(), Some(Why::Awaited))
    );
}

#[tokio::test]
async fn a_landings_line_is_news_only_to_the_session_it_names() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    // Both sessions joined the repository's landings channel, as //ops:land has them.
    for session in ["cc-1", "cc-2"] {
        alpha
            .request(Request::RepoAdd {
                path: alpha_clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
        alpha
            .request(Request::Join {
                repo: alpha_clone.path().to_owned(),
                topic: "landings".into(),
                org: false,
                session: Some(session.into()),
            })
            .await;
    }
    beta.request(Request::Join {
        repo: beta_clone.path().to_owned(),
        topic: "landings".into(),
        org: false,
        session: None,
    })
    .await;
    for machine in [&alpha, &beta] {
        machine
            .eventually(
                |status| status.channels.iter().any(|c| c == "#api/landings"),
                "joining #api/landings",
            )
            .await;
    }
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let to_landings = |text: &str| Request::Post {
        repo: beta_clone.path().to_owned(),
        session: "ci".into(),
        text: text.into(),
        to: Target::Topic {
            name: "landings".into(),
            org: false,
        },
        app: None,
        everyone: false,
        force: true,
        reply: None,
    };
    // Another session's landing, and its Apple apps passing: nothing to cc-1.
    beta.request(to_landings("cc-2: build 7's Apple apps pass on 1a2b3c4"))
        .await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("cc-2: build 7's Apple apps pass on 1a2b3c4".into(), None)
    );
    // Its own is.
    beta.request(to_landings("cc-1: landed login as 5d6e7f8 (build 8)"))
        .await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        (
            "cc-1: landed login as 5d6e7f8 (build 8)".into(),
            Some(Why::Mention)
        )
    );
}

#[tokio::test]
async fn a_branch_that_changed_a_sessions_files_wakes_it_once_when_it_lands() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    beta.request(Request::RepoAdd {
        path: beta_clone.path().to_owned(),
        session: Some("cc-b".into()),
    })
    .await;
    for (machine, clone, session) in [
        (&alpha, &alpha_clone, None),
        (&beta, &beta_clone, Some("cc-b")),
    ] {
        machine
            .request(Request::Join {
                repo: clone.path().to_owned(),
                topic: "landings".into(),
                org: false,
                session: session.map(Into::into),
            })
            .await;
        machine
            .eventually(
                |status| status.channels.iter().any(|c| c == "#api/landings"),
                "joining #api/landings",
            )
            .await;
    }
    std::fs::write(beta_clone.path().join("README.md"), "beta's take\n").unwrap();
    until_own_sets(
        &beta,
        "beta",
        &[own_set("main", Some("cc-b"), &["README.md"])],
        "beta's change",
    )
    .await;
    let mut cc_b = beta.watch(Some("cc-b")).await;
    let (ci, mut ci_in) = zirc_test_irc::integration(&ergo, "zirc-ci").await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#api/landings").await;
    let says = |text: &str, branch: &str| {
        let event = Event::Custom {
            kind: "x-zirc/landing".into(),
            data: serde_json::json!({ "branch": branch }),
            text: text.into(),
        };
        let line = zirc_proto::encode(&zirc_proto::Message::new("ci", event)).unwrap();
        ci.privmsg("#api/landings", &line).unwrap();
    };
    let next_landing = async |watching: &mut Watching| loop {
        let delivery = next(watching).await;
        if let Event::Custom { text, .. } = delivery.message.event {
            return (text, delivery.why);
        }
    };

    // Before anything told cc-b of fix-readme, its landing is another's.
    let early = "cc-a: landed fix-readme as 0a0b0c0 (build 10)";
    says(early, "fix-readme");
    assert_eq!(next_landing(&mut cc_b).await, (early.into(), None));

    // Alpha commits on fix-readme in the file cc-b is changing: news to cc-b.
    git(
        alpha_clone.path(),
        &["checkout", "--quiet", "-b", "fix-readme"],
    );
    std::fs::write(alpha_clone.path().join("README.md"), "alpha's take\n").unwrap();
    git(
        alpha_clone.path(),
        &["commit", "--quiet", "--all", "--message", "Alpha's take"],
    );
    let (branch, why) = loop {
        let delivery = next(&mut cc_b).await;
        if let Event::Commit { branch, .. } = delivery.message.event {
            break (branch, delivery.why);
        }
    };
    // On fix-readme, not on main, which it was switched from just before.
    assert_eq!((branch.as_str(), why), ("fix-readme", Some(Why::Paths)));
    let Response::Awaits { on } = beta.request(wait_for("cc-b", &[])).await else {
        panic!("what cc-b waits on");
    };
    assert_eq!(on, ["fix-readme"], "zircd waits on it for cc-b");

    // Its failing, and another branch landing, stay quiet.
    for (text, branch) in [
        (
            "cc-a: build 11 of fix-readme failed in test: //api:unit",
            "fix-readme",
        ),
        (
            "cc-c: landed fix-readme-2 as 5d6e7f8 (build 12)",
            "fix-readme-2",
        ),
    ] {
        says(text, branch);
        assert_eq!(next_landing(&mut cc_b).await, (text.into(), None), "{text}");
    }
    // Its landing wakes cc-b, once.
    let landed = "cc-a: landed fix-readme as 1a2b3c4 (build 13)";
    for why in [Some(Why::BranchLanded), None] {
        says(landed, "fix-readme");
        assert_eq!(next_landing(&mut cc_b).await, (landed.into(), why));
    }
}

/// A landing's event from a member's machine isn't CI's word, whatever it says: zircd hands it
/// on as a plain post, so it ends no wait and finishes nobody's task.
#[tokio::test]
async fn a_machines_landing_event_is_a_post_and_ends_no_wait() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    for (machine, clone, session) in [
        (&alpha, &alpha_clone, Some("cc-1")),
        (&beta, &beta_clone, None),
    ] {
        machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: session.map(Into::into),
            })
            .await;
    }
    alpha.request(wait_for("cc-1", &["login"])).await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let landed = "cc-1: landed login as 1a2b3c4 (build 7)";
    beta.request(Request::Emit {
        repo: beta_clone.path().to_owned(),
        session: "ci".into(),
        kind: "x-zirc/landing".into(),
        data: serde_json::json!({ "branch": "login", "session": "cc-1", "state": "passed" }),
        text: landed.into(),
        to: Target::Default,
        app: None,
    })
    .await;
    let heard = next(&mut cc1).await;
    assert_eq!(
        heard.message.event,
        Event::Text {
            body: landed.into()
        }
    );
    let Response::Awaits { on } = alpha.request(wait_for("cc-1", &[])).await else {
        panic!("what cc-1 waits on");
    };
    assert_eq!(on, ["login"], "still waited on: only CI says it landed");
}

/// `session` waiting on each of `on`, or saying what it waits on, as `zirc wait-for` asks.
fn wait_for(session: &str, on: &[&str]) -> Request {
    Request::WaitFor {
        session: session.into(),
        on: on.iter().map(|&it| it.to_owned()).collect(),
        states: Vec::new(),
        reactions: Vec::new(),
        drop: false,
    }
}

/// A session stacked on a branch that's deleted from origin without landing is told so, once a
/// minute's look at origin finds it gone: not to land it, and the rebase past it. In the agent
/// evals' S5 a session waited on such a base for good, and another landed it to free its own.
#[tokio::test]
async fn a_session_stacked_on_a_branch_dropped_unlanded_is_told_what_to_do() {
    let ergo = require_ergo!();
    let clock = zircd::Clock::default();
    let hooks = zircd::Hooks {
        clock: clock.clone(),
        ..zircd::Hooks::default()
    };
    let machine = Machine::hooked(&ergo, "alpha", hooks).await;
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    let clone = clone();
    let root = clone.path();
    git(
        root,
        &[
            "remote",
            "set-url",
            "origin",
            remote.path().to_str().unwrap(),
        ],
    );
    git(root, &["push", "--quiet", "origin", "main"]);
    git(root, &["switch", "--quiet", "-c", "money"]);
    std::fs::write(root.join("money.py"), "def format_cents(c): ...\n").unwrap();
    git(root, &["add", "money.py"]);
    git(root, &["commit", "--quiet", "--message", "Money"]);
    git(root, &["push", "--quiet", "origin", "money"]);
    let tip = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "money"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    machine
        .request(Request::RepoAdd {
            path: root.to_owned(),
            session: Some("cc-top".into()),
        })
        .await;
    let mut top = machine.watch(Some("cc-top")).await;
    let stack = |branch: Option<&str>, tip: Option<&str>| Request::Stack {
        session: "cc-top".into(),
        branch: branch.map(str::to_owned),
        tip: tip.map(str::to_owned),
        drop: false,
        gone: false,
    };
    machine
        .request(stack(Some("money"), Some(tip.trim())))
        .await;

    // Dropped: deleted from origin, from this repository, never landed. zircd sees its ref go.
    git(root, &["push", "--quiet", "origin", "--delete", "money"]);
    let mut looks = 0;
    let line = loop {
        looks += 1;
        assert!(looks <= 20, "never told money was dropped");
        clock.advance(std::time::Duration::from_mins(1));
        let Ok(heard) =
            tokio::time::timeout(std::time::Duration::from_secs(2), next(&mut top)).await
        else {
            continue;
        };
        if let Event::Text { body } = &heard.message.event
            && body.contains("deleted from origin without landing")
        {
            assert!(heard.why.is_some(), "{heard:?}");
            break body.clone();
        }
    };
    assert!(
        line.starts_with("cc-top: money, which your work is stacked on"),
        "{line}"
    );
    assert!(line.contains("don't land it yourself"), "{line}");
    let Response::Stacks { stacks } = machine.request(stack(None, None)).await else {
        panic!("its stacks");
    };
    assert!(
        stacks.iter().any(|s| s.branch == "money" && s.dropped),
        "{stacks:?}"
    );
}

/// A session that says what it's stacked on only once that branch is gone from origin, as
/// `zirc stack` finds it from its clone's stale ref, is told it was dropped all the same: in
/// the agent evals' PL2 probe (17 Nov 2026) such a session waited the run out.
#[tokio::test]
async fn a_session_that_says_its_stack_once_its_base_is_gone_is_told_too() {
    let ergo = require_ergo!();
    let clock = zircd::Clock::default();
    let hooks = zircd::Hooks {
        clock: clock.clone(),
        ..zircd::Hooks::default()
    };
    let machine = Machine::hooked(&ergo, "alpha", hooks).await;
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    let clone = clone();
    let root = clone.path();
    git(
        root,
        &[
            "remote",
            "set-url",
            "origin",
            remote.path().to_str().unwrap(),
        ],
    );
    git(root, &["push", "--quiet", "origin", "main"]);
    git(root, &["switch", "--quiet", "-c", "money"]);
    std::fs::write(root.join("money.py"), "def format_cents(c): ...\n").unwrap();
    git(root, &["add", "money.py"]);
    git(root, &["commit", "--quiet", "--message", "Money"]);
    git(root, &["push", "--quiet", "origin", "money"]);
    let tip = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "money"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    machine
        .request(Request::RepoAdd {
            path: root.to_owned(),
            session: Some("cc-top".into()),
        })
        .await;
    let mut top = machine.watch(Some("cc-top")).await;
    let stack = |branch: Option<&str>, tip: Option<&str>| Request::Stack {
        session: "cc-top".into(),
        branch: branch.map(str::to_owned),
        tip: tip.map(str::to_owned),
        drop: false,
        gone: true,
    };
    // Dropped before the session says its stack: `zirc stack`, which asks origin for it, says so.
    git(remote.path(), &["branch", "-D", "money"]);
    machine
        .request(stack(Some("money"), Some(tip.trim())))
        .await;
    let mut looks = 0;
    let line = loop {
        looks += 1;
        assert!(looks <= 20, "never told money was dropped");
        clock.advance(std::time::Duration::from_mins(1));
        let Ok(heard) =
            tokio::time::timeout(std::time::Duration::from_secs(2), next(&mut top)).await
        else {
            continue;
        };
        if let Event::Text { body } = &heard.message.event
            && body.contains("deleted from origin without landing")
        {
            assert!(heard.why.is_some(), "{heard:?}");
            break body.clone();
        }
    };
    assert!(
        line.starts_with("cc-top: money, which your work is stacked on"),
        "{line}"
    );
    assert!(line.contains("don't land it yourself"), "{line}");
    let Response::Stacks { stacks } = machine.request(stack(None, None)).await else {
        panic!("its stacks");
    };
    assert!(
        stacks.iter().any(|s| s.branch == "money" && s.dropped),
        "{stacks:?}"
    );
}

/// A session that pushes a branch built on another, not landed, and never says `zirc stack`
/// is stacked on it all the same, by what its push shows, so it's told when that base is
/// dropped: in the agent evals' PL2 probe (17 Nov 2026) one that hadn't said waited for good.
#[tokio::test]
async fn a_session_that_pushed_on_a_base_and_never_said_so_is_told_its_dropped() {
    let ergo = require_ergo!();
    let clock = zircd::Clock::default();
    let hooks = zircd::Hooks {
        clock: clock.clone(),
        ..zircd::Hooks::default()
    };
    let machine = Machine::hooked(&ergo, "alpha", hooks).await;
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    let clone = clone();
    let root = clone.path();
    git(
        root,
        &[
            "remote",
            "set-url",
            "origin",
            remote.path().to_str().unwrap(),
        ],
    );
    git(root, &["push", "--quiet", "origin", "main"]);
    git(root, &["switch", "--quiet", "-c", "money"]);
    std::fs::write(root.join("money.py"), "def format_cents(c): ...\n").unwrap();
    git(root, &["add", "money.py"]);
    git(root, &["commit", "--quiet", "--message", "Money"]);
    git(root, &["push", "--quiet", "origin", "money"]);
    let tip = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "money"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    machine
        .request(Request::RepoAdd {
            path: root.to_owned(),
            session: Some("cc-top".into()),
        })
        .await;
    let mut top = machine.watch(Some("cc-top")).await;
    let stack = |branch: Option<&str>, tip: Option<&str>| Request::Stack {
        session: "cc-top".into(),
        branch: branch.map(str::to_owned),
        tip: tip.map(str::to_owned),
        drop: false,
        gone: false,
    };
    // Built on money and pushed, with no word of a stack.
    git(root, &["switch", "--quiet", "-c", "receipt"]);
    std::fs::write(root.join("receipt.py"), "from money import format_cents\n").unwrap();
    git(root, &["add", "receipt.py"]);
    git(root, &["commit", "--quiet", "--message", "Receipt"]);
    git(root, &["push", "--quiet", "origin", "receipt"]);
    let mut noted = false;
    for _ in 0..50 {
        let Response::Stacks { stacks } = machine.request(stack(None, None)).await else {
            panic!("its stacks");
        };
        if stacks
            .iter()
            .any(|s| s.branch == "money" && s.tip == tip.trim())
        {
            noted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(noted, "its push never showed its stack on money");
    git(root, &["push", "--quiet", "origin", "--delete", "money"]);
    let mut looks = 0;
    let line = loop {
        looks += 1;
        assert!(looks <= 20, "never told money was dropped");
        clock.advance(std::time::Duration::from_mins(1));
        let Ok(heard) =
            tokio::time::timeout(std::time::Duration::from_secs(2), next(&mut top)).await
        else {
            continue;
        };
        if let Event::Text { body } = &heard.message.event
            && body.contains("deleted from origin without landing")
        {
            assert!(heard.why.is_some(), "{heard:?}");
            break body.clone();
        }
    };
    assert!(
        line.starts_with("cc-top: money, which your work is stacked on"),
        "{line}"
    );
    assert!(line.contains("don't land it yourself"), "{line}");
    let Response::Stacks { stacks } = machine.request(stack(None, None)).await else {
        panic!("its stacks");
    };
    assert!(
        stacks.iter().any(|s| s.branch == "money" && s.dropped),
        "{stacks:?}"
    );
}

/// With no CI saying anything, a default branch that moved on origin under a session's files
/// is said by zircd itself once the repository has it, as after anyone's fetch there: what
/// moved, which of the session's files it changed, and the rebase. zircd asks origin nothing
/// and fetches nothing itself. A merge made on the Git host, or anyone's CI not wired to
/// zirc, said nothing before.
#[tokio::test]
async fn a_default_branch_that_moves_with_no_ci_is_said_to_the_session_it_changes() {
    let ergo = require_ergo!();
    let clock = zircd::Clock::default();
    let hooks = zircd::Hooks {
        clock: clock.clone(),
        ..zircd::Hooks::default()
    };
    let machine = Machine::hooked(&ergo, "alpha", hooks).await;
    let remote = tempfile::tempdir().unwrap();
    git(
        remote.path(),
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    let clone = clone();
    let root = clone.path();
    let origin = remote.path().to_str().unwrap();
    git(root, &["remote", "set-url", "origin", origin]);
    std::fs::write(root.join("money.py"), "def format_cents(c): ...\n").unwrap();
    git(root, &["add", "money.py"]);
    git(root, &["commit", "--quiet", "--message", "Money"]);
    git(root, &["push", "--quiet", "origin", "main"]);
    machine
        .request(Request::RepoAdd {
            path: root.to_owned(),
            session: Some("cc-top".into()),
        })
        .await;
    let mut top = machine.watch(Some("cc-top")).await;
    // The session's own work in the file, not committed yet.
    std::fs::write(root.join("money.py"), "def format_cents(c): return c\n").unwrap();
    machine
        .request(Request::Intend {
            repo: root.to_owned(),
            session: "cc-top".into(),
            summary: "money".into(),
            paths: vec!["money.py".into()],
            seconds: 3600,
        })
        .await;
    // Someone else's change to the same file reaches main, and no CI says so.
    let other = tempfile::tempdir().unwrap();
    git(other.path(), &["clone", "--quiet", origin, "."]);
    git(other.path(), &["config", "user.email", "bo@example.com"]);
    git(other.path(), &["config", "user.name", "Bo"]);
    std::fs::write(other.path().join("money.py"), "def format_money(c): ...\n").unwrap();
    git(
        other.path(),
        &[
            "commit",
            "--quiet",
            "--all",
            "--message",
            "Amounts say money",
        ],
    );
    git(other.path(), &["push", "--quiet", "origin", "main"]);

    // The repository here learns of it when someone fetches, and zircd reads it there.
    git(root, &["fetch", "--quiet", "origin"]);
    let mut looks = 0;
    let line = loop {
        looks += 1;
        assert!(looks <= 20, "never told main moved");
        clock.advance(std::time::Duration::from_secs(5));
        let Ok(heard) =
            tokio::time::timeout(std::time::Duration::from_secs(3), next(&mut top)).await
        else {
            continue;
        };
        if let Event::Text { body } = &heard.message.event
            && body.contains("moved on origin")
        {
            break body.clone();
        }
    };
    assert!(
        line.starts_with("cc-top: main moved on origin to "),
        "{line}"
    );
    assert!(
        line.contains("the newest is \"Amounts say money\""),
        "{line}"
    );
    assert!(
        line.contains("changing files you're changing: money.py"),
        "{line}"
    );
    assert!(line.contains("(`git rebase origin/main`)"), "{line}");
}

/// A push of a branch built on another session's, not landed, wakes that session, so it doesn't
/// rewrite its own from under the other, nor wait on it: the agent evals' S3 deadlock.
#[tokio::test]
async fn a_branch_pushed_on_another_sessions_unlanded_one_wakes_that_session() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    let origin = remote.path().to_str().unwrap();
    git(&main_root, &["remote", "set-url", "origin", origin]);
    std::fs::write(main_root.join("lines.txt"), ten_lines(0, "")).unwrap();
    git(&main_root, &["add", "lines.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Lines"]);
    git(&main_root, &["push", "--quiet", "origin", "main"]);
    // The login session pushes its branch, which doesn't land.
    git(&login_root, &["merge", "--quiet", "--ff-only", "main"]);
    std::fs::write(login_root.join("login.txt"), "login\n").unwrap();
    git(&login_root, &["add", "login.txt"]);
    git(&login_root, &["commit", "--quiet", "--message", "Login"]);
    git(&login_root, &["push", "--quiet", "origin", "login"]);
    let mut login_hears = machine.watch(Some("cc-login")).await;

    // The main session builds on it, and pushes.
    git(&main_root, &["fetch", "--quiet", "origin"]);
    git(
        &main_root,
        &["checkout", "--quiet", "-b", "receipt", "origin/login"],
    );
    std::fs::write(main_root.join("receipt.txt"), "receipt\n").unwrap();
    git(&main_root, &["add", "receipt.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Receipt"]);
    git(&main_root, &["push", "--quiet", "origin", "receipt"]);
    let (event, why) = loop {
        let (event, why) = next_concerning(&mut login_hears).await;
        if matches!(&event, Event::Push { branch, .. } if branch == "receipt") {
            break (event, why);
        }
    };
    assert!(
        matches!(&event, Event::Push { on: Some(on), .. } if on == "login"),
        "{event:?}"
    );
    assert_eq!(why, Why::StackedOn);
}

/// A session whose branch takes out what someone else landed, as a conflict's resolution does
/// when it keeps only its own side, is told so: the evals' S4, where the tests still passed.
#[tokio::test]
async fn a_branch_taking_out_anothers_landing_is_told() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    git(
        &main_root,
        &[
            "remote",
            "set-url",
            "origin",
            remote.path().to_str().unwrap(),
        ],
    );
    std::fs::write(
        main_root.join("report.py"),
        "def summary(n):\n    return f\"{n} orders in all\"\n",
    )
    .unwrap();
    git(&main_root, &["add", "report.py"]);
    git(&main_root, &["commit", "--quiet", "--message", "Report"]);
    // Carol lands new words for the line on main.
    std::fs::write(
        main_root.join("report.py"),
        "def summary(n):\n    return f\"orders: {n}, total so far\"\n",
    )
    .unwrap();
    git(&main_root, &["add", "report.py"]);
    git(
        &main_root,
        &[
            "-c",
            "user.name=Carol",
            "-c",
            "user.email=carol@example.com",
            "commit",
            "--quiet",
            "--message",
            "The summary names its numbers",
        ],
    );
    git(&main_root, &["push", "--quiet", "origin", "main"]);
    git(&login_root, &["fetch", "--quiet", "origin"]);
    git(&login_root, &["reset", "--quiet", "--hard", "origin/main"]);
    let mut login_hears = machine.watch(Some("cc-login")).await;

    // The login session puts its own words back over hers, and commits.
    std::fs::write(
        login_root.join("report.py"),
        "def summary(n):\n    return f\"{n} orders, {n * 2} in all\"\n",
    )
    .unwrap();
    git(
        &login_root,
        &[
            "commit",
            "--quiet",
            "--all",
            "--message",
            "Money in the summary",
        ],
    );
    let told = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let delivery = next(&mut login_hears).await;
            if let Event::Text { body } = &delivery.message.event
                && body.contains("takes out")
            {
                return body.clone();
            }
        }
    })
    .await
    .expect("told its branch takes out Carol's landing");
    assert!(
        told.starts_with("cc-login: your branch takes out what Carol landed in"),
        "{told}"
    );
    assert!(told.contains("The summary names its numbers"), "{told}");
}
