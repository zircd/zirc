//! The host against a real Ergo: an owner's machine asks and gets the command's output, a
//! machine can't call what is for the owner's own account, and a stranger gets no answer.

use std::time::Duration;

use tokio::task::JoinHandle;
use zirc_irc::multiline::Batches;
use zirc_irc::{Client, Credentials, Incoming, Line};
use zirc_proto::{Event, Message};
use zirc_remote::{Config, Options, run};
use zirc_test_ergo::{Ergo, require_ergo};
use zirc_test_irc::{config, host_machine, next_whole_matching, register};

const CONFIG: &str = r##"{
    "trust": ["zirc-alerts"],
    "channels": ["#machines"],
    "commands": {
        "work": {
            "summary": "Says how far it is",
            "who": "mine",
            "exec": ["/bin/sh", "-c", "echo progress: one; echo progress: two; echo all done"]
        },
        "slow": {
            "summary": "Takes a second",
            "who": "mine",
            "exec": ["/bin/sleep", "1"]
        },
        "echo": {
            "summary": "Say it back",
            "who": "mine",
            "args": { "text": { "maxLength": 2000 } },
            "exec": ["/bin/echo", "{text}"]
        },
        "count": {
            "summary": "Count to n",
            "who": "mine",
            "args": { "n": { "type": "integer", "min": 1, "max": 3 } },
            "exec": ["/bin/echo", "{n}"]
        },
        "fail": {
            "summary": "Always fails",
            "who": "mine",
            "exec": ["/bin/sh", "-c", "echo nope >&2; exit 3"]
        },
        "nap": {
            "summary": "Runs too long",
            "who": "mine",
            "timeout": "300ms",
            "exec": ["/bin/sleep", "5"]
        },
        "key": {
            "summary": "Says what its secret is",
            "who": "mine",
            "secrets": { "MYKEY": "hunter" },
            "exec": ["/bin/sh", "-c", "printenv MYKEY"]
        },
        "nokey": {
            "summary": "Looks for a secret it wasn't given",
            "who": "mine",
            "exec": ["/bin/sh", "-c", "printenv MYKEY || echo none"]
        },
        "lost": {
            "summary": "Needs a secret the host doesn't have",
            "who": "mine",
            "secrets": { "OTHER": "missing" },
            "exec": ["/bin/echo", "x"]
        },
        "big": {
            "summary": "Says a lot",
            "who": "mine",
            "exec": ["/bin/sh", "-c", "yes y | head -c 6000 | tr -d '\\n'"]
        },
        "owner-only": {
            "summary": "For the owner's own account",
            "exec": ["/bin/echo", "for the owner"]
        }
    }
}"##;

const HOST: &str = "mattias-ci";

/// What a client hears, with its multiline batches joined as apps see them.
struct Inbox {
    incoming: Incoming,
    batches: Batches,
}

impl Inbox {
    /// What the next answer to query `id` says.
    async fn answer(&mut self, id: &str) -> String {
        let line = next_whole_matching(&mut self.incoming, &mut self.batches, |line| {
            answered(line, id).is_some()
        })
        .await;
        answered(&line, id).expect("an answer")
    }

    /// What the next plain message from the host says.
    async fn text(&mut self) -> String {
        let line = next_whole_matching(&mut self.incoming, &mut self.batches, |line| {
            said(line).is_some()
        })
        .await;
        said(&line).expect("a message")
    }

    /// Whether a plain message from the host comes within half a second.
    async fn hears_text(&mut self) -> bool {
        tokio::time::timeout(
            Duration::from_millis(500),
            next_whole_matching(&mut self.incoming, &mut self.batches, |line| {
                said(line).is_some()
            }),
        )
        .await
        .is_ok()
    }

    /// Whether an answer to one of the queries `ids` comes within half a second.
    async fn hears_any(&mut self, ids: &[&str]) -> bool {
        tokio::time::timeout(
            Duration::from_millis(500),
            next_whole_matching(&mut self.incoming, &mut self.batches, |line| {
                ids.iter().any(|id| answered(line, id).is_some())
            }),
        )
        .await
        .is_ok()
    }
}

/// Registers `name` and logs in to it with SASL, so the server tells who it is.
async fn account(ergo: &Ergo, name: &str) -> (Client, Inbox) {
    register(ergo, name, "pass").await;
    login(ergo, name).await
}

/// Logs in to the registered account `name` with SASL.
async fn login(ergo: &Ergo, name: &str) -> (Client, Inbox) {
    let config = zirc_irc::Config {
        sasl: Some(Credentials {
            account: name.to_owned(),
            password: "pass".to_owned(),
        }),
        ..config(ergo, name)
    };
    let (client, incoming) = zirc_irc::connect(&config).await.expect("logged in");
    (
        client,
        Inbox {
            incoming,
            batches: Batches::default(),
        },
    )
}

/// `owner`'s machine `name`, logged in: its host says whose it is, as the server sets it.
async fn machine(ergo: &Ergo, name: &str, owner: &str) -> (Client, Inbox) {
    register(ergo, name, "pass").await;
    host_machine(ergo, name, owner).await;
    login(ergo, name).await
}

/// The host, as `HOST`'s account.
async fn host(ergo: &Ergo) -> JoinHandle<anyhow::Result<()>> {
    host_with(ergo, CONFIG).await
}

/// The host with another config.
async fn host_with(ergo: &Ergo, config: &str) -> JoinHandle<anyhow::Result<()>> {
    register(ergo, HOST, "pass").await;
    let options = Options {
        server: "127.0.0.1".to_owned(),
        port: ergo.port(),
        tls: false,
        account: HOST.to_owned(),
        owner: "mattias".to_owned(),
        on: None,
        passphrase: "pass".to_owned(),
        secrets: [("hunter".to_owned(), "hunter2".to_owned())].into(),
        // The test's own Ergo, whose hosts only the test sets, as zirc.dev's are.
        trust_hosts: true,
    };
    tokio::spawn(run(options, Config::parse(config).expect("a config")))
}

fn ask(client: &Client, id: &str, text: &str) {
    let event = Event::Query {
        id: id.to_owned(),
        to: None,
        text: text.to_owned(),
    };
    zirc_core::send::send_message(client, HOST, &Message::new("cc-test", event)).expect("sent");
}

/// What `line` answers query `id` with, if it does.
fn answered(line: &Line, id: &str) -> Option<String> {
    let body = line.params.get(1)?;
    match zirc_proto::decode(body).ok()??.event {
        Event::Answer { query, text, .. } if query == id => Some(text),
        _ => None,
    }
}

/// What `line` says, if it is a plain message from the host.
/// What `line` says, if it is a plain message from the host: the machine's own, without a
/// session, so an app shows it in the person's thread with the machine.
fn said(line: &Line) -> Option<String> {
    let body = line.params.get(1)?;
    let message = zirc_proto::decode(body).ok()??;
    match message.event {
        Event::Text { body } if message.session == zirc_proto::NO_SESSION => Some(body),
        _ => None,
    }
}

/// Says `text` to the host as an app does: a plain message, not a query.
fn chat(client: &Client, text: &str) {
    let event = Event::Text {
        body: text.to_owned(),
    };
    zirc_core::send::send_message(client, HOST, &Message::new("app", event)).expect("sent");
}

/// Asks until the host, which is still logging in, answers.
async fn ready(client: &Client, inbox: &mut Inbox) {
    for n in 0..50 {
        let id = format!("ready{n}");
        ask(client, &id, "ping");
        if tokio::time::timeout(Duration::from_millis(300), inbox.answer(&id))
            .await
            .is_ok()
        {
            return;
        }
    }
    panic!("the host never answered");
}

#[tokio::test]
async fn an_owners_machine_asks_and_gets_the_output() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (client, mut inbox) = machine(&ergo, "m3-mac", "mattias").await;
    ready(&client, &mut inbox).await;

    ask(&client, "q1", "echo hello world");
    assert_eq!(inbox.answer("q1").await, "hello world");

    ask(&client, "q2", "count n=2");
    assert_eq!(inbox.answer("q2").await, "2");

    ask(&client, "q3", "fail");
    let failed = inbox.answer("q3").await;
    assert!(
        failed.contains("exit 3") && failed.contains("nope"),
        "{failed}"
    );

    ask(&client, "q4", "count n=9");
    assert!(inbox.answer("q4").await.contains("at most 3"));

    ask(&client, "q5", "nap");
    assert!(inbox.answer("q5").await.contains("ran past"));

    ask(&client, "q6", "nothing-here");
    assert!(inbox.answer("q6").await.contains("no command"));
}

#[tokio::test]
async fn the_host_sits_in_its_channels_so_its_owners_apps_see_it_online() {
    let ergo = require_ergo!();
    let (client, mut inbox) = account(&ergo, "mattias").await;
    client.join("#machines").expect("joined");
    let _host = host(&ergo).await;
    let joined = next_whole_matching(&mut inbox.incoming, &mut inbox.batches, |line| {
        line.command == "JOIN"
            && line
                .source
                .as_deref()
                .is_some_and(|source| source.starts_with(&format!("{HOST}!")))
    })
    .await;
    assert_eq!(joined.params.first().map(String::as_str), Some("#machines"));
}

#[tokio::test]
async fn a_long_question_and_a_long_answer_both_get_through() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (client, mut inbox) = machine(&ergo, "m3-mac", "mattias").await;
    ready(&client, &mut inbox).await;

    // Too long for one IRC line, so each way it goes as a multiline batch: the answer in two.
    let long = "x".repeat(1500);
    ask(&client, "l1", &format!("echo {long}"));
    let first = inbox.answer("l1").await;
    assert_eq!(first.len(), 1400);
    assert_eq!(first + &inbox.answer("l1").await, long);
}

#[tokio::test]
async fn an_answer_too_long_for_one_message_comes_whole_in_parts() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (client, mut inbox) = machine(&ergo, "m3-mac", "mattias").await;
    ready(&client, &mut inbox).await;

    // As long as Claude's answer about a landing can be: its text twice, in its data and for
    // plain clients, is more than one multiline batch holds.
    ask(&client, "b1", "big");
    let mut said = String::new();
    while said.len() < 3000 {
        said.push_str(&inbox.answer("b1").await);
    }
    assert_eq!(said, "y".repeat(3000));
}

#[tokio::test]
async fn a_machine_may_not_call_what_is_for_the_owners_own_account() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (machine, mut machine_inbox) = machine(&ergo, "m3-mac", "mattias").await;
    let (person, mut person_inbox) = account(&ergo, "mattias").await;
    ready(&machine, &mut machine_inbox).await;

    ask(&machine, "m1", "owner-only");
    assert!(machine_inbox.answer("m1").await.starts_with("refused"));
    ask(&machine, "m2", "help");
    let help = machine_inbox.answer("m2").await;
    assert!(
        help.contains("echo") && !help.contains("owner-only"),
        "{help}"
    );

    ask(&person, "p1", "owner-only");
    assert_eq!(person_inbox.answer("p1").await, "for the owner");
    ask(&person, "p2", "help");
    assert!(person_inbox.answer("p2").await.contains("owner-only"));
}

#[tokio::test]
async fn a_stranger_gets_no_answer() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (machine, mut machine_inbox) = machine(&ergo, "m3-mac", "mattias").await;
    let (stranger, mut stranger_inbox) = account(&ergo, "zed-box").await;
    ready(&machine, &mut machine_inbox).await;

    ask(&stranger, "s1", "ping");
    ask(&machine, "o1", "ping");
    assert_eq!(machine_inbox.answer("o1").await, "pong");
    assert!(
        !stranger_inbox.hears_any(&["s1"]).await,
        "a stranger is not answered, not even to say no"
    );
}

#[tokio::test]
async fn progress_comes_as_it_is_said_and_the_rest_at_the_end() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (client, mut inbox) = machine(&ergo, "m3-mac", "mattias").await;
    ready(&client, &mut inbox).await;

    ask(&client, "w1", "work");
    let mut said = Vec::new();
    for _ in 0..3 {
        said.push(inbox.answer("w1").await);
    }
    assert_eq!(said, ["progress: one", "progress: two", "all done"]);
}

#[tokio::test]
async fn a_call_while_the_same_command_runs_is_told_it_is_busy() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (client, mut inbox) = machine(&ergo, "m3-mac", "mattias").await;
    ready(&client, &mut inbox).await;

    ask(&client, "b1", "slow");
    ask(&client, "b2", "slow");
    assert!(inbox.answer("b2").await.contains("already running"));
    assert_eq!(inbox.answer("b1").await, "done");
}

#[tokio::test]
async fn a_trusted_integration_speaks_for_the_owner_and_nobody_else() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (machine, mut machine_inbox) = machine(&ergo, "m3-mac", "mattias").await;
    let (alerts, mut alerts_inbox) = account(&ergo, "zirc-alerts").await;
    ready(&machine, &mut machine_inbox).await;

    ask(&alerts, "t1", "as mattias owner-only");
    assert_eq!(alerts_inbox.answer("t1").await, "for the owner");

    // Not for anyone else, and not without saying who: no answer at all.
    ask(&alerts, "t2", "as zed owner-only");
    ask(&alerts, "t3", "owner-only");
    ask(&alerts, "t4", "as mattias ping");
    assert_eq!(alerts_inbox.answer("t4").await, "pong");
    assert!(
        !alerts_inbox.hears_any(&["t2", "t3"]).await,
        "it spoke for someone it wasn't told to"
    );
}

#[tokio::test]
async fn the_owner_chats_with_the_host_in_plain_messages() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (machine, mut machine_inbox) = machine(&ergo, "m3-mac", "mattias").await;
    let (person, mut person_inbox) = account(&ergo, "mattias").await;
    ready(&machine, &mut machine_inbox).await;

    // As an app sends it.
    chat(&person, "echo hello there");
    assert_eq!(person_inbox.text().await, "hello there");

    // As a plain IRC client sends it: no zirc framing at all.
    person
        .privmsg(HOST, "echo from a plain client")
        .expect("sent");
    assert_eq!(person_inbox.text().await, "from a plain client");

    // What is for the owner's own account works here, as the person's app is that account.
    chat(&person, "owner-only");
    assert_eq!(person_inbox.text().await, "for the owner");
    chat(&person, "help");
    assert!(person_inbox.text().await.contains("echo"));
}

#[tokio::test]
async fn what_isnt_a_command_goes_whole_to_the_one_the_config_names() {
    let ergo = require_ergo!();
    let _host = host_with(
        &ergo,
        r#"{
            "otherwise": "say",
            "commands": {
                "say": {
                    "summary": "Says it back",
                    "who": "mine",
                    "args": { "text": {} },
                    "exec": ["/bin/echo", "{text}"]
                }
            }
        }"#,
    )
    .await;
    let (machine, mut machine_inbox) = machine(&ergo, "m3-mac", "mattias").await;
    let (person, mut person_inbox) = account(&ergo, "mattias").await;
    ready(&machine, &mut machine_inbox).await;

    chat(&person, "hey, what are you testing?");
    assert_eq!(person_inbox.text().await, "hey, what are you testing?");
    // A command is still that command, and help still lists them.
    chat(&person, "say hello");
    assert_eq!(person_inbox.text().await, "hello");
    chat(&person, "help");
    assert!(person_inbox.text().await.contains("say"));
}

#[tokio::test]
async fn the_host_is_typing_while_it_works_on_a_persons_message() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (machine, mut machine_inbox) = machine(&ergo, "m3-mac", "mattias").await;
    let (person, mut person_inbox) = account(&ergo, "mattias").await;
    ready(&machine, &mut machine_inbox).await;

    chat(&person, "slow");
    let typing = |state: &'static str| {
        move |line: &Line| {
            line.command == "TAGMSG"
                && line
                    .source
                    .as_deref()
                    .is_some_and(|s| s.starts_with(&format!("{HOST}!")))
                && line.tag("+typing") == Some(state)
        }
    };
    next_whole_matching(
        &mut person_inbox.incoming,
        &mut person_inbox.batches,
        typing("active"),
    )
    .await;
    assert_eq!(person_inbox.text().await, "done");
    next_whole_matching(
        &mut person_inbox.incoming,
        &mut person_inbox.batches,
        typing("done"),
    )
    .await;
}

#[tokio::test]
async fn nobody_but_the_owners_own_account_is_answered_in_plain_messages() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (machine, mut machine_inbox) = machine(&ergo, "m3-mac", "mattias").await;
    let (stranger, mut stranger_inbox) = account(&ergo, "zed-box").await;
    let (person, mut person_inbox) = account(&ergo, "mattias").await;
    ready(&machine, &mut machine_inbox).await;

    // An agent on the owner's machine asks with a query, which is answered as information. A
    // plain reply to it would read as its user's own words.
    chat(&machine, "ping");
    chat(&stranger, "ping");
    machine.privmsg(HOST, "ping").expect("sent");
    chat(&person, "ping");
    assert_eq!(person_inbox.text().await, "pong");
    assert!(!machine_inbox.hears_text().await, "a machine was answered");
    assert!(
        !stranger_inbox.hears_text().await,
        "a stranger was answered"
    );
}

#[tokio::test]
async fn a_command_gets_the_secrets_it_names_and_no_other_does() {
    let ergo = require_ergo!();
    let _host = host(&ergo).await;
    let (client, mut inbox) = machine(&ergo, "m3-mac", "mattias").await;
    ready(&client, &mut inbox).await;

    ask(&client, "k1", "key");
    assert_eq!(inbox.answer("k1").await, "hunter2");
    ask(&client, "k2", "nokey");
    assert_eq!(inbox.answer("k2").await, "none");
    // A secret the host wasn't given is named, never shown, and nothing runs.
    ask(&client, "k3", "lost");
    let lost = inbox.answer("k3").await;
    assert!(
        lost.contains("missing") && !lost.contains("hunter2"),
        "{lost}"
    );
}
