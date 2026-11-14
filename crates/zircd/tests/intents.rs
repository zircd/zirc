//! What a session says it will change, and the working sets zircd tells.

use zirc_core::ipc::{self, Heard, Request, Response, Target, Watching, Why};
use zirc_proto::Event;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn an_intent_wakes_the_sessions_changing_what_it_names() {
    let fleet = Fleet::with_sessions(require_ergo!()).await;
    fleet.beta.edits("web/login.ts", "export {}\n");
    fleet.beta.sees_changes_by("beta").await;
    let mut watching = fleet.beta.watch().await;

    let told = fleet.alpha.intends("split the login page", &["web"]).await;

    // An intent is agents' news: in the agents' room, counted as the repository's.
    assert!(matches!(told, Response::Channel { channel, .. } if channel == "#api/agents"));
    let delivery = next_intent(&mut watching).await;
    assert_eq!(
        (delivery.from.as_str(), delivery.why),
        ("alpha", Some(Why::Intent))
    );
    let Event::Intent { paths, expires, .. } = delivery.message.event else {
        unreachable!()
    };
    assert_eq!(paths, ["web"]);
    assert!(expires.is_some());
}

#[tokio::test]
async fn changing_files_is_an_intent_for_their_directory_that_every_machine_lists() {
    let fleet = Fleet::with_sessions(require_ergo!()).await;

    fleet.beta.edits("web/login.ts", "export {}\n");

    // Changing files in web/, cc-b has an intent for it without saying so, which alpha
    // hears too.
    fleet
        .alpha
        .sees_intent_of("cc-b", |paths| paths == ["web"])
        .await;
}

#[tokio::test]
async fn an_intent_is_listed_by_every_machine_until_its_session_says_done() {
    let fleet = Fleet::with_sessions(require_ergo!()).await;
    fleet.alpha.intends("split the login page", &["web"]).await;
    fleet.beta.sees_intent_of("cc-a", |_| true).await;

    fleet.alpha.says("done: the login page").await;

    fleet.beta.sees_no_intent_of("cc-a").await;
}

#[tokio::test]
async fn an_automatic_intent_is_withdrawn_once_its_changes_are_gone() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    beta.request(Request::RepoAdd {
        path: beta_clone.path().to_owned(),
        session: Some("cc-b".into()),
    })
    .await;
    std::fs::create_dir(beta_clone.path().join("web")).unwrap();
    std::fs::write(beta_clone.path().join("web/login.ts"), "export {}\n").unwrap();
    let has_intent = |channels: &[ipc::ChannelPresence]| {
        channels
            .iter()
            .any(|c| c.intents.iter().any(|i| i.session == "cc-b"))
    };
    until_who(&alpha, has_intent, "cc-b's automatic intent").await;
    // Its changes gone, the intent zircd said for them is withdrawn at once, everywhere,
    // not after its half hour.
    std::fs::remove_dir_all(beta_clone.path().join("web")).unwrap();
    until_who(
        &alpha,
        |channels| !has_intent(channels),
        "cc-b's automatic intent to be withdrawn",
    )
    .await;
}

#[tokio::test]
async fn an_automatic_intent_is_withdrawn_when_its_checkout_is_removed() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    // Another checkout keeps beta in the repository's channels once the first is gone.
    let staying = clone();
    for (path, session) in [(beta_clone.path(), "cc-b"), (staying.path(), "cc-c")] {
        beta.request(Request::RepoAdd {
            path: path.to_owned(),
            session: Some(session.into()),
        })
        .await;
    }
    std::fs::create_dir(beta_clone.path().join("web")).unwrap();
    std::fs::write(beta_clone.path().join("web/login.ts"), "export {}\n").unwrap();
    let has_intent = |channels: &[ipc::ChannelPresence]| {
        channels
            .iter()
            .any(|c| c.intents.iter().any(|i| i.session == "cc-b"))
    };
    until_who(&alpha, has_intent, "cc-b's automatic intent").await;
    // A temporary worktree removed with its changes: the intent is withdrawn in the room it
    // was said in, not the repository's channel, so it ends. zircd's own intents are debug,
    // live and out of history, so it's seen ending, not read back.
    std::fs::remove_dir_all(beta_clone.path()).unwrap();
    until_who(
        &alpha,
        |channels| !has_intent(channels),
        "no intent of cc-b's",
    )
    .await;
}

#[tokio::test]
async fn a_report_waits_for_the_sessions_next_wake() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let post = |text: &str| Request::Post {
        repo: beta_clone.path().to_owned(),
        session: "cc-9".into(),
        text: text.into(),
        to: Target::Repo,
        app: None,
        everyone: false,
        force: true,
        reply: None,
    };
    // It names cc-1, but needs nothing of it: no wake.
    beta.request(post("cc-1: login landed as 1a2b3c4 (build 8)"))
        .await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("cc-1: login landed as 1a2b3c4 (build 8)".into(), None)
    );
    // A question does wake it, and the report comes first, introduced.
    beta.request(post("cc-1: can you rebase onto it?")).await;
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
        (
            "cc-1: login landed as 1a2b3c4 (build 8)".into(),
            Some(Why::Mention)
        )
    );
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("cc-1: can you rebase onto it?".into(), Some(Why::Mention))
    );
}

#[tokio::test]
async fn the_same_post_again_at_once_goes_out_once() {
    let ergo = require_ergo!();
    let [(_alpha, _), (beta, beta_clone)] = two_machines(&ergo).await;
    let post = |text: &str| Request::Post {
        repo: beta_clone.path().to_owned(),
        session: "cc-9".into(),
        text: text.into(),
        to: Target::Repo,
        app: None,
        everyone: false,
        force: true,
        reply: None,
    };
    let first = beta.request(post("done: the login page")).await;
    assert!(!matches!(first, Response::Error { .. }), "{first:?}");
    let again = beta.try_request(post("done: the login page")).await;
    assert!(
        again
            .as_ref()
            .is_err_and(|err| err.to_string().contains("not posted: cc-9 posted the same")),
        "{again:?}"
    );
    let other = beta.request(post("taking: the sign-up page")).await;
    assert!(!matches!(other, Response::Error { .. }), "{other:?}");
}

#[tokio::test]
async fn working_sets_are_told_when_they_change_and_not_more_often() {
    let ergo = require_ergo!();
    let alpha = Machine::start_with(&ergo, "alpha", "announce_every = 4\n").await;
    let beta = Machine::start(&ergo, "beta").await;
    let (alpha_clone, beta_clone) = (clone(), clone());
    for (machine, clone) in [(&alpha, &alpha_clone), (&beta, &beta_clone)] {
        machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: None,
            })
            .await;
        // Every channel, not only #api: one joining later is told alpha's set at once, as
        // anyone joining is, which read as alpha telling it again too soon (build 1667).
        machine
            .eventually(|status| status.channels == JOINED, "joining")
            .await;
    }
    let mut beta_hears = beta.watch(None).await;
    // Alpha's sets in #api, the channel whose announcements are counted, and when the
    // server got each, in milliseconds: its clock, not when beta happened to hear them,
    // which a busy machine delays (builds 1716 and 1784).
    let from_alpha = async |watching: &mut Watching| {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Heard::Message(delivery) = heard(watching).await
                    && delivery.from == "alpha"
                    && delivery.channel == "#api"
                    && let Event::Working { paths, .. } = delivery.message.event
                {
                    let at = delivery
                        .time
                        .as_deref()
                        .and_then(zirc_core::time::millis_at);
                    return (paths, at.expect("the server's time on the line"));
                }
            }
        })
        .await
        .expect("alpha's working set")
    };

    // The first change is told at once.
    std::fs::write(alpha_clone.path().join("a.rs"), "// one\n").unwrap();
    let (first, told) = from_alpha(&mut beta_hears).await;
    assert_eq!(first, ["a.rs"]);
    // The same set again is nothing to tell; a new one is told in its turn, after the
    // first. How long its turn is, zircd's own clock says
    // (daemon.rs: a_channel_hears_whole_sets_no_sooner_than_announce_every_apart): the
    // server's is no measure of it, as a busy machine's send queue can hold the first line
    // back by seconds (builds 1667, 1716, 1784 and 1905).
    std::fs::write(alpha_clone.path().join("a.rs"), "// two\n").unwrap();
    std::fs::write(alpha_clone.path().join("b.rs"), "// three\n").unwrap();
    let (next, again) = from_alpha(&mut beta_hears).await;
    assert_eq!(next, ["a.rs", "b.rs"]);
    assert!(again >= told, "told the new set before the first");
}

#[tokio::test]
async fn a_renewal_keeps_an_intent_listed_without_a_line_or_a_wake() {
    let fleet = Fleet::with_sessions(require_ergo!()).await;
    fleet.beta.edits("web/login.ts", "export {}\n");
    fleet.beta.sees_changes_by("beta").await;
    let mut watching = fleet.beta.watch().await;

    // An intent renewed, as zircd renews the ones it says for a session by itself.
    let intent = Event::Intent {
        summary: "changing files here (said by zircd)".into(),
        paths: vec!["web".into()],
        expires: None,
        lasts: Some(600),
    };
    let Some(Event::Custom { kind, data, text }) = zirc_proto::renewal(&intent) else {
        unreachable!()
    };
    fleet
        .alpha
        .machine
        .request(Request::Emit {
            repo: fleet.alpha.clone.path().to_owned(),
            session: "cc-a".into(),
            kind,
            data,
            text,
            // The agents' room, where intents go.
            to: Target::Topic {
                name: "agents".into(),
                org: false,
            },
            app: None,
        })
        .await;

    // Every machine lists it, as if it were said again.
    fleet
        .beta
        .sees_intent_of("cc-a", |paths| paths == ["web"])
        .await;
    // But beta's watch heard no line of it: the next intent it hears is the one said
    // next, and, the renewal having told it already, that's no news to wake it for.
    fleet.alpha.intends("split the login page", &["web"]).await;
    let delivery = next_intent(&mut watching).await;
    let Event::Intent { summary, .. } = &delivery.message.event else {
        unreachable!()
    };
    assert_eq!(summary, "split the login page");
    assert_eq!(delivery.why, None);
}

#[tokio::test]
async fn a_session_starting_to_change_files_is_told_the_gotchas_kept_about_them() {
    let fleet = Fleet::with_sessions(require_ergo!()).await;
    let mut watching = fleet.beta.watch().await;
    let kept = fleet
        .alpha
        .machine
        .request(Request::Emit {
            repo: fleet.alpha.clone.path().to_owned(),
            session: "cc-a".into(),
            kind: zirc_core::gotchas::KIND.into(),
            data: serde_json::json!({ "paths": ["web"] }),
            text: "the login page only builds with pnpm".into(),
            to: Target::Topic {
                name: zirc_core::gotchas::ROOM.into(),
                org: false,
            },
            app: None,
        })
        .await;
    assert!(matches!(kept, Response::Channel { channel, .. } if channel == "#api/gotchas"));
    // Beta hears it, and it wakes nobody.
    loop {
        let delivery = next(&mut watching).await;
        if zirc_core::gotchas::of(&delivery).is_some() {
            assert_eq!(delivery.why, None);
            break;
        }
    }

    fleet.beta.edits("web/login.ts", "export {}\n");

    let (body, why) = next_post_why(&mut watching).await;
    assert!(
        body.starts_with("cc-b: you've started changing web, where gotchas others learnt here"),
        "{body}"
    );
    assert!(
        body.contains("cc-a: the login page only builds with pnpm (web)"),
        "{body}"
    );
    assert_eq!(why, Some(Why::Mention));
}

#[tokio::test]
async fn the_gotchas_told_one_session_dont_wake_the_session_that_learnt_them() {
    let ergo = require_ergo!();
    let machine = Machine::start(&ergo, "alpha").await;
    let (theirs, mine) = (clone(), clone());
    for (clone, session) in [(&theirs, "cc-a"), (&mine, "cc-b")] {
        machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
    }
    machine
        .eventually(
            |status| status.channels.iter().any(|c| c == "#api"),
            "joining",
        )
        .await;
    // cc-a keeps a gotcha about web/ on this machine, and watches on.
    let kept = machine
        .request(Request::Emit {
            repo: theirs.path().to_owned(),
            session: "cc-a".into(),
            kind: zirc_core::gotchas::KIND.into(),
            data: serde_json::json!({ "paths": ["web"] }),
            text: "the login page only builds with pnpm".into(),
            to: Target::Topic {
                name: zirc_core::gotchas::ROOM.into(),
                org: false,
            },
            app: None,
        })
        .await;
    assert!(matches!(kept, Response::Channel { .. }), "{kept:?}");
    let mut author = machine.watch(Some("cc-a")).await;
    let mut told = machine.watch(Some("cc-b")).await;
    // cc-b starts changing web/: it's told the gotcha, which names cc-a as who learnt it.
    let file = mine.path().join("web/login.ts");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, "export {}\n").unwrap();
    let (body, why) = next_post_why(&mut told).await;
    assert!(
        body.starts_with("cc-b: you've started changing web"),
        "{body}"
    );
    assert!(
        body.contains("cc-a: the login page only builds with pnpm"),
        "{body}"
    );
    assert_eq!(why, Some(Why::Mention));
    // It's cc-b's news, not cc-a's: nothing wakes cc-a before a line that names it.
    post(&machine, mine.path(), "cc-b", "cc-a: marker").await;
    let heard = until_marker(&mut author, |heard| says(heard, "marker")).await;
    let woke: Vec<&str> = heard
        .iter()
        .filter_map(|heard| match heard {
            Heard::Message(delivery) if delivery.why.is_some() => match &delivery.message.event {
                Event::Text { body } if !body.contains("marker") => Some(body.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(woke, Vec::<&str>::new());
}

/// The intents among `heard` that woke the session watching, by what they said.
fn intent_wakes(heard: &[Heard]) -> Vec<String> {
    heard
        .iter()
        .filter_map(|heard| match heard {
            Heard::Message(delivery) if delivery.why.is_some() => match &delivery.message.event {
                Event::Intent { summary, .. } => Some(format!("{summary} ({:?})", delivery.why)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_automatic_intent_said_again_for_the_same_directory_wakes_a_session_once() {
    let fleet = Fleet::with_sessions(require_ergo!()).await;
    fleet.beta.edits("web/login.ts", "export {}\n");
    fleet.beta.sees_changes_by("beta").await;
    let mut watching = fleet.beta.watch().await;
    // cc-a starts changing web/: its automatic intent for it wakes cc-b, changing a file there.
    fleet.alpha.edits("web/cart.ts", "export {}\n");
    let first = next_intent(&mut watching).await;
    assert_eq!(first.why, Some(Why::Intent));
    // Committed, its changes are gone and the intent withdrawn; then it changes web/ again,
    // and zircd says the same intent again: nothing new to cc-b.
    std::fs::remove_file(fleet.alpha.clone.path().join("web/cart.ts")).unwrap();
    fleet.beta.sees_no_intent_of("cc-a").await;
    fleet.alpha.edits("web/cart.ts", "export const total = 0\n");
    fleet
        .beta
        .sees_intent_of("cc-a", |paths| paths == ["web"])
        .await;
    fleet.alpha.says("cc-b: marker").await;
    let heard = until_marker(&mut watching, |heard| says(heard, "marker")).await;
    assert_eq!(intent_wakes(&heard), Vec::<String>::new());
}

#[tokio::test]
async fn on_one_machine_an_automatic_intent_said_again_wakes_another_session_once() {
    let ergo = require_ergo!();
    let machine = Machine::start(&ergo, "alpha").await;
    let (theirs, mine) = (clone(), clone());
    for (clone, session) in [(&theirs, "cc-a"), (&mine, "cc-b")] {
        machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
    }
    machine
        .eventually(
            |status| status.channels.iter().any(|c| c == "#api"),
            "joining",
        )
        .await;
    let edit = |clone: &tempfile::TempDir, path: &str, content: &str| {
        let file = clone.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    };
    // cc-b says it'll change a file in web/, as a session does with `zirc intend`.
    machine
        .request(Request::Intend {
            repo: mine.path().to_owned(),
            session: "cc-b".into(),
            summary: "the login page".into(),
            paths: vec!["web/login.ts".into()],
            seconds: 3600,
        })
        .await;
    let mut watching = machine.watch(Some("cc-b")).await;
    let lists = |session: &'static str, present: bool| {
        move |channels: &[ipc::ChannelPresence]| {
            channels
                .iter()
                .any(|c| c.intents.iter().any(|i| i.session == session))
                == present
        }
    };
    edit(&theirs, "web/cart.ts", "export {}\n");
    let first = next_intent(&mut watching).await;
    assert_eq!(first.why, Some(Why::Intent));
    std::fs::remove_file(theirs.path().join("web/cart.ts")).unwrap();
    until_who(&machine, lists("cc-a", false), "cc-a's intent withdrawn").await;
    edit(&theirs, "web/cart.ts", "export const total = 0\n");
    until_who(&machine, lists("cc-a", true), "cc-a's intent again").await;
    post(&machine, theirs.path(), "cc-a", "cc-b: marker").await;
    let heard = until_marker(&mut watching, |heard| says(heard, "marker")).await;
    assert_eq!(intent_wakes(&heard), Vec::<String>::new());
}

#[tokio::test]
async fn an_intent_says_how_it_meets_the_work_of_the_session_it_wakes() {
    let ergo = require_ergo!();
    let machine = Machine::start(&ergo, "alpha").await;
    let (theirs, mine) = (clone(), clone());
    for (clone, session) in [(&theirs, "cc-a"), (&mine, "cc-b")] {
        machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
    }
    machine
        .eventually(
            |status| status.channels.iter().any(|c| c == "#api"),
            "joining",
        )
        .await;
    let edit = |clone: &tempfile::TempDir, path: &str| {
        let file = clone.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "export {}\n").unwrap();
    };
    edit(&mine, "web/login.ts");
    until_who(
        &machine,
        |channels| {
            channels.iter().any(|c| {
                c.members
                    .iter()
                    .any(|m| m.session.as_deref() == Some("cc-b") && !m.paths.is_empty())
            })
        },
        "cc-b's changes",
    )
    .await;
    let mut watching = machine.watch(Some("cc-b")).await;

    // cc-a changes another file in web/: its automatic intent for web/ wakes cc-b, saying
    // whose files are whose, and that they're different ones.
    edit(&theirs, "web/cart.ts");

    let delivery = next_intent(&mut watching).await;
    assert_eq!(delivery.why, Some(Why::Intent));
    let fit = delivery.fit.expect("the intent's fit");
    assert_eq!(fit.theirs, ["web/cart.ts"]);
    assert_eq!(fit.mine, ["web/login.ts"]);
    assert_eq!(fit.verdict(), zirc_core::fit::Verdict::DifferentFiles);
    let Event::Intent { summary, .. } = &delivery.message.event else {
        unreachable!()
    };
    assert!(summary.starts_with("changing web/cart.ts"), "{summary}");
}
