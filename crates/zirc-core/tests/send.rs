//! Sending events against a real Ergo: a short one as a line, a long one as one multiline
//! message, and pieces where the server takes no multiline messages.

use std::time::Duration;

use zirc_core::send::send;
use zirc_irc::multiline::Batches;
use zirc_irc::{Config, Incoming, Line, connect};
use zirc_proto::Event;
use zirc_test_ergo::{Ergo, require_ergo};

const WAIT: Duration = Duration::from_secs(10);
const CHANNEL: &str = "#send";

fn config(ergo: &Ergo, nick: &str) -> Config {
    Config {
        tls: false,
        ..Config::new("127.0.0.1", ergo.port(), nick)
    }
}

/// The next whole `PRIVMSG`, multiline batches joined.
async fn next_privmsg(incoming: &mut Incoming, batches: &mut Batches) -> Line {
    tokio::time::timeout(WAIT, async {
        loop {
            let line = incoming.recv().await.expect("connection ended");
            if let Some(line) = batches.feed(line)
                && line.command == "PRIVMSG"
            {
                return line;
            }
        }
    })
    .await
    .expect("no PRIVMSG in time")
}

/// The events `count` `PRIVMSG`s carry.
async fn received(incoming: &mut Incoming, count: usize) -> Vec<Event> {
    let mut batches = Batches::default();
    let mut events = Vec::new();
    for _ in 0..count {
        let line = next_privmsg(incoming, &mut batches).await;
        let message = zirc_proto::decode(&line.params[1]).unwrap().unwrap();
        assert_eq!(message.session, "cc-1");
        events.push(message.event);
    }
    events
}

/// Waits until `incoming` hears `nick` join [`CHANNEL`].
async fn joined(incoming: &mut Incoming, nick: &str) {
    let source = format!("{nick}!");
    tokio::time::timeout(WAIT, async {
        loop {
            let line = incoming.recv().await.expect("connection ended");
            if line.command == "JOIN"
                && line
                    .source
                    .as_deref()
                    .is_some_and(|s| s.starts_with(&source))
            {
                return;
            }
        }
    })
    .await
    .expect("no JOIN in time");
}

/// Alice to send and Bob to hear, both in [`CHANNEL`], and Alice's incoming lines, which
/// keep her connected.
async fn pair(ergo: &Ergo) -> (zirc_irc::Client, Incoming, Incoming) {
    let (alice, alice_in) = connect(&config(ergo, "alice")).await.unwrap();
    let (bob, mut bob_in) = connect(&config(ergo, "bob")).await.unwrap();
    bob.join(CHANNEL).unwrap();
    joined(&mut bob_in, "bob").await;
    alice.join(CHANNEL).unwrap();
    // Bob hears Alice join, so she's in before she speaks.
    joined(&mut bob_in, "alice").await;
    (alice, alice_in, bob_in)
}

fn long_post() -> Event {
    let text = "cc-9: approve https://buildkite.com/zirc/monorepo/builds/123456: ".to_owned()
        + &["web controller alert-bridge test-results"; 12].join(" ");
    assert!(text.len() > 400);
    Event::Custom {
        kind: zirc_proto::ci::DEPLOY_KIND.into(),
        data: serde_json::json!({"build": "123456"}),
        text,
    }
}

#[tokio::test]
async fn sends_a_short_event_as_a_line() {
    let ergo = require_ergo!();
    let (alice, _alice_in, mut bob_in) = pair(&ergo).await;
    let event = Event::Text {
        body: "hello".into(),
    };
    send(&alice, CHANNEL, "cc-1", event.clone()).unwrap();
    assert_eq!(received(&mut bob_in, 1).await, [event]);
}

#[tokio::test]
async fn sends_a_long_event_as_one_multiline_message() {
    let ergo = require_ergo!();
    let (alice, _alice_in, mut bob_in) = pair(&ergo).await;
    assert!(alice.multiline().is_some());
    send(&alice, CHANNEL, "cc-1", long_post()).unwrap();
    assert_eq!(received(&mut bob_in, 1).await, [long_post()]);
}

#[tokio::test]
async fn sends_pieces_without_multiline() {
    let Some(ergo) = Ergo::start_with(&[("ERGO__LIMITS__MULTILINE__MAX_BYTES", "0")]) else {
        return;
    };
    let (alice, _alice_in, mut bob_in) = pair(&ergo).await;
    assert!(alice.multiline().is_none());
    let expected = zirc_core::send::fitting("cc-1", long_post());
    assert!(expected.len() > 1);
    send(&alice, CHANNEL, "cc-1", long_post()).unwrap();
    let events = received(&mut bob_in, expected.len()).await;
    let Event::Custom { text: whole, .. } = long_post() else {
        unreachable!()
    };
    let text: Vec<String> = events
        .iter()
        .map(|event| match event {
            Event::Custom { text, .. } => text.clone(),
            other => panic!("not a custom event: {other:?}"),
        })
        .collect();
    assert_eq!(text.join(" "), whole);
}
