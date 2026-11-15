use std::sync::atomic::{AtomicU8, Ordering};

use super::*;
use crate::chat::{Here, Link, Options, Ran};
use crate::command::Command;
use crate::sealed::KEY_LEN;

fn key() -> Key {
    Key::new("acme", [7; KEY_LEN])
}

/// Nonces that differ, as random ones would.
fn counting(bytes: &mut [u8]) -> bool {
    static NEXT: AtomicU8 = AtomicU8::new(0);
    bytes.fill(NEXT.fetch_add(1, Ordering::Relaxed));
    true
}

/// `nick` in #api, holding `keys`, on a server that echoes.
fn chat(nick: &str, keys: Vec<Key>) -> Chat {
    let mut chat = Chat::new(nick, "irc.zirc.test", Vec::new(), Options::default());
    chat.random = counting;
    chat.set_keys(keys);
    chat.connected(Link {
        nick: nick.into(),
        chathistory: true,
        echo: true,
        read_marker: false,
        trim: true,
        multiline: None,
        metadata: false,
    });
    chat.receive(format!(":{nick}!u@h JOIN #api").parse().unwrap());
    chat
}

fn here(name: &str) -> Here<'_> {
    Here {
        name,
        picked: None,
        replying: None,
        editing: None,
        newest: (0, 0),
    }
}

/// What `from` sends saying `text` in #api, as another member hears it, with an id.
fn heard(from: &mut Chat, text: &str) -> Line {
    let Ok(lines) = from.say_sealed("#api", text, None) else {
        panic!("not sealed")
    };
    let mut line = lines.into_iter().next().unwrap();
    line.source = Some("alice!u@h".into());
    line.tags
        .push(("msgid".into(), Some("01jbx7abcdef".into())));
    line
}

fn last(chat: &Chat) -> Entry {
    chat.conversation("#api")
        .and_then(|conversation| conversation.entries.last())
        .cloned()
        .unwrap()
}

#[test]
fn sends_only_the_placeholder_and_the_sealed_tag() {
    let mut alice = chat("alice", vec![key()]);
    let line = heard(&mut alice, "the key is ||sk_test_1|| and `make || true`");
    assert_eq!(line.params[1], "the key is [sealed] and `make || true`");
    assert!(line.tag(TAG).is_some());
    assert!(!line.to_string().contains("sk_test_1"), "{line}");
}

#[test]
fn shows_a_part_sealed_until_revealed_and_hides_it_again() {
    let mut alice = chat("alice", vec![key()]);
    let mut bob = chat("bob", vec![key()]);
    bob.receive(heard(&mut alice, "the key is ||sk_test_1||"));
    let entry = last(&bob);
    assert_eq!(
        bob.sealed_parts(&entry),
        [SealedPart {
            number: 1,
            state: SealedState::Hidden,
            text: SEALED.into()
        }]
    );
    assert_eq!(
        bob.sealed_text(&entry, "the key is [sealed]"),
        "the key is 🔒 sealed"
    );

    bob.toggle_sealed(&entry);
    assert_eq!(
        bob.sealed_text(&entry, "the key is [sealed]"),
        "the key is 🔒 sk_test_1"
    );
    bob.toggle_sealed(&entry);
    assert_eq!(bob.sealed_parts(&entry)[0].state, SealedState::Hidden);
    bob.toggle_sealed(&entry);
    bob.hide_sealed();
    assert_eq!(bob.sealed_parts(&entry)[0].state, SealedState::Hidden);
}

#[test]
fn says_what_it_cant_open() {
    let mut alice = chat("alice", vec![key()]);
    let line = heard(&mut alice, "the key is ||sk_test_1||");

    let mut keyless = chat("carol", Vec::new());
    keyless.receive(line.clone());
    let entry = last(&keyless);
    assert_eq!(keyless.sealed_parts(&entry)[0].state, SealedState::NoKey);
    // Revealing what doesn't open shows nothing more.
    keyless.toggle_sealed(&entry);
    assert_eq!(
        keyless.sealed_text(&entry, "the key is [sealed]"),
        format!("the key is 🔒 {SEALED_NO_KEY}")
    );

    // Moved to another channel, it doesn't open.
    let mut moved = line;
    moved.params[0] = "#ops".into();
    let mut dave = chat("dave", vec![key()]);
    dave.receive(":dave!u@h JOIN #ops".parse().unwrap());
    dave.receive(moved);
    let entry = dave
        .conversation("#ops")
        .and_then(|conversation| conversation.entries.last())
        .cloned()
        .unwrap();
    assert_eq!(dave.sealed_parts(&entry)[0].state, SealedState::Failed);
}

#[test]
fn never_sends_a_part_it_cant_seal() {
    let mut keyless = chat("carol", Vec::new());
    let say = Command::Say("the key is ||sk_test_1||".into());
    assert_eq!(
        keyless.run(&here("#api"), say.clone()),
        Ran::Note(NO_KEY_TO_SEAL.into())
    );

    let mut randomless = chat("dave", vec![key()]);
    randomless.random = no_random;
    assert_eq!(
        randomless.run(&here("#api"), say.clone()),
        Ran::Note(CANT_SEAL_HERE.into())
    );

    // Only a new message or a reply seals: a correction, /me and /msg don't.
    let mut alice = chat("alice", vec![key()]);
    let editing = Here {
        editing: Some("01jbx7abcdef"),
        ..here("#api")
    };
    assert_eq!(
        alice.run(&editing, say),
        Ran::Note(SEALS_ONLY_SAYING.into())
    );
    assert_eq!(
        alice.run(&here("#api"), Command::Me("hands over ||sk_1||".into())),
        Ran::Note(SEALS_ONLY_SAYING.into())
    );
    // Too long for one message.
    let long = format!("||sk_1|| {}", "words ".repeat(200));
    assert_eq!(
        alice.run(&here("#api"), Command::Say(long)),
        Ran::Note(SEALS_IN_ONE.into())
    );
}

#[test]
fn replies_with_a_sealed_part() {
    let mut alice = chat("alice", vec![key()]);
    let replying = Here {
        replying: Some("01jbx7abcdef"),
        ..here("#api")
    };
    let Ran::Replied(lines) = alice.run(&replying, Command::Say("it's ||sk_1||".into())) else {
        panic!("not a reply")
    };
    assert_eq!(lines[0].tag(REPLY), Some("01jbx7abcdef"));
    assert!(lines[0].tag(TAG).is_some());
}
