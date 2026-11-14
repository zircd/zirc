//! The client against a real Ergo server.

use std::time::Duration;

use zirc_irc::{Config, Credentials, Error, Incoming, Line, connect};
use zirc_proto::{Event, Message};
use zirc_test_ergo::{Ergo, require_ergo};

const WAIT: Duration = Duration::from_secs(10);

fn config(ergo: &Ergo, nick: &str) -> Config {
    Config {
        tls: false,
        ..Config::new("127.0.0.1", ergo.port(), nick)
    }
}

/// The next line that `matches`, skipping the rest.
async fn next_matching(incoming: &mut Incoming, matches: impl Fn(&Line) -> bool) -> Line {
    tokio::time::timeout(WAIT, async {
        loop {
            let line = incoming.recv().await.expect("connection ended");
            if matches(&line) {
                return line;
            }
        }
    })
    .await
    .expect("no matching line in time")
}

async fn joined(incoming: &mut Incoming, nick: &str, channel: &str) {
    next_matching(incoming, |line| {
        line.command == "JOIN"
            && line.params.first().is_some_and(|c| c == channel)
            && line
                .source
                .as_deref()
                .is_some_and(|s| s.starts_with(&format!("{nick}!")))
    })
    .await;
}

#[tokio::test]
async fn carries_zirc_messages_between_clients() {
    let ergo = require_ergo!();
    let (alice, mut alice_in) = connect(&config(&ergo, "alice")).await.unwrap();
    let (bob, mut bob_in) = connect(&config(&ergo, "bob")).await.unwrap();
    assert!(alice.has_cap("message-tags"), "{}", ergo.log());

    alice.join("#api").unwrap();
    joined(&mut alice_in, "alice", "#api").await;
    bob.join("#api").unwrap();
    joined(&mut bob_in, "bob", "#api").await;

    // An event that's kept in history goes as a PRIVMSG body.
    let sent = Message::new(
        "cc-1",
        Event::Commit {
            paths: vec!["src/auth.rs".into()],
            branch: "main".into(),
            sha: "abc123".into(),
            subject: "Fix login".into(),
            repo: None,
        },
    );
    alice
        .privmsg("#api", &zirc_proto::encode(&sent).unwrap())
        .unwrap();

    let line = next_matching(&mut bob_in, |line| line.command == "PRIVMSG").await;
    assert!(
        line.source
            .as_deref()
            .is_some_and(|s| s.starts_with("alice!"))
    );
    assert_eq!(line.params[0], "#api");
    assert_eq!(zirc_proto::decode(&line.params[1]).unwrap(), Some(sent));
    // server-time came along as a tag.
    assert!(line.tag("time").is_some());

    // One that's only true now, a working set, goes as a TAGMSG's client tag, which Ergo
    // passes on whole.
    let working = Message::new(
        "cc-1",
        Event::Working {
            paths: vec!["src/auth.rs".into()],
            continued: false,
            worktree: Some("api".into()),
            branch: Some("login".into()),
            part: Some([1, 1]),
            base: None,
            committed: Vec::new(),
        },
    );
    let mut tagged = Line::new("TAGMSG", ["#api"]);
    tagged.tags = vec![(
        zirc_proto::EVENT_TAG.to_owned(),
        Some(zirc_proto::payload(&working).unwrap()),
    )];
    alice.send(tagged).unwrap();
    let line = next_matching(&mut bob_in, |line| line.command == "TAGMSG").await;
    let payload = line
        .tag(zirc_proto::EVENT_TAG)
        .expect("the event tag came along");
    assert_eq!(zirc_proto::from_payload(payload).unwrap(), working);
}

/// Ergo passes on a client-only tag, such as the app a message was sent from, to clients
/// that take `message-tags`, and keeps it in history.
#[tokio::test]
async fn keeps_client_tags_live_and_in_history() {
    const CLIENT: &str = "+zirc.dev/client";
    let ergo = require_ergo!();
    let (alice, mut alice_in) = connect(&config(&ergo, "alice")).await.unwrap();
    let (bob, mut bob_in) = connect(&config(&ergo, "bob")).await.unwrap();
    alice.join("#api").unwrap();
    joined(&mut alice_in, "alice", "#api").await;
    bob.join("#api").unwrap();
    joined(&mut bob_in, "bob", "#api").await;

    let mut said = Line::new("PRIVMSG", ["#api", "hello"]);
    said.tags = vec![(CLIENT.to_owned(), Some("desktop".to_owned()))];
    alice.send(said).unwrap();
    let live = next_matching(&mut bob_in, |line| line.command == "PRIVMSG").await;
    assert_eq!(live.tag(CLIENT), Some("desktop"), "live: {live}");

    let (carol, mut carol_in) = connect(&config(&ergo, "carol")).await.unwrap();
    carol.join("#api").unwrap();
    joined(&mut carol_in, "carol", "#api").await;
    carol
        .send(Line::new("CHATHISTORY", ["LATEST", "#api", "*", "10"]))
        .unwrap();
    // History has HistServ's words for the joins too.
    let replayed = next_matching(&mut carol_in, |line| {
        line.command == "PRIVMSG" && line.tag("batch").is_some() && line.params[1] == "hello"
    })
    .await;
    assert_eq!(
        replayed.tag(CLIENT),
        Some("desktop"),
        "replayed: {replayed}"
    );
}

/// History kept as zirc's servers keep it, in SQLite, gives back a client tag as long as a
/// message's client tags may be, after a restart: IRCv3 allows 4094 bytes of them, the
/// tag's name included. End-to-end sealed parts of messages would travel this way.
#[tokio::test]
async fn keeps_a_long_client_tag_in_history_across_a_restart() {
    const TAG: &str = "+zirc.dev/sealed";
    let Some(mut ergo) = Ergo::start_with(zirc_test_ergo::HISTORY) else {
        return;
    };
    let (alice, mut alice_in) = connect(&config(&ergo, "alice")).await.unwrap();
    let (bob, mut bob_in) = connect(&config(&ergo, "bob")).await.unwrap();
    alice.join("#api").unwrap();
    joined(&mut alice_in, "alice", "#api").await;
    bob.join("#api").unwrap();
    joined(&mut bob_in, "bob", "#api").await;

    // Base64url, as a sealed part's would be, filling what's left after the name and `=`.
    let value: String = "AbC-_0189xYz"
        .chars()
        .cycle()
        .take(4094 - TAG.len() - 1)
        .collect();
    let mut said = Line::new("PRIVMSG", ["#api", "the key is [sealed]"]);
    said.tags = vec![(TAG.to_owned(), Some(value.clone()))];
    alice.send(said).unwrap();
    // Heard once Ergo has taken it, so it's in history before the restart.
    let live = next_matching(&mut bob_in, |line| line.command == "PRIVMSG").await;
    assert_eq!(live.tag(TAG), Some(value.as_str()), "live: {live}");
    drop((alice, bob));

    ergo.restart();
    let (carol, mut carol_in) = connect(&config(&ergo, "carol")).await.unwrap();
    carol.join("#api").unwrap();
    joined(&mut carol_in, "carol", "#api").await;
    carol
        .send(Line::new("CHATHISTORY", ["LATEST", "#api", "*", "10"]))
        .unwrap();
    let replayed = next_matching(&mut carol_in, |line| {
        line.command == "PRIVMSG"
            && line.tag("batch").is_some()
            && line.params[1].contains("[sealed]")
    })
    .await;
    assert_eq!(replayed.tag(TAG), Some(value.as_str()), "{}", ergo.log());
}

#[tokio::test]
async fn takes_another_nick_when_its_own_is_taken() {
    let ergo = require_ergo!();
    let (first, _first_in) = connect(&config(&ergo, "zed")).await.unwrap();
    let (second, _second_in) = connect(&config(&ergo, "zed")).await.unwrap();
    assert_eq!(first.nick(), "zed");
    assert_eq!(second.nick(), "zed_");
}

#[tokio::test]
async fn logs_in_to_an_account_with_sasl() {
    let ergo = require_ergo!();

    // Register the account the way a person would, through NickServ.
    let (carol, mut carol_in) = connect(&config(&ergo, "carol")).await.unwrap();
    carol.privmsg("NickServ", "REGISTER hunter22").unwrap();
    next_matching(&mut carol_in, |line| {
        line.command == "NOTICE"
            && line
                .source
                .as_deref()
                .is_some_and(|s| s.starts_with("NickServ!"))
    })
    .await;
    carol.quit("bye").unwrap();

    let with_password = |password: &str| Config {
        sasl: Some(Credentials {
            account: "carol".into(),
            password: password.into(),
        }),
        ..config(&ergo, "carol-laptop")
    };
    let (laptop, _laptop_in) = connect(&with_password("hunter22")).await.unwrap();
    assert!(laptop.has_cap("sasl"));
    assert!(matches!(
        connect(&with_password("wrong")).await,
        Err(Error::Sasl(_))
    ));
    // A device's token the server doesn't take (this one has no device keys): the passphrase.
    let with_token = |password: &str| Config {
        bearer: Some(std::sync::Arc::new(|| Some("not-a-token".to_owned()))),
        ..with_password(password)
    };
    let (key_refused, _) = connect(&with_token("hunter22")).await.unwrap();
    assert_eq!(key_refused.nick(), "carol");
    assert!(matches!(
        connect(&with_token("wrong")).await,
        Err(Error::Sasl(_))
    ));
}

#[tokio::test]
async fn needs_the_server_password_when_set() {
    let Some(hash) = Ergo::hash_password("gate") else {
        return;
    };
    // A server password is the team's gate, and accounts log in with SASL, so PASS must not
    // double as an account login, as it does in Ergo's defaults.
    let Some(ergo) = Ergo::start_with(&[
        ("ERGO__SERVER__PASSWORD", &hash),
        ("ERGO__ACCOUNTS__LOGIN_VIA_PASS_COMMAND", "false"),
    ]) else {
        return;
    };
    let with_password = |password: Option<&str>| Config {
        server_password: password.map(Into::into),
        ..config(&ergo, "dave")
    };
    assert!(connect(&with_password(Some("gate"))).await.is_ok());
    assert!(matches!(
        connect(&with_password(Some("wrong"))).await,
        Err(Error::BadPassword | Error::Closed(_))
    ));
}
