use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::chat::{Here, Link, Options, Ran};
use crate::command::Command;
use crate::sealed::{DeviceKey, KEY_LEN};

fn key() -> Key {
    Key::new("acme", [7; KEY_LEN])
}

/// Bytes that differ from call to call, as random ones would: xorshift's, from a seed the
/// tests share.
fn counting(bytes: &mut [u8]) -> bool {
    static STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    for byte in bytes {
        let mut x = STATE.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        STATE.store(x, Ordering::Relaxed);
        *byte = x.to_le_bytes()[0];
    }
    true
}

/// `nick` in #api on acme's server, holding `keys`, on a server that echoes.
fn chat(nick: &str, keys: Vec<Key>) -> Chat {
    chat_on(nick, Some("acme"), keys)
}

/// `nick` on `org`'s server (none told for `None`), holding `keys`.
fn chat_on(nick: &str, org: Option<&str>, keys: Vec<Key>) -> Chat {
    let mut chat = Chat::new(nick, "irc.zirc.test", Vec::new(), Options::default());
    chat.random = counting;
    if let Some(org) = org {
        chat.set_keys(org, keys);
    }
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

#[test]
fn draws_a_sealed_part_as_a_chip_apps_reveal_and_copy() {
    let mut alice = chat("alice", vec![key()]);
    let mut bob = chat("bob", vec![key()]);
    bob.receive(heard(&mut alice, "the key is ||sk_test_1||, keep it"));
    let line = |bob: &Chat| {
        bob.lines("#api", chrono::FixedOffset::east_opt(0).unwrap(), false)
            .into_iter()
            .find(|line| line.text.starts_with("the key is"))
            .unwrap()
    };
    let chip = |line: &crate::chat::lines::Line| {
        let mut chips = Vec::new();
        RichBlock::each_span(&line.rich, &mut |span| {
            if let Some(sealed) = &span.sealed {
                chips.push((span.text.clone(), sealed.clone()));
            }
        });
        chips
    };

    // Plain words though it is, it comes as spans, its chip among them.
    let drawn = line(&bob);
    assert_eq!(drawn.text, "the key is 🔒 sealed, keep it");
    assert_eq!(
        chip(&drawn),
        [(
            format!("{LOCK} {SEALED}"),
            SealedSpan {
                message: "01jbx7abcdef".into(),
                number: 1,
                state: SealedState::Hidden
            }
        )]
    );
    // Copy takes the value without revealing it.
    assert_eq!(
        bob.sealed_value("01jbx7abcdef", 1).as_deref(),
        Some("sk_test_1")
    );
    assert_eq!(bob.sealed_value("01jbx7abcdef", 2), None);
    assert_eq!(bob.sealed_numbers("01jbx7abcdef"), [1]);

    bob.toggle_sealed_in("01jbx7abcdef");
    let drawn = line(&bob);
    assert_eq!(chip(&drawn)[0].0, "🔒 sk_test_1");
    assert_eq!(chip(&drawn)[0].1.state, SealedState::Revealed);

    // Without the key, it's said so, and Copy has nothing.
    let mut keyless = chat("carol", Vec::new());
    keyless.receive(heard(&mut alice, "the key is ||sk_test_1||"));
    assert_eq!(chip(&line(&keyless))[0].1.state, SealedState::NoKey);
    assert_eq!(keyless.sealed_value("01jbx7abcdef", 1), None);
}

fn in_general(chat: &mut Chat) -> Vec<Line> {
    let me = chat.me().to_owned();
    chat.receive(format!(":{me}!u@h JOIN #general").parse().unwrap())
}

/// `line`, from `source`, as the server passes it on, with the account it says.
fn from(mut line: Line, source: &str, account: Option<&str>) -> Line {
    line.source = Some(source.to_owned());
    if let Some(account) = account {
        line.tags.push(("account".into(), Some(account.into())));
    }
    line
}

fn tagged(lines: Vec<Line>, tag: &str) -> Option<Line> {
    lines.into_iter().find(|line| line.tag(tag).is_some())
}

#[test]
fn a_new_device_is_handed_the_key_by_one_holding_it() {
    use zirc_proto::sealed::{ASK_TAG, GRANT_TAG};
    let mut alice = chat("alice", vec![key()]);
    in_general(&mut alice);
    let mut bob = chat("bob", Vec::new());
    bob.set_device(DeviceKey::from_bytes([3; 32]).unwrap());

    // Joining the organisation's channel without a key, it asks.
    let ask = tagged(in_general(&mut bob), ASK_TAG).expect("an ask");
    assert_eq!(ask.command, "TAGMSG");
    assert_eq!(ask.params, ["#general"]);
    let answer =
        tagged(alice.receive(from(ask, "bob!u@h", Some("bob"))), GRANT_TAG).expect("an answer");
    assert_eq!(answer.params, ["bob"]);
    // The key itself, the 43 characters after `zirc-key1-acme-`: longer than any nonce, whose
    // test bytes can say the same as a stretch of it.
    assert!(!answer.to_string().contains(&key().export()[15..58]));

    bob.receive(from(answer, "alice!u@h", Some("alice")));
    assert_eq!(bob.take_received_keys(), [key()]);
    assert!(bob.can_seal());
    assert!(bob.take_received_keys().is_empty());
    // It says so, quietly, where it's looked at.
    let said = bob
        .conversations
        .iter()
        .flat_map(|conversation| &conversation.entries)
        .any(|entry| format!("{:?}", entry.kind).contains("Sealed parts open here now"));
    assert!(said);
}

#[test]
fn hands_the_key_only_to_a_members_device_that_asked() {
    use zirc_proto::sealed::{ASK_TAG, GRANT_TAG};
    let mut alice = chat("alice", vec![key()]);
    in_general(&mut alice);
    let ask_of = |nick: &str| {
        let mut asker = chat(nick, Vec::new());
        asker.set_device(DeviceKey::from_bytes([3; 32]).unwrap());
        tagged(in_general(&mut asker), ASK_TAG).unwrap()
    };
    // Not signed in to an account, or a machine: no answer.
    for (nick, source, account) in [
        ("dan", "dan!u@h", None),
        ("ada-mbp", "ada-mbp!u@machine/ada", Some("ada-mbp")),
    ] {
        let answered = alice.receive(from(ask_of(nick), source, account));
        assert!(tagged(answered, GRANT_TAG).is_none(), "{nick}");
    }
    // Asked for a key that isn't the newest: only the newest is handed out.
    let mut older: Line = ask_of("eve");
    let mut ask: zirc_proto::sealed::Ask =
        serde_json::from_str(older.tag(ASK_TAG).unwrap()).unwrap();
    ask.k = Some(Key::new("acme", [9; KEY_LEN]).id());
    older.tags = vec![(ASK_TAG.into(), Some(serde_json::to_string(&ask).unwrap()))];
    assert!(
        tagged(
            alice.receive(from(older, "eve!u@h", Some("eve"))),
            GRANT_TAG
        )
        .is_none()
    );

    // An answer nobody asked for is passed over: a key can't be pressed on a device.
    let mut carol = chat("carol", Vec::new());
    let device = DeviceKey::from_bytes([4; 32]).unwrap();
    carol.set_device(device.clone());
    let value = crate::sealed::grant(&key(), &device.public(), [5; 32], [6; 12]).unwrap();
    let mut pressed = Line::new("TAGMSG", ["carol"]);
    pressed.tags = vec![(GRANT_TAG.into(), Some(value))];
    carol.receive(from(pressed, "mallory!u@h", Some("mallory")));
    assert!(carol.take_received_keys().is_empty());
    assert!(!carol.can_seal());
}

#[test]
fn takes_a_key_only_from_a_members_device_and_only_the_one_it_asked_for() {
    use zirc_proto::sealed::{ASK_TAG, GRANT_TAG};
    // Carol holds an older key, so joining asks for nothing; a part sealed with one she lacks
    // makes her ask for that one, in #general.
    let mut carol = chat("carol", vec![Key::new("acme", [8; KEY_LEN])]);
    let device = DeviceKey::from_bytes([4; 32]).unwrap();
    carol.set_device(device.clone());
    let mut alice = chat("alice", vec![key()]);
    assert!(tagged(in_general(&mut carol), ASK_TAG).is_none());
    assert!(tagged(carol.receive(heard(&mut alice, "||sk_test_1||")), ASK_TAG).is_some());
    let granted = |key: &Key, source: &str, account: Option<&str>| {
        let value = crate::sealed::grant(key, &device.public(), [5; 32], [6; 12]).unwrap();
        let mut line = Line::new("TAGMSG", ["carol"]);
        line.tags = vec![(GRANT_TAG.into(), Some(value))];
        from(line, source, account)
    };
    // While she's asking, a machine's or an unsigned answer is passed over: anyone
    // in the channel saw her device's key, and could encrypt a key of their own to it.
    for (source, account) in [
        ("ada-mbp!u@machine/ada", Some("ada-mbp")),
        ("dan!u@h", None),
    ] {
        carol.receive(granted(&Key::new("acme", [9; KEY_LEN]), source, account));
        assert!(carol.take_received_keys().is_empty(), "{source}");
    }
    // A member's, but not the key she asked for: passed over too.
    carol.receive(granted(
        &Key::new("acme", [9; KEY_LEN]),
        "bob!u@h",
        Some("bob"),
    ));
    assert!(carol.take_received_keys().is_empty());
    // The one she asked for, by its id, but labelled another organisation's: passed over, or
    // a member here could make it the key she seals that organisation's messages with.
    carol.receive(granted(
        &Key::new("globex", [7; KEY_LEN]),
        "bob!u@h",
        Some("bob"),
    ));
    assert!(carol.take_received_keys().is_empty());
    // The one she asked for, from a member: taken.
    carol.receive(granted(&key(), "bob!u@h", Some("bob")));
    assert_eq!(carol.take_received_keys(), [key()]);
}

#[test]
fn asks_for_the_key_a_part_was_sealed_with() {
    use zirc_proto::sealed::ASK_TAG;
    let mut alice = chat("alice", vec![key()]);
    let mut bob = chat("bob", Vec::new());
    in_general(&mut bob);
    bob.set_device(DeviceKey::from_bytes([3; 32]).unwrap());
    let asked = bob.receive(heard(&mut alice, "the key is ||sk_test_1||"));
    let ask = tagged(asked, ASK_TAG).expect("an ask for its key");
    let ask: zirc_proto::sealed::Ask = serde_json::from_str(ask.tag(ASK_TAG).unwrap()).unwrap();
    assert_eq!(ask.k, Some(key().id()));
    // Once a connection.
    assert!(tagged(bob.receive(heard(&mut alice, "again ||sk_2||")), ASK_TAG).is_none());
}

#[test]
fn holds_hands_out_and_takes_only_its_own_organisations_keys() {
    use zirc_proto::sealed::{ASK_TAG, GRANT_TAG};
    // On globex's server, holding acme's key among globex's: it seals with globex's alone.
    let globex = Key::new("globex", [2; KEY_LEN]);
    let mut there = chat_on("alice", Some("globex"), vec![key(), globex.clone()]);
    in_general(&mut there);
    let mut asker = chat_on("bob", Some("globex"), Vec::new());
    asker.set_device(DeviceKey::from_bytes([3; 32]).unwrap());
    let ask = tagged(in_general(&mut asker), ASK_TAG).expect("an ask");
    let answer =
        tagged(there.receive(from(ask, "bob!u@h", Some("bob"))), GRANT_TAG).expect("an answer");
    asker.receive(from(answer, "alice!u@h", Some("alice")));
    assert_eq!(asker.take_received_keys(), [globex]);
}

#[test]
fn a_chat_never_told_its_organisation_hands_out_and_takes_no_key() {
    use zirc_proto::sealed::{ASK_TAG, GRANT_TAG};
    // Told no organisation, it asks for nothing and answers nobody, whatever keys the device
    // holds.
    let mut erin = chat_on("erin", None, vec![key()]);
    let device = DeviceKey::from_bytes([4; 32]).unwrap();
    assert!(tagged(erin.set_device(device.clone()), ASK_TAG).is_none());
    assert!(tagged(in_general(&mut erin), ASK_TAG).is_none());
    let mut staff = chat("dana", Vec::new());
    staff.set_device(DeviceKey::from_bytes([3; 32]).unwrap());
    let ask = tagged(in_general(&mut staff), ASK_TAG).expect("an ask");
    assert!(tagged(erin.receive(from(ask, "dana!u@h", Some("dana"))), GRANT_TAG).is_none());
    // Nor does it take a key pressed on it.
    let value = crate::sealed::grant(&key(), &device.public(), [5; 32], [6; 12]).unwrap();
    let mut pressed = Line::new("TAGMSG", ["erin"]);
    pressed.tags = vec![(GRANT_TAG.into(), Some(value))];
    erin.receive(from(pressed, "dana!u@h", Some("dana")));
    assert!(erin.take_received_keys().is_empty());
}
