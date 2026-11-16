//! A session's ritual, as few commands as it can be: its `zirc watch` registers it in the
//! repository it runs in, its `zirc intend` takes its task on, and zircd says `done:` when
//! that task's landing lands. One test alone in its binary, as it runs commands in a clone
//! and so changes the process's directory.

use std::path::Path;
use std::time::Duration;

use clap::Parser as _;
use zirc::Cli;
use zirc_test_ergo::require_ergo;

const WAIT: Duration = Duration::from_secs(15);

const CI_PASSPHRASE: &str = "ci-passphrase";

async fn run(home: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut out = Vec::new();
    let mut all = vec!["zirc", "--home", home.to_str().unwrap()];
    all.extend(args);
    zirc::run(Cli::try_parse_from(all)?, &mut out).await?;
    Ok(String::from_utf8(out)?)
}

/// `zirc <args>` in `dir`, as a session there runs it.
async fn zirc_in(dir: &Path, home: &Path, args: &[&str]) -> anyhow::Result<String> {
    std::env::set_current_dir(dir).unwrap();
    run(home, args).await
}

/// `zirc <args>` in `dir` again until it says `has`: what it said then.
async fn until(dir: &Path, home: &Path, args: &[&str], has: &str) -> String {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(out) = zirc_in(dir, home, args).await
                && out.contains(has)
            {
                return out;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("zirc {args:?} never said {has:?}"))
}

#[tokio::test]
async fn a_watch_registers_intend_takes_the_task_and_its_landing_says_done() {
    let ergo = require_ergo!();
    let home = tempfile::Builder::new()
        .prefix("zirc")
        .tempdir_in("/tmp")
        .unwrap();
    let port = ergo.port().to_string();
    let init = [
        "init",
        "--server",
        "127.0.0.1",
        "--port",
        &port,
        "--no-tls",
        "--nick",
        "alpha",
    ];
    run(home.path(), &init).await.unwrap();
    tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    let home = home.path();
    let clone = zirc_test_ergo::git::clone("https://github.com/acme/web.git");
    let repo = clone.path();
    until(repo, home, &["repo", "add"], "#web").await;
    until(repo, home, &["status"], " #web").await;

    // A session's watch, started in the repository, is all it needs to be registered there.
    let watch_home = home.to_owned();
    let watch = tokio::spawn(async move {
        let args = ["watch", "--once", "--session", "cc-t"];
        run(&watch_home, &args).await
    });
    until(repo, home, &["repo", "list"], "cc-t").await;

    // Its intend takes the task on, once, and says nobody else is there.
    let intend = [
        "intend",
        "--session",
        "cc-t",
        "login.ts",
        "-m",
        "the login form",
    ];
    let said = zirc_in(repo, home, &intend).await.unwrap();
    assert!(
        said.starts_with("posted taking: the login form to #web/agents: "),
        "{said}"
    );
    assert!(said.contains(zirc_core::clashes::NOBODY), "{said}");
    let task = zirc_in(repo, home, &["task", "--session", "cc-t"])
        .await
        .unwrap();
    assert!(zirc_proto::task_id(task.trim()).is_some(), "{task}");
    let again = [
        "intend",
        "--session",
        "cc-t",
        "signup.ts",
        "-m",
        "the signup form",
    ];
    let said = zirc_in(repo, home, &again).await.unwrap();
    assert!(!said.contains("taking:"), "taken twice: {said}");
    let still = zirc_in(repo, home, &["task", "--session", "cc-t"])
        .await
        .unwrap();
    assert_eq!(still, task, "the same task");

    // Its landing landing finishes it, with zircd's done:, which the session needn't post.
    let claimed = zirc_in(repo, home, &["task", "--landing", "--session", "cc-t"])
        .await
        .unwrap();
    assert_eq!(claimed, task);
    zirc_test_ergo::signin::register(ergo.port(), "zirc-ci", CI_PASSPHRASE).expect("CI's account");
    let event = zirc_proto::Event::Custom {
        kind: zirc_proto::ci::LANDING_KIND.into(),
        data: serde_json::json!({"session": "cc-t", "branch": "login", "state": "passed"}),
        text: "cc-t: landed login as 1a2b3c4 (build 7)".into(),
    };
    let line = zirc_proto::encode(&zirc_proto::Message::new("ci", event)).unwrap();
    zirc_test_ergo::signin::say_as(ergo.port(), "zirc-ci", CI_PASSPHRASE, "#web", &line)
        .expect("CI said it");
    // zircd's done: ends it (its words are zircd's tests' to check).
    tokio::time::timeout(WAIT, async {
        while !zirc_in(repo, home, &["task", "--session", "cc-t"])
            .await
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("its landing never ended the task");
    watch.abort();

    an_unwatched_repository_ends_a_watch_at_once(home).await;
}

/// In a Git repository this machine doesn't watch, a session's watch ends at once saying so,
/// for the session to decide whether to use zirc at all.
async fn an_unwatched_repository_ends_a_watch_at_once(home: &Path) {
    let stranger = zirc_test_ergo::git::clone("https://github.com/acme/api.git");
    let said = tokio::time::timeout(
        WAIT,
        zirc_in(
            stranger.path(),
            home,
            &["watch", "--once", "--session", "cc-u"],
        ),
    )
    .await
    .expect("the watch ended")
    .unwrap();
    assert!(
        said.contains("no repository is watched for cc-u")
            && said.contains("zirc repo add --session cc-u"),
        "{said}"
    );
}
