//! Sessions as the apps show them: names, cards, direct messages, typing and reactions.

use std::path::PathBuf;
use std::time::Duration;
use zirc_core::ipc::{self, Heard, Request, Response, Why};
use zirc_proto::Event;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn a_task_runs_from_taking_to_done_on_what_its_session_says() {
    async fn task_of(machine: &Machine, landing: bool) -> Option<String> {
        match machine
            .request(Request::Task {
                session: "cc-t".into(),
                landing,
            })
            .await
        {
            Response::Task { task } => task,
            other => panic!("{other:?}"),
        }
    }
    async fn task_now(machine: &Machine) -> Option<String> {
        task_of(machine, false).await
    }
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-t".into()),
        })
        .await;
    let mut beta_hears = beta.watch(None).await;
    let repo = alpha_clone.path();
    assert_eq!(task_now(&alpha).await, None, "no task before taking:");
    post(&alpha, repo, "cc-t", "nothing yet").await;
    post(&alpha, repo, "cc-t", "taking: the recorder").await;
    let task = task_now(&alpha).await.expect("taking: starts a task");
    assert_eq!(zirc_proto::task_id(&task), Some(task.clone()));
    post(&alpha, repo, "cc-t", "halfway").await;
    post(&alpha, repo, "cc-t", "done: the recorder").await;
    assert_eq!(task_now(&alpha).await, None, "done: ends it");
    post(&alpha, repo, "cc-t", "after").await;
    assert_eq!(
        task_of(&alpha, true).await.as_ref(),
        Some(&task),
        "a landing after done: claims it"
    );
    let next = task_of(&alpha, true)
        .await
        .expect("every landing has a task");
    assert_ne!(next, task, "the next landing's is its own");
    let mut tasks = Vec::new();
    for _ in 0..5 {
        tasks.push(next_post(&mut beta_hears).await.message.task);
    }
    let on = Some(task);
    assert_eq!(tasks, [None, on.clone(), on.clone(), on, None]);
}

#[tokio::test]
async fn a_landing_landing_finishes_the_task_it_was_queued_under() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-t".into()),
        })
        .await;
    let land = async || match alpha
        .request(Request::Task {
            session: "cc-t".into(),
            landing: true,
        })
        .await
    {
        Response::Task { task } => task,
        other => panic!("{other:?}"),
    };
    let (ci, mut ci_in) = zirc_test_irc::integration(&ergo, "zirc-alerts").await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#agents").await;
    let landed = |build: u32| {
        zirc_proto::encode(&ci_message(Event::Custom {
            kind: "x-zirc/landing".into(),
            data: serde_json::json!({ "session": "cc-t", "branch": "login" }),
            text: format!("cc-t: landed login as 1a2b3c4 (build {build})"),
        }))
        .unwrap()
    };
    let mut beta_hears = beta.watch(None).await;
    let repo = alpha_clone.path();
    post(&alpha, repo, "cc-t", "taking: the recorder").await;
    let task = land().await.expect("its landing claims the task it's on");
    ci.privmsg("#agents", &landed(7)).unwrap();
    // zircd says done: for it, where its taking: went, as part of that task.
    let done = loop {
        let delivery = next_post(&mut beta_hears).await;
        if let Event::Text { body } = &delivery.message.event
            && body.starts_with("done:")
        {
            break delivery;
        }
    };
    assert_eq!(done.message.session, "cc-t");
    assert_eq!(done.message.task.as_ref(), Some(&task));
    let Event::Text { body } = &done.message.event else {
        unreachable!()
    };
    assert_eq!(body, "done: landed (said by zircd)");
    // A task taken on after the landing was queued is for what comes next: it runs on.
    post(&alpha, repo, "cc-t", "taking: the player").await;
    land().await;
    post(&alpha, repo, "cc-t", "taking: the next thing").await;
    ci.privmsg("#agents", &landed(8)).unwrap();
    post(&alpha, repo, "cc-t", "marker").await;
    loop {
        let delivery = next_post(&mut beta_hears).await;
        let Event::Text { body } = &delivery.message.event else {
            continue;
        };
        assert!(
            !body.starts_with("done:"),
            "ended a task taken on after: {body}"
        );
        if body == "marker" {
            break;
        }
    }
}

#[tokio::test]
async fn a_sessions_title_goes_on_its_card_and_its_link_stays_here() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    let mut beta_hears = beta.watch(None).await;
    let link = "claude://claude.ai/epitaxy/0123";
    let named = alpha
        .request(Request::Name {
            session: "cc-1".into(),
            name: Some("desktop".into()),
            tool: None,
            avatar: None,
            title: Some("Desktop:  3f review\nfixes".into()),
            link: Some(link.into()),
        })
        .await;
    assert_eq!(named, Response::Done);
    // The other machine hears the card, signed with the name, and nothing of the link.
    let delivery = loop {
        let delivery = next(&mut beta_hears).await;
        if zirc_proto::session::SessionCard::of(&delivery.message.event).is_some() {
            break delivery;
        }
    };
    let card = zirc_proto::session::SessionCard::of(&delivery.message.event).unwrap();
    assert_eq!(
        (
            card.name.as_deref(),
            card.title.as_str(),
            delivery.channel.as_str()
        ),
        (Some("desktop"), "Desktop: 3f review fixes", "#agents")
    );
    assert_eq!(delivery.message.name.as_deref(), Some("desktop"));
    assert!(
        !serde_json::to_string(&delivery)
            .unwrap()
            .contains("claude://")
    );
    // The owner's apps here get the link over the socket; the other machine has none.
    let Response::LocalSessions { sessions } = alpha.request(Request::LocalSessions).await else {
        panic!("not the local sessions")
    };
    assert_eq!(
        sessions,
        [zirc_core::ipc::LocalSession {
            session: "cc-1".into(),
            name: Some("desktop".into()),
            title: Some("Desktop: 3f review fixes".into()),
            link: Some(link.into()),
        }]
    );
    let Response::LocalSessions { sessions } = beta.request(Request::LocalSessions).await else {
        panic!("not the local sessions")
    };
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn a_session_with_no_name_goes_by_its_branch_until_it_names_itself() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    let switched = std::process::Command::new("git")
        .args(["switch", "-q", "-c", "mattias/login-page"])
        .current_dir(alpha_clone.path())
        .status()
        .unwrap();
    assert!(switched.success());
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-b".into()),
        })
        .await;
    // Once zircd has seen the branch, as a change on it shows.
    std::fs::write(alpha_clone.path().join("login.rs"), "fn login() {}\n").unwrap();
    until_own_sets(
        &alpha,
        "alpha",
        &[own_set("mattias/login-page", Some("cc-b"), &["login.rs"])],
        "the change on the branch",
    )
    .await;
    let mut beta_hears = beta.watch(None).await;
    post(&alpha, alpha_clone.path(), "cc-b", "taking: the login page").await;
    // The branch's last part, as a name.
    let delivery = next_post(&mut beta_hears).await;
    assert_eq!(
        (
            delivery.message.session.as_str(),
            delivery.message.name.as_deref()
        ),
        ("cc-b", Some("login-page"))
    );
    // A name it gives itself wins.
    let named = alpha
        .request(Request::Name {
            session: "cc-b".into(),
            name: Some("login".into()),
            tool: None,
            avatar: None,
            title: None,
            link: None,
        })
        .await;
    assert_eq!(named, Response::Done);
    post(&alpha, alpha_clone.path(), "cc-b", "halfway").await;
    let delivery = next_post(&mut beta_hears).await;
    assert_eq!(delivery.message.name.as_deref(), Some("login"));
}

#[tokio::test]
async fn a_session_goes_by_its_name_and_says_its_tool_and_avatar() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    for session in ["cc-1", "cc-2"] {
        alpha
            .request(Request::RepoAdd {
                path: alpha_clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
    }
    let named = alpha
        .request(Request::Name {
            session: "cc-1".into(),
            name: Some("Signup Web".into()),
            tool: Some("Claude Code".into()),
            avatar: Some("Rocket".into()),
            title: None,
            link: None,
        })
        .await;
    assert_eq!(named, Response::Done);
    let unknown = Request::Name {
        session: "cc-2".into(),
        name: None,
        tool: None,
        avatar: Some("unicorn".into()),
        title: None,
        link: None,
    };
    let refused = ipc::request(&alpha.socket(), &unknown).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("isn't a role: pick one of agent, reviewer"),
        "{refused}"
    );
    let mut beta_hears = beta.watch(None).await;
    post(
        &alpha,
        alpha_clone.path(),
        "cc-1",
        "taking: the signup page",
    )
    .await;
    post(&alpha, alpha_clone.path(), "cc-2", "taking: the login page").await;
    post(&alpha, alpha_clone.path(), "cc-3", "taking: nothing much").await;
    let mut heard = Vec::new();
    for _ in 0..3 {
        let delivery = next_post(&mut beta_hears).await;
        heard.push((
            delivery.message.session,
            delivery.message.name,
            delivery.message.tool,
            delivery.message.avatar,
        ));
    }
    assert_eq!(
        heard,
        [
            (
                "cc-1".to_owned(),
                Some("signup-web".to_owned()),
                Some("claude-code".to_owned()),
                Some("rocket".to_owned())
            ),
            ("cc-2".to_owned(), None, None, None),
            ("cc-3".to_owned(), None, None, None),
        ]
    );
}

#[tokio::test]
async fn a_session_hears_what_concerns_it_and_not_its_own_posts() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    // Session cc-1 works in alpha's clone, and watches as itself.
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;

    post(&alpha, alpha_clone.path(), "cc-1", "taking: the router").await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("taking: the router".into(), None),
        "its own post, back"
    );
    post(&beta, beta_clone.path(), "cc-2", "done: the docs").await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("done: the docs".into(), None),
        "chatter"
    );
    post(&beta, beta_clone.path(), "cc-2", "can you rebase, cc-1?").await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("can you rebase, cc-1?".into(), Some(Why::Mention))
    );
    std::fs::write(alpha_clone.path().join("router.ts"), "export {}\n").unwrap();
    // Once alpha's own working set has it.
    tokio::time::timeout(WAIT, async {
        loop {
            if let Response::Who { channels } = alpha.request(Request::Who).await
                && channels
                    .iter()
                    .flat_map(|channel| &channel.members)
                    .any(|member| {
                        member.worktree.is_some() && member.paths.contains(&"router.ts".to_owned())
                    })
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("alpha never noticed router.ts");
    // In #agents a post naming a path it changes isn't news to it: only one naming it is.
    let heads_up = "heads up, router.ts is changing";
    post(&beta, beta_clone.path(), "cc-2", heads_up).await;
    assert_eq!(next_post_why(&mut cc1).await, (heads_up.into(), None));
    let add = |session: &str, path: &std::path::Path| Request::RepoAdd {
        path: path.to_owned(),
        session: Some(session.into()),
    };
    // Added in another clone of the repository, cc-1 no longer works in the first.
    let other = clone();
    let Response::Repos { repos } = alpha.request(add("cc-1", other.path())).await else {
        panic!("not a repo list")
    };
    let first = std::fs::canonicalize(alpha_clone.path()).unwrap();
    let sessions_in = |path: &std::path::Path| {
        repos.iter().find(|repo| repo.path == path).map(|repo| {
            repo.sessions
                .iter()
                .map(|it| it.session.clone())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(sessions_in(&first), Some(Vec::new()));
}

#[tokio::test]
async fn a_session_no_repository_was_added_with_hears_only_what_names_it() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut stranger = machine.watch(Some("cc-stranger")).await;

    // Two other sessions overlap: news to them, not to it.
    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    let overlap = loop {
        if let Heard::LocalOverlap(overlap) = heard(&mut stranger).await {
            break overlap;
        }
    };
    assert_eq!(overlap.why, None);
    // What names it still does.
    post(
        &machine,
        &main_root,
        "cc-main",
        "which worktree are you in, cc-stranger?",
    )
    .await;
    assert_eq!(
        next_post_why(&mut stranger).await,
        (
            "which worktree are you in, cc-stranger?".to_owned(),
            Some(Why::Mention)
        )
    );
}

#[tokio::test]
async fn the_recorder_is_told_which_sessions_are_live_for_readers_in_no_channel() {
    let ergo = require_ergo!();
    let (_recorder, mut recorded) =
        zirc_test_irc::person(&ergo, zirc_proto::activity::RECORDER).await;
    let (machine, _main, _main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;

    let _agent = machine.watch(Some("cc-login")).await;

    let live = loop {
        let line =
            zirc_test_irc::next_matching(&mut recorded, |line| line.command == "TAGMSG").await;
        let message = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap();
        if let Some(live) = zirc_proto::session::Live::of(&message.event)
            && live.online("cc-login")
        {
            assert!(message.is_live(), "never in history");
            break live;
        }
    };
    assert!(live.away.is_empty());
}

#[tokio::test]
async fn a_wake_and_what_the_session_did_go_to_the_recorder_alone() {
    use zirc_proto::activity::{Acted, Record, Wake, WakeOutcome};
    let ergo = require_ergo!();
    let (machine, _main, _main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut agent = machine.watch(Some("cc-login")).await;
    let (_recorder, mut recorded) =
        zirc_test_irc::person(&ergo, zirc_proto::activity::RECORDER).await;
    let (ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_hears, "#agents").await;
    let next_record = async |recorded: &mut zirc_irc::Incoming| {
        let line = zirc_test_irc::next_matching(recorded, |line| line.command == "TAGMSG").await;
        assert_eq!(
            line.params[0],
            zirc_proto::activity::RECORDER,
            "to the recorder alone"
        );
        zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap()
    };

    ada.privmsg("#agents", "cc-login: can you look at the login page?")
        .unwrap();
    assert_eq!(next_post(&mut agent).await.why, Some(Why::Mention));
    let sent = next_record(&mut recorded).await;
    assert_eq!(sent.session, "cc-login");
    assert!(sent.is_live(), "never in history");
    let wake = Wake::of(&sent.event).expect("a wake");
    assert_eq!((wake.why.as_str(), wake.kind.as_str()), ("mention", "text"));
    assert!(wake.line.is_some(), "the line's id, not its words");

    post(&machine, &login_root, "cc-login", "on it").await;
    let outcome = WakeOutcome::of(&next_record(&mut recorded).await.event).expect("an outcome");
    assert_eq!(outcome.wake, wake.id);
    assert_eq!(outcome.acted, Acted::Posted);
    // Nobody else in the channel is sent either: by the machine's next post, which follows
    // the records on its one connection, any would have reached Ada.
    post(&machine, &login_root, "cc-login", "marker").await;
    loop {
        let line = zirc_test_irc::next_matching(&mut ada_hears, |_| true).await;
        if line.command == "PRIVMSG" && line.params.get(1).is_some_and(|b| b.contains("marker")) {
            break;
        }
        let is_record = line
            .tag(zirc_proto::EVENT_TAG)
            .and_then(|payload| zirc_proto::from_payload(payload).ok())
            .is_some_and(|message| {
                matches!(&message.event, Event::Custom { kind, .. }
                    if zirc_proto::activity::KINDS.contains(&kind.as_str()))
            });
        assert!(!is_record, "{line:?}");
    }
}

#[tokio::test]
async fn an_overlap_zircd_tells_of_goes_to_the_recorder_without_its_paths() {
    use zirc_proto::activity::{Notice, Noticed, Record};
    let ergo = require_ergo!();
    let (_machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let (_recorder, mut recorded) =
        zirc_test_irc::person(&ergo, zirc_proto::activity::RECORDER).await;

    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    let (sent, notice) = loop {
        let line =
            zirc_test_irc::next_matching(&mut recorded, |line| line.command == "TAGMSG").await;
        let sent = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap();
        if let Some(notice) = Notice::of(&sent.event) {
            break (sent, notice);
        }
    };
    assert!(sent.is_live(), "never in history");
    assert_eq!(notice.what, Noticed::Overlap);
    assert!(notice.local && notice.overlap.is_some());
    let mut sides = vec![sent.session.clone(), notice.with.clone().unwrap()];
    sides.sort();
    assert_eq!(sides, ["cc-login", "cc-main"], "one record for the two");
    let Event::Custom { data, .. } = &sent.event else {
        panic!("a custom event");
    };
    assert!(!data.to_string().contains("README"), "{data}");
}

#[tokio::test]
async fn a_hint_a_client_showed_goes_to_the_recorder_and_nothing_else_does_that_way() {
    use zirc_proto::activity::{HINT_SHOWN_KIND, HintShown, Record};
    let ergo = require_ergo!();
    let (machine, _main, _main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;
    let (_recorder, mut recorded) =
        zirc_test_irc::person(&ergo, zirc_proto::activity::RECORDER).await;

    let shown = HintShown {
        hint: "0123456789abcdef".into(),
        kind: "overlap".into(),
        layer: "repo".into(),
    };
    let record = |kind: &str, data: serde_json::Value| Request::Record {
        session: "cc-login".into(),
        kind: kind.into(),
        data,
    };
    // Not a playbook record, or not one of its kind: refused.
    for (kind, data) in [
        ("x-zirc/wake", serde_json::to_value(&shown).unwrap()),
        (HINT_SHOWN_KIND, serde_json::json!({ "words": "x" })),
    ] {
        let refused = machine.try_request(record(kind, data)).await;
        assert!(refused.is_err(), "{refused:?}");
    }
    let done = machine
        .request(record(
            HINT_SHOWN_KIND,
            serde_json::to_value(&shown).unwrap(),
        ))
        .await;
    assert!(matches!(done, Response::Done), "{done:?}");
    let sent = loop {
        let line =
            zirc_test_irc::next_matching(&mut recorded, |line| line.command == "TAGMSG").await;
        let sent = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap();
        if HintShown::of(&sent.event).is_some() {
            break sent;
        }
    };
    assert_eq!(sent.session, "cc-login");
    assert!(sent.is_live(), "never in history");
    assert_eq!(HintShown::of(&sent.event), Some(shown));
}

#[tokio::test]
async fn a_session_two_worktrees_register_is_warned_of_never_refused() {
    let ergo = require_ergo!();
    let (machine, main, _main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut login = machine.watch(Some("cc-login")).await;
    zirc_test_ergo::git::git(
        main.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "other",
            ".claude/worktrees/other",
        ],
    );
    let other_root = main
        .path()
        .join(".claude/worktrees/other")
        .canonicalize()
        .unwrap();
    // A second worktree takes the name: it's let, and said.
    let Response::Repos { repos } = machine
        .request(Request::RepoAdd {
            path: other_root.clone(),
            session: Some("cc-login".into()),
        })
        .await
    else {
        panic!("repos");
    };
    let here = repos.iter().find(|repo| repo.path == other_root).unwrap();
    assert!(
        here.sessions.iter().any(|it| it.session == "cc-login"),
        "not refused"
    );
    assert_eq!(here.shared.len(), 1, "{here:?}");
    assert_eq!(here.shared[0].with, login_root);
    let warned = loop {
        let delivery = next_post(&mut login).await;
        if let Event::Text { body } = &delivery.message.event
            && body.contains("two sessions under one name")
        {
            break body.clone();
        }
    };
    assert!(warned.starts_with("cc-login: warning:"), "{warned}");
    assert!(
        warned.contains(&login_root.display().to_string()),
        "{warned}"
    );
    // Moving from the main checkout to a worktree of its own is no warning.
    let Response::Repos { repos } = machine
        .request(Request::RepoAdd {
            path: login_root.clone(),
            session: Some("cc-main".into()),
        })
        .await
    else {
        panic!("repos");
    };
    let login_repo = repos.iter().find(|repo| repo.path == login_root).unwrap();
    assert!(login_repo.shared.is_empty(), "{login_repo:?}");
}

#[tokio::test]
async fn a_person_typing_in_any_irc_client_reaches_the_session_they_name() {
    let ergo = require_ergo!();
    let (machine, _main, _main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut login_hears = machine.watch(Some("cc-login")).await;
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#agents").await;

    // Chatter that names nobody stays out of the session's way; a line naming it doesn't.
    ada.privmsg("#agents", "lunch, anyone?").unwrap();
    ada.privmsg("#agents", "cc-login: can you look at the login page?")
        .unwrap();
    let delivery = loop {
        let delivery = next(&mut login_hears).await;
        if delivery.why.is_some() {
            break delivery;
        }
        assert_eq!(delivery.why, None, "{delivery:?}");
    };
    assert_eq!(delivery.from, "ada");
    assert_eq!(delivery.message.session, "");
    assert_eq!(delivery.why, Some(Why::Mention));
    assert_eq!(
        delivery.message.event,
        Event::Text {
            body: "cc-login: can you look at the login page?".into()
        }
    );
}

#[tokio::test]
async fn a_reaction_to_a_sessions_message_reaches_it_as_who_gave_it() {
    let ergo = require_ergo!();
    // With a web app to link to, a post is answered once the server has echoed it, and so
    // once zircd knows it's cc-login's. Without one it's answered as it's sent, and Ada could
    // react before that echo came: Ergo sends the sender's echo once it's done with the
    // message, after the channel's others have it.
    let (machine, _main, _main_root, login_root) =
        machine_with_a_session_worktree_and(&ergo, "web = \"https://zirc.dev\"\n").await;
    let mut login_hears = machine.watch(Some("cc-login")).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#agents").await;

    post(&machine, &login_root, "cc-login", "merge the login page?").await;
    // Ada's client sees it, with the id the server gave it, and reacts.
    let said = zirc_test_irc::next_matching(&mut ada_in, |line| {
        line.command == "PRIVMSG" && line.params.get(1).is_some_and(|b| b.contains("login page"))
    })
    .await;
    let id = said.tag("msgid").expect("a msgid").to_owned();
    let react = |tag: &str| {
        let mut line = zirc_irc::Line::new("TAGMSG", ["#agents"]);
        line.tags = vec![
            (tag.to_owned(), Some("👍".to_owned())),
            ("+draft/reply".to_owned(), Some(id.clone())),
        ];
        line
    };
    ada.send(react("+draft/react")).unwrap();
    let reaction = next_reaction(&mut login_hears).await;
    assert_eq!(
        (
            reaction.from.as_str(),
            reaction.emoji.as_str(),
            reaction.to.as_str()
        ),
        ("ada", "👍", id.as_str())
    );
    assert_eq!(
        reaction.why,
        Some(Why::Reacted),
        "on cc-login's own message"
    );
    // Ada has no account here: who gave it is only her nick, which proves nothing.
    assert_eq!(reaction.account, None);
    assert!(!reaction.removed);
    // Another session here hears it too, but it's not its message.
    assert_eq!(next_reaction(&mut main_hears).await.why, None);

    ada.send(react("+draft/unreact")).unwrap();
    let taken = next_reaction(&mut login_hears).await;
    assert!(
        taken.removed && taken.why == Some(Why::Reacted),
        "{taken:?}"
    );
}

#[tokio::test]
async fn naming_the_machine_or_its_owner_wakes_its_person_not_each_session() {
    let ergo = require_ergo!();
    let alpha = Machine::start_with(&ergo, "alpha", "owner = \"ada\"\n").await;
    let beta = Machine::start(&ergo, "beta").await;
    let (alpha_clone, beta_clone) = (clone(), clone());
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
        machine
            .eventually(
                |status| status.channels.iter().any(|c| c == "#agents"),
                "joining",
            )
            .await;
    }
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let mut person = alpha.watch(None).await;

    // As CI says what it did for the machine's person: every session there shares the
    // machine's nick, so it's news to none of them.
    for text in ["alpha: production runs 1a2b3c", "ada: staging runs 1a2b3c"] {
        post(&beta, beta_clone.path(), "ci", text).await;
        assert_eq!(next_post_why(&mut cc1).await, (text.into(), None));
        assert_eq!(
            next_post_why(&mut person).await,
            (text.into(), Some(Why::Mention))
        );
    }
    // A landing names the session it's for.
    let landed = "cc-1: landed login as 1a2b3c (build 7)";
    post(&beta, beta_clone.path(), "ci", landed).await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        (landed.into(), Some(Why::Mention))
    );
}

#[tokio::test]
async fn a_session_messages_a_person_it_names() {
    let ergo = require_ergo!();
    let alpha = Machine::start(&ergo, "alpha").await;
    let (_ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    let dm = |to: &str| Request::Dm {
        session: "cc-1".into(),
        text: "your build is green".into(),
        to: Some(to.into()),
    };

    assert_eq!(alpha.request(dm("ada")).await, Response::channel("ada"));
    // In the zirc envelope, signed with the session as posts are.
    let line = zirc_test_irc::next_matching(&mut ada_hears, |line| line.command == "PRIVMSG").await;
    let message = zirc_proto::decode(&line.params[1]).unwrap().unwrap();
    assert_eq!(message.session, "cc-1");
    assert_eq!(
        message.event,
        Event::Text {
            body: "your build is green".into()
        }
    );
    let refused = ipc::request(&alpha.socket(), &dm("#agents"))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("isn't a person's nick"), "{refused}");
}

#[tokio::test]
async fn a_session_shows_its_typing_until_it_answers_or_its_time_is_up() {
    let ergo = require_ergo!();
    let alpha = Machine::start(&ergo, "alpha").await;
    let (_ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    let typing = |seconds: u64| Request::Typing {
        repo: PathBuf::from("/"),
        session: "cc-1".into(),
        to: "ada".into(),
        seconds,
    };
    // zircd's own live events, such as its live sessions, aren't what's looked for.
    let tagmsg = |line: &zirc_irc::Line| {
        line.command == "TAGMSG" && line.tag(zirc_proto::EVENT_TAG).is_none()
    };
    let state = |line: &zirc_irc::Line| line.tag("+typing").map(str::to_owned);

    // Typing until it answers: the answer, then done.
    assert_eq!(alpha.request(typing(60)).await, Response::channel("ada"));
    let line = zirc_test_irc::next_matching(&mut ada_hears, tagmsg).await;
    assert_eq!(state(&line).as_deref(), Some("active"));
    alpha
        .request(Request::Dm {
            session: "cc-1".into(),
            text: "here's the answer".into(),
            to: Some("ada".into()),
        })
        .await;
    let answer = zirc_test_irc::next_matching(&mut ada_hears, |line| {
        line.command == "PRIVMSG" || line.command == "TAGMSG"
    })
    .await;
    assert_eq!(answer.command, "PRIVMSG");
    let line = zirc_test_irc::next_matching(&mut ada_hears, tagmsg).await;
    assert_eq!(state(&line).as_deref(), Some("done"));

    // Typing for a second: done when it's up, without an answer.
    alpha.request(typing(1)).await;
    let line = zirc_test_irc::next_matching(&mut ada_hears, tagmsg).await;
    assert_eq!(state(&line).as_deref(), Some("active"));
    let line = zirc_test_irc::next_matching(&mut ada_hears, tagmsg).await;
    assert_eq!(state(&line).as_deref(), Some("done"));
}

#[tokio::test]
async fn a_session_reacts_to_a_message_as_the_apps_do() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), _beta] = two_machines(&ergo).await;
    let (ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_hears, "#agents").await;
    let mut cc1 = alpha.watch(Some("cc-1")).await;

    ada.privmsg("#agents", "can someone look at the login page?")
        .unwrap();
    let asked = loop {
        let delivery = next(&mut cc1).await;
        if delivery.from == "ada" {
            break delivery;
        }
    };
    let id = asked.id.clone().expect("the server's id");
    let react = |message: Option<&str>, last: Option<&str>, emoji: &str| Request::React {
        repo: alpha_clone.path().to_owned(),
        session: "cc-1".into(),
        message: message.map(Into::into),
        last: last.map(Into::into),
        to: None,
        emoji: emoji.into(),
        remove: false,
    };
    let tagmsg = |line: &zirc_irc::Line| line.tag("+draft/react").is_some();

    // By the start of its id, as zirc watch shows it.
    assert_eq!(
        alpha.request(react(Some(&id[..8]), None, "👀")).await,
        Response::channel("#agents")
    );
    let line = zirc_test_irc::next_matching(&mut ada_hears, tagmsg).await;
    assert_eq!(line.params, ["#agents"]);
    assert_eq!(line.tag("+draft/react"), Some("👀"));
    assert_eq!(line.tag("+draft/reply"), Some(id.as_str()));

    // Or the latest from someone, without its id.
    assert_eq!(
        alpha.request(react(None, Some("ada"), "👍")).await,
        Response::channel("#agents")
    );
    let line = zirc_test_irc::next_matching(&mut ada_hears, tagmsg).await;
    assert_eq!(line.tag("+draft/react"), Some("👍"));
    assert_eq!(line.tag("+draft/reply"), Some(id.as_str()));
}

#[tokio::test]
async fn a_sessions_post_is_answered_with_its_link() {
    let ergo = require_ergo!();
    let alpha = Machine::start_with(&ergo, "alpha", "web = \"https://zirc.dev\"\n").await;
    let checkout = clone();
    alpha
        .request(Request::RepoAdd {
            path: checkout.path().to_owned(),
            session: None,
        })
        .await;
    alpha
        .eventually(|status| status.channels == JOINED, "joining")
        .await;
    let (ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_hears, "#agents").await;
    // A post is answered once the server echoes it, with its own link.
    let Response::Channel { channel, link } = alpha
        .request(Request::Post {
            repo: checkout.path().to_owned(),
            session: "cc-1".into(),
            text: "looking at it".into(),
            to: ipc::Target::Agents,
            app: None,
            everyone: false,
            reply: None,
        })
        .await
    else {
        panic!("not posted");
    };
    assert_eq!(channel, "#agents");
    let read = zirc_proto::link::parse(&link.expect("the post's link")).unwrap();
    assert_eq!(read.channel, "#agents");
    let posted = loop {
        let heard =
            zirc_test_irc::next_matching(&mut ada_hears, |line| line.command == "PRIVMSG").await;
        if heard
            .params
            .last()
            .is_some_and(|text| text.contains("looking at it"))
        {
            break heard;
        }
    };
    assert_eq!(read.message.as_deref(), posted.tag("msgid"));
}

#[tokio::test]
async fn a_sessions_first_watch_says_it_is_live_before_it_posts() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), _beta] = two_machines(&ergo).await;
    let (ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_hears, "#agents").await;
    let live_of = |line: &zirc_irc::Line| {
        let message = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG)?).ok()?;
        assert_eq!(message.level, Some(zirc_proto::Level::Debug));
        zirc_proto::session::Live::of(&message.event)
    };

    let _cc1 = alpha.watch(Some("cc-1")).await;
    let line = zirc_test_irc::next_matching(&mut ada_hears, |line| {
        live_of(line).is_some_and(|live| live.sessions == ["cc-1"])
    })
    .await;
    assert_eq!(line.command, "TAGMSG");
    assert_eq!(line.params, ["#agents"]);
}

#[tokio::test]
async fn starting_to_watch_a_repository_invites_the_machines_owner_to_agents() {
    let ergo = require_ergo!();
    let (_ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    let alpha = Machine::start_with(&ergo, "ada-mbp", "owner = \"ada\"\n").await;
    let (first, second) = (clone(), clone());
    alpha
        .request(Request::RepoAdd {
            path: first.path().to_owned(),
            session: None,
        })
        .await;
    let line = zirc_test_irc::next_matching(&mut ada_hears, |line| line.command == "INVITE").await;
    assert_eq!(line.params, ["ada", "#agents"]);

    // Another checkout of the same repository, and a session added to it: nothing new.
    for (path, session) in [(first.path(), Some("cc-1")), (second.path(), None)] {
        alpha
            .request(Request::RepoAdd {
                path: path.to_owned(),
                session: session.map(Into::into),
            })
            .await;
    }
    let again = tokio::time::timeout(
        Duration::from_secs(2),
        zirc_test_irc::next_matching(&mut ada_hears, |line| line.command == "INVITE"),
    )
    .await;
    assert!(again.is_err(), "invited again: {again:?}");
}

#[tokio::test]
async fn a_sessions_first_watch_goes_to_the_recorder_once() {
    use zirc_proto::activity::{HINT_SHOWN_KIND, HintShown, Record, Watching};
    let ergo = require_ergo!();
    let (machine, _main, _main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;
    let (_recorder, mut recorded) =
        zirc_test_irc::person(&ergo, zirc_proto::activity::RECORDER).await;
    let next_record = async |recorded: &mut zirc_irc::Incoming| {
        let line = zirc_test_irc::next_matching(recorded, |line| line.command == "TAGMSG").await;
        zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap()
    };

    let first = machine.watch(Some("cc-login")).await;
    let sent = next_record(&mut recorded).await;
    assert_eq!(sent.session, "cc-login");
    assert!(sent.is_live(), "never in history");
    assert_eq!(Watching::of(&sent.event), Some(Watching {}));
    drop(first);

    // Watching again, as `zirc watch --once` does after each wake, says nothing more: the
    // next record is the hint that follows it.
    let _again = machine.watch(Some("cc-login")).await;
    let shown = HintShown {
        hint: "0123456789abcdef".into(),
        kind: "overlap".into(),
        layer: "repo".into(),
    };
    let done = machine
        .request(Request::Record {
            session: "cc-login".into(),
            kind: HINT_SHOWN_KIND.into(),
            data: serde_json::to_value(&shown).unwrap(),
        })
        .await;
    assert!(matches!(done, Response::Done), "{done:?}");
    let sent = next_record(&mut recorded).await;
    assert_eq!(HintShown::of(&sent.event), Some(shown), "{sent:?}");
}
