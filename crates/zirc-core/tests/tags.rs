//! Client-only tags against a real Ergo, as the apps use them through zirc-irc and
//! zirc-chat: two people in a channel see each other typing, an agent session thinking, and
//! each other's reactions, both ways.

use std::time::Duration;

use zirc_core::chat::{Chat, Options};
use zirc_core::connection::{APP_CAPS, link};
use zirc_irc::{Client, Config, Incoming, Line, connect};
use zirc_test_ergo::{Ergo, require_ergo};

const WAIT: Duration = Duration::from_secs(10);
const CHANNEL: &str = "#tags";

/// Someone connected, with the chat their app keeps.
struct Peer {
    client: Client,
    incoming: Incoming,
    chat: Chat,
}

impl Peer {
    async fn connect(ergo: &Ergo, nick: &str) -> Self {
        let mut config = Config {
            tls: false,
            ..Config::new("127.0.0.1", ergo.port(), nick)
        };
        config
            .caps
            .extend(APP_CAPS.iter().map(|&cap| cap.to_owned()));
        let (client, incoming) = connect(&config).await.unwrap();
        let mut chat = Chat::new(nick, "ergo", vec![CHANNEL.to_owned()], Options::default());
        let lines = chat.connected(link(&client));
        let peer = Self {
            client,
            incoming,
            chat,
        };
        peer.send(lines);
        peer
    }

    fn send(&self, lines: Vec<Line>) {
        for line in lines {
            eprintln!("{} -> {line}", self.client.nick());
            self.client.send(line).unwrap();
        }
    }

    /// Takes in what the server sends until `done` says the chat has what it waits for.
    async fn until(&mut self, what: &str, done: impl Fn(&Chat) -> bool) {
        tokio::time::timeout(WAIT, async {
            while !done(&self.chat) {
                let line = self.incoming.recv().await.expect("connection ended");
                eprintln!("{} <- {line}", self.client.nick());
                let replies = self.chat.receive(line);
                self.send(replies);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{} never saw {what}", self.client.nick()));
    }
}

fn in_channel(chat: &Chat, nick: &str) -> bool {
    chat.people(CHANNEL)
        .iter()
        .any(|person| person.nick == nick)
}

/// Alice and Bob, both in [`CHANNEL`], each seeing the other there.
async fn pair(ergo: &Ergo) -> (Peer, Peer) {
    let mut alice = Peer::connect(ergo, "alice").await;
    alice
        .until("her join", |chat| in_channel(chat, "alice"))
        .await;
    // Each joins once its app has taken in what registration sent.
    let mut bob = Peer::connect(ergo, "bob").await;
    bob.until("alice there", |chat| in_channel(chat, "alice"))
        .await;
    alice
        .until("bob join", |chat| in_channel(chat, "bob"))
        .await;
    (alice, bob)
}

/// The id of the last message in [`CHANNEL`] that `text` is, once there is one.
fn id_of(chat: &Chat, text: &str) -> Option<String> {
    chat.conversation(CHANNEL)?
        .entries
        .iter()
        .rev()
        .find(|entry| entry.kind == zirc_core::chat::EntryKind::Message(text.into()))
        .and_then(|entry| entry.id.clone())
}

fn reactors(chat: &Chat, id: &str, emoji: &str) -> Vec<String> {
    chat.conversation(CHANNEL)
        .map(|c| c.reactions(id))
        .unwrap_or_default()
        .iter()
        .find(|reaction| reaction.emoji == emoji)
        .map(|reaction| reaction.nicks.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn people_see_each_other_typing_both_ways() {
    let ergo = require_ergo!();
    let (mut alice, mut bob) = pair(&ergo).await;
    assert!(alice.client.has_cap("message-tags"));

    let typing = alice.chat.typed(CHANNEL, false);
    assert!(!typing.is_empty(), "alice sends no typing");
    alice.send(typing);
    bob.until("alice typing", |chat| {
        chat.typing_text(CHANNEL).as_deref() == Some("alice is typing…")
    })
    .await;

    let typing = bob.chat.typed(CHANNEL, false);
    bob.send(typing);
    alice
        .until("bob typing", |chat| {
            chat.typing_text(CHANNEL).as_deref() == Some("bob is typing…")
        })
        .await;
}

#[tokio::test]
async fn an_agent_session_shows_as_thinking() {
    let ergo = require_ergo!();
    let (alice, mut bob) = pair(&ergo).await;
    // What `zirc thinking` sends for a session.
    let mut line = Line::new("TAGMSG", [CHANNEL]);
    line.tags = vec![
        ("+typing".into(), Some("active".into())),
        ("+zirc.dev/session".into(), Some("cc-1".into())),
    ];
    alice.client.send(line).unwrap();
    bob.until("cc-1 thinking", |chat| {
        chat.typing_text(CHANNEL).as_deref() == Some("cc-1 is thinking…")
    })
    .await;
}

#[tokio::test]
async fn people_see_each_others_reactions_both_ways() {
    let ergo = require_ergo!();
    let (mut alice, mut bob) = pair(&ergo).await;

    let said = alice.chat.say(CHANNEL, "ship it?", false);
    alice.send(said);
    bob.until("alice's message", |chat| id_of(chat, "ship it?").is_some())
        .await;
    let id = id_of(&bob.chat, "ship it?").unwrap();
    alice
        .until("her own message's id", |chat| {
            id_of(chat, "ship it?").as_deref() == Some(&id)
        })
        .await;

    // Bob reacts; Alice sees it, and so does Bob.
    let react = bob.chat.react(CHANNEL, &id, "👍");
    bob.send(react);
    alice
        .until("bob's reaction", |chat| {
            reactors(chat, &id, "👍") == ["bob"]
        })
        .await;
    bob.until("his own reaction", |chat| {
        reactors(chat, &id, "👍") == ["bob"]
    })
    .await;

    // Alice reacts back.
    let react = alice.chat.react(CHANNEL, &id, "🎉");
    alice.send(react);
    bob.until("alice's reaction", |chat| {
        reactors(chat, &id, "🎉") == ["alice"]
    })
    .await;
}
