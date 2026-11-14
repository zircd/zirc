//! Machines on a shared Ergo: what one hears of another, what a daemon keeps and refuses.

use std::time::Duration;
use zirc_core::ipc::{self, Request, Response, Target, Why};
use zirc_core::paths;
use zirc_proto::Event;
use zirc_test_ergo::git::git;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn a_machine_drops_the_ops_it_gets_for_making_a_channel() {
    let ergo = require_ergo!();
    let [(_alpha, _), (_beta, _)] = two_machines(&ergo).await;
    let mut config = zirc_irc::Config::new("127.0.0.1", ergo.port(), "looker");
    config.tls = false;
    let (client, mut incoming) = zirc_irc::connect(&config).await.unwrap();
    client.join("#api").unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        client.send(zirc_irc::Line::new("NAMES", ["#api"])).unwrap();
        let names = tokio::time::timeout(WAIT, async {
            loop {
                let line = incoming.recv().await.unwrap();
                if line.command == "353" {
                    break line.params.last().cloned().unwrap_or_default();
                }
            }
        })
        .await
        .unwrap();
        assert!(names.contains("alpha"), "{names}");
        if !names.split(' ').any(|n| n.starts_with('@')) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "still opped: {names}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn one_machine_hears_what_happens_in_anothers_clone() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("ZIRC_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    let mut beta_hears = beta.watch(None).await;

    // An edit: a new file, which git status reports as untracked.
    std::fs::create_dir(alpha_clone.path().join("src")).unwrap();
    std::fs::write(alpha_clone.path().join("src/auth.rs"), "fn login() {}\n").unwrap();
    assert_eq!(
        next_working(&mut beta_hears, "alpha").await,
        ["src/auth.rs"]
    );

    // A branch switch moves HEAD but isn't a commit, so nothing is said...
    git(alpha_clone.path(), &["checkout", "--quiet", "-b", "login"]);
    // ...and the commit after it is.
    git(alpha_clone.path(), &["add", "src/auth.rs"]);
    git(
        alpha_clone.path(),
        &["commit", "--quiet", "--message", "Add login"],
    );
    let commit = next(&mut beta_hears).await;
    let Event::Commit {
        branch, subject, ..
    } = commit.message.event
    else {
        panic!("expected a commit, got {:?}", commit.message.event)
    };
    assert_eq!((branch.as_str(), subject.as_str()), ("login", "Add login"));

    // What someone says through the CLI goes to the same channel.
    let posted = alpha
        .request(Request::Post {
            repo: alpha_clone.path().join("src"),
            session: "cc-1".into(),
            text: "taking the login flow".into(),
            to: Target::Repo,
            app: None,
            everyone: false,
            force: true,
            reply: None,
        })
        .await;
    assert_eq!(posted, Response::channel("#api"));
    let said = next(&mut beta_hears).await;
    assert_eq!(said.message.session, "cc-1");
    assert_eq!(
        said.message.event,
        Event::Text {
            body: "taking the login flow".into()
        }
    );

    // Stopping watching leaves the channel.
    let removed = beta
        .request(Request::RepoRemove {
            path: beta_clone.path().to_owned(),
            session: None,
        })
        .await;
    assert_eq!(removed, Response::Repos { repos: vec![] });
    beta.eventually(|status| status.channels.is_empty(), "leaving")
        .await;
}

#[tokio::test]
async fn remembers_watched_repositories_across_restarts() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("ZIRC_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    let ergo = require_ergo!();
    let machine = Machine::start(&ergo, "gamma").await;
    let project = clone();
    machine
        .request(Request::RepoAdd {
            path: project.path().to_owned(),
            session: None,
        })
        .await;
    machine
        .request(Request::Join {
            repo: project.path().to_owned(),
            topic: "auth".into(),
            org: false,
            session: None,
        })
        .await;

    // A second daemon on the same directory refuses to start while the first runs...
    let second = zircd::run(machine.home.path().to_owned(), std::future::pending()).await;
    assert!(second.unwrap_err().to_string().contains("already running"));

    // ...and after the first stops, the next one watches what it watched...
    let machine = machine.restart().await;
    let Response::Repos { repos } = machine.request(Request::RepoList).await else {
        panic!("the repositories watched");
    };
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].channel, "#api");
    // ...and rejoins its topics.
    machine
        .eventually(
            |status| status.channels.contains(&"#api/auth".to_owned()),
            "rejoining the topic",
        )
        .await;
}

#[tokio::test]
async fn remembers_what_sessions_wait_on_across_restarts() {
    let ergo = require_ergo!();
    let machine = Machine::start(&ergo, "gamma").await;
    let waits = |on: &[&str]| Request::WaitFor {
        session: "cc-1".into(),
        on: on.iter().map(|&it| it.to_owned()).collect(),
        states: Vec::new(),
        reactions: Vec::new(),
        drop: false,
    };
    let Response::Awaits { on } = machine.request(waits(&["1300", "fix-login"])).await else {
        panic!("what cc-1 waits on");
    };
    assert_eq!(on, ["1300", "fix-login"]);
    let machine = machine.restart().await;
    let Response::Awaits { on } = machine.request(waits(&[])).await else {
        panic!("what cc-1 waits on, after a restart");
    };
    assert_eq!(on, ["1300", "fix-login"]);
}

#[tokio::test]
async fn keeps_saved_state_it_cant_read() {
    let ergo = require_ergo!();
    let home = tempfile::Builder::new()
        .prefix("zirc")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::write(
        paths::config(home.path()),
        format!(
            "nick = \"delta\"\n[server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n",
            ergo.port()
        ),
    )
    .unwrap();
    std::fs::write(paths::repos(home.path()), "[{\"path\": ").unwrap();
    let _daemon = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    let aside = home.path().join("repos.json.unreadable");
    tokio::time::timeout(WAIT, async {
        while !aside.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("repos.json kept aside");
    assert_eq!(std::fs::read_to_string(aside).unwrap(), "[{\"path\": ");
}

#[tokio::test]
async fn of_two_started_at_once_on_one_home_one_runs() {
    // No server: the lock and the socket come before any connection.
    let home = tempfile::Builder::new()
        .prefix("zirc")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::write(
        paths::config(home.path()),
        "nick = \"twice\"\n[server]\nhost = \"127.0.0.1\"\nport = 1\ntls = false\n",
    )
    .unwrap();
    // A lock file a zircd that's gone left behind holds nothing.
    std::fs::write(paths::lock(home.path()), "1\n").unwrap();
    let mut first = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    let mut second = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    let (refused, running) = tokio::time::timeout(WAIT, async {
        tokio::select! {
            ended = &mut first => (ended, second),
            ended = &mut second => (ended, first),
        }
    })
    .await
    .expect("one of them refused to run");
    let err = refused.unwrap().unwrap_err().to_string();
    let pid = std::process::id();
    assert!(
        err.contains(&format!("zircd is already running (process {pid}")),
        "{err}"
    );
    let status = ipc::request(&paths::socket(home.path()), &Request::Status).await;
    assert!(status.is_ok(), "the one running answers: {status:?}");
    assert!(!running.is_finished());

    // Once it's gone, the home is free again.
    running.abort();
    let _ = running.await;
    let next = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    zirc_test_irc::eventually("the next zircd answering", || async {
        ipc::request(&paths::socket(home.path()), &Request::Status)
            .await
            .is_ok()
    })
    .await;
    next.abort();
}

#[tokio::test]
async fn a_watch_hears_what_it_missed_since_the_last_one_ended() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;
    drop(machine.watch(Some("cc-login")).await);

    post(&machine, &main_root, "cc-main", "chatter").await;
    post(&machine, &main_root, "cc-main", "cc-login: rebase please").await;
    // Once zircd has told every watch, and found cc-login's gone.
    let mut main_hears = machine.watch(Some("cc-main")).await;
    post(&machine, &main_root, "cc-main", "sent").await;
    while next_post_why(&mut main_hears).await.0 != "sent" {}

    let mut login_hears = machine.watch(Some("cc-login")).await;
    let said: Vec<(String, Option<Why>)> = [
        next_post_why(&mut login_hears).await,
        next_post_why(&mut login_hears).await,
        next_post_why(&mut login_hears).await,
    ]
    .into();
    assert_eq!(
        said,
        [
            (
                "cc-login: while no watch of yours ran, this concerned you:".to_owned(),
                Some(Why::Mention)
            ),
            ("cc-login: rebase please".to_owned(), Some(Why::Mention)),
            (
                "cc-login: that's what you missed".to_owned(),
                Some(Why::Mention)
            ),
        ]
    );
}

#[tokio::test]
async fn a_watch_that_hung_up_is_noticed_at_once_and_its_news_kept() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;
    let login = Some("cc-login".to_owned());
    drop(machine.watch(Some("cc-login")).await);
    // No news arrives to show the watch is gone: zircd sees the socket close, so the
    // session isn't counted as watching (which leases rely on too).
    machine
        .eventually(
            |status| !status.watching.contains(&login),
            "the hung-up watch gone",
        )
        .await;

    post(&machine, &main_root, "cc-main", "cc-login: rebase please").await;
    let mut login_hears = machine.watch(Some("cc-login")).await;
    let said = [
        next_post_why(&mut login_hears).await.0,
        next_post_why(&mut login_hears).await.0,
    ];
    assert_eq!(
        said,
        [
            "cc-login: while no watch of yours ran, this concerned you:",
            "cc-login: rebase please"
        ]
    );
}

#[tokio::test]
async fn a_direct_message_to_a_machine_is_refused() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    std::fs::write(beta_clone.path().join("README.md"), "beta's take\n").unwrap();
    until_who(
        &alpha,
        |channels| {
            channels
                .iter()
                .any(|c| c.members.iter().any(|m| m.nick == "beta"))
        },
        "beta's working set",
    )
    .await;
    let _ = &beta;

    let refused = ipc::request(
        &alpha.socket(),
        &Request::Dm {
            session: "cc-1".into(),
            text: "hello".into(),
            to: Some("beta".into()),
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(refused.contains("beta is a machine"), "{refused}");
}

#[tokio::test]
async fn a_channel_the_server_refused_is_joined_once_it_can_be() {
    let ergo = require_ergo!();
    // Ada has #api first, and lets nobody in, as a server coming back from a restart may.
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#api").await;
    ada.send(zirc_irc::Line::new("MODE", ["#api", "+i"]))
        .unwrap();
    // The server echoes the mode once it's set.
    zirc_test_irc::next_matching(&mut ada_in, |line| {
        line.command == "MODE" && line.params.get(1).is_some_and(|mode| mode.contains('i'))
    })
    .await;

    let alpha = Machine::start(&ergo, "alpha").await;
    let clone = clone();
    alpha
        .request(Request::RepoAdd {
            path: clone.path().to_owned(),
            session: None,
        })
        .await;
    alpha
        .eventually(
            |status| status.channels.iter().any(|c| c == "#general"),
            "joining #general",
        )
        .await;
    assert!(
        !alpha
            .status()
            .await
            .unwrap()
            .channels
            .contains(&"#api".to_owned()),
        "#api let alpha in"
    );

    ada.send(zirc_irc::Line::new("MODE", ["#api", "-i"]))
        .unwrap();
    alpha
        .eventually(
            |status| status.channels.iter().any(|c| c == "#api"),
            "joining #api again",
        )
        .await;
}
