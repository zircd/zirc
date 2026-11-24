//! What agents do, said where people see it: a wake's line and a settled overlap's.

use zirc_core::ipc::{Delivery, Heard, Request, Response, Watching, Why};
use zirc_proto::woke::{By, Settled, What, Woke};
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

/// Alpha and beta, each with a session working in its clone: `cc-a` and `cc-b`.
async fn two_sessions(ergo: &zirc_test_ergo::Ergo) -> [(Machine, tempfile::TempDir); 2] {
    let machines = two_machines(ergo).await;
    for ((machine, clone), session) in machines.iter().zip(["cc-a", "cc-b"]) {
        machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
    }
    machines
}

/// `session` on `machine`, in `repo`, says it settled README.md with `with`, who lands first.
async fn settle(machine: &Machine, repo: &std::path::Path, session: &str, with: &str) -> Response {
    machine
        .request(Request::Settle {
            repo: repo.to_owned(),
            session: session.into(),
            with: with.into(),
            first: "cc-b".into(),
            paths: vec![repo.join("README.md")],
            same_lines: false,
        })
        .await
}

/// The next wake's line heard, and what it says.
async fn next_woke(watching: &mut Watching) -> (Delivery, Woke) {
    loop {
        if let Heard::Message(delivery) = heard(watching).await
            && let Some(woke) = Woke::of(&delivery.message.event)
        {
            return (delivery, woke);
        }
    }
}

#[tokio::test]
async fn a_wake_is_said_as_the_session_it_woke_and_wakes_no_one() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), (beta, beta_clone)] = two_sessions(&ergo).await;
    let mut woken = alpha.watch(Some("cc-a")).await;
    let mut asker = beta.watch(Some("cc-b")).await;
    let mut everything = beta.watch(None).await;
    post(
        &beta,
        beta_clone.path(),
        "cc-b",
        "cc-a: is the cache keyed by branch?",
    )
    .await;
    assert_eq!(
        next_post_why(&mut woken).await.1,
        Some(Why::Mention),
        "the post wakes cc-a"
    );
    let (line, woke) = next_woke(&mut everything).await;
    assert_eq!(
        (line.from.as_str(), line.message.session.as_str()),
        ("alpha", "cc-a")
    );
    assert_eq!(
        (line.channel.as_str(), line.message.level),
        ("#agents", Some(zirc_proto::Level::Info))
    );
    assert_eq!((woke.what, woke.by), (What::Mentioned, By::Agent));
    assert_eq!(
        (
            woke.from.as_deref(),
            woke.session.as_deref(),
            woke.channel.as_deref()
        ),
        (Some("beta"), Some("cc-b"), Some("#agents"))
    );
    assert!(woke.wake.is_some() && woke.line.is_some(), "{woke:?}");
    // Whoever caused it hears the line with no reason, so its own watch stays asleep.
    let (heard, _) = next_woke(&mut asker).await;
    assert_eq!(heard.why, None);
}

#[tokio::test]
async fn a_settled_overlap_is_said_once_and_wakes_no_one() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_sessions(&ergo).await;
    let mut other = beta.watch(Some("cc-b")).await;
    // Heard of, so alpha knows where cc-b works.
    post(&beta, beta_clone.path(), "cc-b", "taking: the cache").await;
    let mut hears = alpha.watch(None).await;
    post(&beta, beta_clone.path(), "cc-b", "still on the cache").await;
    next_post(&mut hears).await;
    let Response::Settled { text, said } =
        settle(&alpha, alpha_clone.path(), "cc-a", "beta/cc-b").await
    else {
        panic!("not settled");
    };
    assert!(said);
    assert_eq!(
        text,
        "overlap settled: cc-b lands first, cc-a rebases onto it: README.md (different lines, \
         no conflict)"
    );
    let said = loop {
        let delivery = next(&mut other).await;
        if let Some(settled) = Settled::of(&delivery.message.event) {
            break (settled, delivery.why);
        }
    };
    assert!(said.0.between(("alpha", "cc-a"), ("beta", "cc-b")));
    assert_eq!((said.0.first.session.as_str(), said.1), ("cc-b", None));
    // The other side saying the same is told it's said, and says nothing more.
    let Response::Settled { said, .. } =
        settle(&beta, beta_clone.path(), "cc-b", "alpha/cc-a").await
    else {
        panic!("not settled");
    };
    assert!(!said);
}
