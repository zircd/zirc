//! Reading a channel's history through zircd.

use std::time::Duration;
use zirc_core::ipc::{self, Request, Response};
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn a_reply_read_from_history_says_what_it_answers() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), _beta] = two_machines(&ergo).await;
    let (ada, _incoming) = zirc_test_irc::person(&ergo, "ada").await;
    ada.join("#agents").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ada.privmsg("#agents", "who's on the login page?").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let read = |alpha: &Machine| {
        let request = Request::Read {
            target: "#agents".into(),
            limit: 50,
            since: None,
            until: None,
        };
        let socket = alpha.socket();
        async move {
            let Ok(Response::Messages { messages }) = ipc::request(&socket, &request).await else {
                panic!("no messages");
            };
            messages
        }
    };
    let asked = read(&alpha)
        .await
        .into_iter()
        .find(|it| it.from == "ada")
        .unwrap();
    let mut line = zirc_irc::Line::new("PRIVMSG", ["#agents", "me, today"]);
    line.tags = vec![("+draft/reply".to_owned(), asked.id.clone())];
    ada.send(line).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let answer = read(&alpha)
        .await
        .into_iter()
        .find(|it| it.message.event.human_text() == "me, today")
        .expect("the answer");
    assert_eq!(answer.reply, asked.id);
}

#[tokio::test]
async fn reads_a_channels_history_between_two_moments() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), _beta] = two_machines(&ergo).await;
    let (ada, _incoming) = zirc_test_irc::person(&ergo, "ada").await;
    ada.join("#lunch").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ada.privmsg("#lunch", "before").unwrap();
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };
    // A whole second after "before", and a whole second before "after".
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let moment = now() + 1;
    tokio::time::sleep(Duration::from_millis(2100)).await;
    ada.privmsg("#lunch", "after").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let read = |since: Option<u64>, until: Option<u64>| {
        let request = Request::Read {
            target: "#lunch".into(),
            limit: 50,
            since,
            until,
        };
        let socket = alpha.socket();
        async move {
            let Ok(Response::Messages { messages }) = ipc::request(&socket, &request).await else {
                panic!("#lunch's history");
            };
            messages
                .iter()
                .map(|it| it.message.event.human_text())
                .filter(|text| text == "before" || text == "after")
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(read(None, None).await, ["before", "after"]);
    assert_eq!(read(Some(moment), None).await, ["after"]);
    assert_eq!(read(None, Some(moment)).await, ["before"]);
}

#[tokio::test]
async fn reads_past_joins_and_parts_to_the_messages_before_them() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), _beta] = two_machines(&ergo).await;
    let (ada, _ada_incoming) = zirc_test_irc::person(&ergo, "ada").await;
    ada.join("#lunch").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ada.privmsg("#lunch", "noodles?").unwrap();
    // Then only comings and goings, more than one read's page.
    let (bob, _bob_incoming) = zirc_test_irc::person(&ergo, "bob").await;
    for _ in 0..3 {
        bob.join("#lunch").unwrap();
        bob.send(zirc_irc::Line::new("PART", ["#lunch"])).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let request = Request::Read {
        target: "#lunch".into(),
        limit: 1,
        since: None,
        until: None,
    };
    let Ok(Response::Messages { messages }) = ipc::request(&alpha.socket(), &request).await else {
        panic!("#lunch's history");
    };
    let texts: Vec<String> = messages
        .iter()
        .map(|it| it.message.event.human_text())
        .collect();
    assert_eq!(texts, ["noodles?"]);
}

#[tokio::test]
async fn reading_a_channel_never_makes_one() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), _beta] = two_machines(&ergo).await;
    let (ada, mut incoming) = zirc_test_irc::person(&ergo, "ada").await;
    ada.join("#lunch").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ada.privmsg("#lunch", "noodles?").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let read = |target: &str| {
        let request = Request::Read {
            target: target.into(),
            limit: 50,
            since: None,
            until: None,
        };
        let socket = alpha.socket();
        async move { ipc::request(&socket, &request).await }
    };
    // A channel the machine isn't in, but the server has: read, as before.
    let Ok(Response::Messages { messages }) = read("#lunch").await else {
        panic!("#lunch's history");
    };
    assert!(
        messages
            .iter()
            .any(|it| it.message.event.human_text() == "noodles?")
    );
    // One the server hasn't: refused, rather than JOINed, which would make it.
    let message = read("#nowhere").await.unwrap_err().to_string();
    assert!(
        message.contains("#nowhere isn't on the server"),
        "{message}"
    );
    ada.send(zirc_irc::Line::new("LIST", ["#nowhere"])).unwrap();
    let listed = tokio::time::timeout(WAIT, async {
        let mut listed = false;
        while let Some(line) = incoming.recv().await {
            match line.command.as_str() {
                "322" => listed = true,
                "323" => return listed,
                _ => {}
            }
        }
        listed
    })
    .await
    .unwrap();
    assert!(!listed, "#nowhere was made");
}

/// zircd keeps `#ops`'s newest messages, which `zirc state` and `zirc status` read each time
/// they run: a read of them says what was said since, as reading the server would.
#[tokio::test]
async fn reads_of_ops_say_what_was_said_since_the_last() {
    let ergo = require_ergo!();
    let (solo, _main, _, _) = machine_with_a_session_worktree(&ergo).await;
    solo.eventually(
        |status| status.channels.iter().any(|c| c == "#ops"),
        "joining #ops",
    )
    .await;
    let (ada, _incoming) = zirc_test_irc::person(&ergo, "ada").await;
    ada.join("#ops").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let read = || read_ops(&solo, 2);
    let said = |text: &'static str| {
        ada.privmsg("#ops", text).unwrap();
        tokio::time::sleep(Duration::from_millis(300))
    };
    said("one").await;
    assert_eq!(read().await, ["one"]);
    said("two").await;
    said("three").await;
    assert_eq!(read().await, ["two", "three"]);
}

async fn read_ops(solo: &Machine, limit: usize) -> Vec<String> {
    {
        let request = Request::Read {
            target: "#ops".into(),
            limit,
            since: None,
            until: None,
        };
        let socket = solo.socket();
        async move {
            let Ok(Response::Messages { messages }) = ipc::request(&socket, &request).await else {
                panic!("no messages");
            };
            messages
                .into_iter()
                .map(|it| it.message.event.human_text())
                .collect::<Vec<_>>()
        }
    }
    .await
}

/// A read that waits past its time leaves its history to nobody: the lines in it are never
/// told to watches as if just said. On 10 Nov 2026 a session woke on every new watch to the
/// same three CI lines, long dealt with, as reads of its channel ran out of time.
#[tokio::test]
async fn history_a_read_gave_up_on_never_wakes_anyone() {
    let ergo = require_ergo!();
    let beta = Peer::join(Machine::start(&ergo, "beta").await, Some("cc-b")).await;
    // A plain IRC client in #api, which tells when something's been said there.
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#agents").await;
    // Said, and on the server, before alpha's zircd ever heard #api.
    beta.says("cc-a: dry-safety conflicts with master").await;
    tokio::time::timeout(
        WAIT,
        zirc_test_irc::next_matching(&mut ada_in, |line| {
            line.command == "PRIVMSG"
                && line
                    .params
                    .get(1)
                    .is_some_and(|body| body.contains("dry-safety"))
        }),
    )
    .await
    .expect("beta's post on the server");
    let hooks = zircd::Hooks {
        ask_wait: Some(Duration::ZERO),
        ..zircd::Hooks::default()
    };
    let alpha = Peer::join(Machine::hooked(&ergo, "alpha", hooks).await, Some("cc-a")).await;
    let mut watching = alpha.watch().await;

    let read = Request::Read {
        target: "#agents".into(),
        limit: 50,
        since: None,
        until: None,
    };
    let gave_up = alpha.machine.try_request(read).await;
    assert!(
        gave_up.is_err(),
        "the read waited past its time: {gave_up:?}"
    );
    // Its history has come by the time a post said after it has: the server holds back a
    // client asking much at once for up to two seconds.
    tokio::time::sleep(Duration::from_secs(3)).await;
    beta.says("cc-a: marker").await;

    let heard = until_marker(&mut watching, |heard| says(heard, "marker")).await;
    assert!(
        !heard.iter().any(|heard| says(heard, "dry-safety")),
        "history told as if just said: {heard:?}"
    );
}
