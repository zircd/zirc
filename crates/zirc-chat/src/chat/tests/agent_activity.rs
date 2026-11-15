//! An agent session's conversation, from a scene of one machine of ours with two sessions
//! and a teammate's agent.

use super::*;

/// A chat with `me-mbp` (ours) running `checkout` (`cc-1`) and `docs` (`cc-2`), and
/// `bob-laptop` running `cc-9`, in `#api` and its agents room.
fn scene() -> Chat {
    let mut chat = Scene::in_api().into_chat();
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            ":bob-laptop!u@machine/bob JOIN #api",
            ":bob!u@h JOIN #api bob :Bob Lee",
            ":me!u@h JOIN #api/agents",
        ],
    );
    chat
}

fn named(session: &str, name: &str, event: Event) -> Message {
    Message {
        name: Some(name.into()),
        ..Message::new(session, event)
    }
}

fn text(body: &str) -> Event {
    Event::Text { body: body.into() }
}

/// `message` from `nick` in `room`, said at `time` on the day of the scene.
fn say(room: &str, nick: &str, time: &str, message: &Message) -> String {
    format!(
        "@time=2026-10-30T{time}.000Z @msgid=m-{time} {}",
        line_from(nick, room, message)
    )
}

/// `message` from `nick` as a private message to us.
fn dm_from(nick: &str, time: &str, message: &Message) -> String {
    format!(
        "@time=2026-10-30T{time}.000Z @msgid=d-{time} :{} PRIVMSG me :{}",
        source(nick),
        zirc_proto::encode(message).unwrap()
    )
}

fn landing(session: &str, text: &str) -> Message {
    Message::new(
        "ci",
        Event::Custom {
            kind: zirc_proto::ci::LANDING_KIND.into(),
            data: serde_json::json!({ "session": session, "landing": "monorepo/7", "state": "passed" }),
            text: text.into(),
        },
    )
}

fn commit(subject: &str) -> Event {
    Event::Commit {
        branch: "fix".into(),
        sha: "a".repeat(40),
        subject: subject.into(),
        paths: vec!["web/src".into()],
        repo: None,
    }
}

/// Who said what where: the room's name, or `dm`, and the entry's words.
fn lines(chat: &Chat, name: &str) -> Vec<(String, String)> {
    chat.agent_activity(name)
        .into_iter()
        .map(|a| (a.room.unwrap_or_else(|| "dm".into()), entry_text(&a.entry)))
        .collect()
}

fn hear_checkout(chat: &mut Chat) {
    let checkout = |event| named("cc-1", "checkout", event);
    let docs = |event| named("cc-2", "docs", event);
    feed(
        chat,
        &[
            &say(
                "#api/agents",
                "me-mbp",
                "10:00:00",
                &checkout(text("taking: the login page")),
            ),
            &say(
                "#api/agents",
                "me-mbp",
                "10:05:00",
                &docs(text("taking: the docs")),
            ),
            &say("#api", "me-mbp", "10:10:00", &checkout(commit("Fix login"))),
            // Another machine's session with the same id is another session.
            &say(
                "#api/agents",
                "bob-laptop",
                "10:12:00",
                &named("cc-1", "x", text("not ours")),
            ),
            &say(
                "#api",
                "zirc-ci",
                "10:20:00",
                &landing("cc-1", "cc-1: landed fix as abc"),
            ),
            &say(
                "#api",
                "zirc-ci",
                "10:21:00",
                &landing("cc-2", "cc-2: landed docs as def"),
            ),
            &dm_from("me-mbp", "10:30:00", &checkout(text("which page?"))),
            ":me!u@h PRIVMSG me-mbp :/agent cc-1 the login page",
            // A working set is never an entry.
            &say(
                "#api/agents",
                "me-mbp",
                "10:40:00",
                &working("cc-1", &["web/src"]),
            ),
        ],
    );
}

#[test]
fn an_agents_conversation_merges_its_rooms_and_dms_and_leaves_other_sessions_out() {
    let mut chat = scene();
    hear_checkout(&mut chat);
    let mut got = lines(&chat, "me-mbp/cc-1");
    // Our own `/agent` line is echoed with the time of arrival; it's ours, last.
    let ours = got.pop().unwrap();
    assert_eq!(ours.0, "dm");
    assert_eq!(ours.1, "<me> the login page");
    assert_eq!(
        got,
        [
            (
                "#api/agents".into(),
                "[post] checkout: taking: the login page".into()
            ),
            (
                "#api".into(),
                "[commit] checkout: Fix login  (fix aaaaaaa)".into()
            ),
            ("#api".into(), "[post] CI: cc-1: landed fix as abc".into()),
            ("dm".into(), "[post] checkout: which page?".into()),
        ]
    );
    // Not an agent conversation: nothing.
    assert!(chat.agent_activity("#api").is_empty());
    assert!(chat.agent_activity("bob").is_empty());
}

#[test]
fn activity_counts_no_unread_and_the_dm_counts_only_the_agents_answers() {
    let mut chat = scene();
    hear_checkout(&mut chat);
    let dm = chat.conversation("me-mbp/cc-1").unwrap();
    assert_eq!(
        dm.entries.len(),
        2,
        "only the DMs are the conversation's own"
    );
    assert_eq!(dm.unread, 1, "the answer, not the room's five lines");
}

#[test]
fn a_teammates_agent_is_read_only_with_a_button_to_its_owner() {
    let chat = scene();
    assert!(chat.can_message("me-mbp/cc-1"));
    assert!(chat.can_message("bob"));
    assert!(!chat.can_message("bob-laptop/cc-9"));
    assert_eq!(chat.read_only_note("me-mbp/cc-1"), None);
    assert_eq!(
        chat.read_only_note("bob-laptop/cc-9"),
        Some(ReadOnlyNote {
            text: "Only Bob Lee can message cc-9".into(),
            button: "Message Bob Lee".into(),
            dm: "bob".into(),
        })
    );
}

#[test]
fn a_machine_whose_host_says_no_owner_says_how_to_make_it_someones() {
    let mut chat = scene();
    // Not logged in through zirc.dev: its host says nothing of whose it is, whatever its name.
    feed(&mut chat, &[":s 354 me 42 h me-pi H 0 :me"]);
    assert!(!chat.can_message("me-pi/cc-2"));
    let note = chat.read_only_note("me-pi/cc-2").unwrap();
    assert_eq!(
        note.text,
        "me-pi doesn't say whose it is, so it can't hear you: run `zirc login` on it"
    );
    assert!(note.button.is_empty() && note.dm.is_empty(), "{note:?}");
}

#[test]
fn a_conversation_with_nothing_yet_says_so_and_has_an_intro() {
    let mut chat = scene();
    feed(
        &mut chat,
        &[&dm_from(
            "me-mbp",
            "09:00:00",
            &named("cc-3", "reviews", text("hello")),
        )],
    );
    assert_eq!(chat.agent_empty("me-mbp/cc-3"), None, "its DM is something");
    assert_eq!(
        chat.agent_empty("me-mbp/cc-4").as_deref(),
        Some("Nothing from cc-4 yet. What it says, changes and lands shows here.")
    );
    assert_eq!(chat.agent_empty("bob"), None);
    let intro = chat.agent_intro("me-mbp/cc-3").unwrap();
    assert_eq!(
        (intro.title.as_str(), intro.session.as_str()),
        ("reviews", "cc-3")
    );
    assert_eq!(chat.agent_intro("bob"), None);
    // Where it runs comes from what it said, titled or not: its tool and its machine.
    feed(
        &mut chat,
        &[&dm_from(
            "me-mbp",
            "09:05:00",
            &Message {
                tool: Some(zirc_proto::CODEX.into()),
                ..named("cc-5", "search", text("hi"))
            },
        )],
    );
    assert_eq!(
        chat.agent_intro("me-mbp/cc-5").unwrap().place.as_deref(),
        Some("Codex · me-mbp")
    );
}

#[test]
fn our_machines_thread_is_our_agents_with_their_glyph_and_sessions() {
    let mut chat = scene();
    hear_checkout(&mut chat);
    feed(
        &mut chat,
        &[":me!u@h PRIVMSG me-mbp :/agent hello everyone"],
    );
    assert_eq!(chat.title("me-mbp"), "Your agents · me-mbp");
    assert_eq!(chat.short_title("me-mbp"), "every agent on me-mbp");
    // A teammate's machine is not ours.
    assert_eq!(chat.title("bob-laptop"), "bob · bob-laptop");
    assert!(chat.is_machine_thread("me-mbp"));
    assert!(!chat.is_machine_thread("bob-laptop"));
    let sessions: Vec<_> = chat
        .machine_sessions("me-mbp")
        .into_iter()
        .map(|s| (s.conversation, s.label))
        .collect();
    assert_eq!(
        sessions,
        [
            ("me-mbp/cc-1".into(), "checkout".into()),
            ("me-mbp/cc-2".into(), "docs".into())
        ]
    );
    // Each row's Now line, for a session that never registered a title card too.
    let nows: Vec<_> = chat
        .machine_sessions("me-mbp")
        .into_iter()
        .map(|s| s.now)
        .collect();
    assert_eq!(
        nows,
        [
            Some("⟶ the login page · 2h".to_owned()),
            Some("⟶ the docs · 1h".to_owned())
        ]
    );
    // Each with its face as its row draws it: the session's, on our machine.
    let faces: Vec<_> = chat
        .machine_sessions("me-mbp")
        .into_iter()
        .map(|s| s.face)
        .collect();
    let rows: Vec<_> = ["me-mbp/cc-1", "me-mbp/cc-2"]
        .iter()
        .map(|name| chat.face(&chat.agent(name).unwrap().avatar))
        .collect();
    assert_eq!(faces, rows);
    assert!(matches!(
        &faces[0],
        crate::chat::Face::Agent { session: Some(session), .. } if session == "cc-1"
    ));
    assert!(chat.machine_sessions("bob-laptop").is_empty());
}

#[test]
fn a_line_from_a_machine_itself_is_headed_with_its_name_and_the_agents_glyph() {
    let mut chat = scene();
    feed(
        &mut chat,
        &[
            &dm_from(
                "me-mbp",
                "09:00:00",
                &Message::new(
                    zirc_proto::NO_SESSION,
                    text("all agents: pause before landing"),
                ),
            ),
            &say(
                "#api",
                "bob-laptop",
                "09:01:00",
                &Message::new(zirc_proto::NO_SESSION, text("heads up")),
            ),
            &dm_from("me-mbp", "09:02:00", &named("cc-1", "checkout", text("ok"))),
        ],
    );
    let head = |room: &str, at: usize| {
        let entry = chat.conversation(room).unwrap().entries[at].clone();
        chat.speaker(&entry)
    };
    // Our machine, bare: its name, not "your agent", with the agents' glyph.
    let ours = head("me-mbp", 0);
    assert_eq!(ours.name, "me-mbp");
    assert_eq!(ours.detail.as_deref(), Some("your machine"));
    assert_eq!(ours.avatar, Chat::agents_avatar());
    assert!(!ours.is_app, "never an APP tag");
    // A teammate's.
    let at = chat
        .conversation("#api")
        .unwrap()
        .entries
        .iter()
        .position(|e| e.from == "bob-laptop")
        .expect("bob-laptop's line");
    let theirs = head("#api", at);
    assert_eq!(
        (theirs.name.as_str(), theirs.detail.as_deref()),
        ("bob-laptop", Some("Bob Lee's machine"))
    );
    assert_eq!(theirs.avatar, Chat::agents_avatar());
    // A session's line is unchanged.
    let session = head("me-mbp/cc-1", 0);
    assert_eq!(
        (session.name.as_str(), session.detail.as_deref()),
        ("checkout", Some("your agent"))
    );
    // And the thread's preview follows the rule.
    assert_eq!(
        chat.preview("me-mbp").unwrap().text,
        "me-mbp: all agents: pause before landing"
    );
    assert_eq!(chat.sender_name("me-mbp", None), "me-mbp");
}

#[test]
fn a_header_has_the_now_line_for_an_untitled_session_too() {
    let mut chat = scene();
    hear_checkout(&mut chat);
    // `checkout` never registered a title card, yet its header says what it's doing.
    let header = chat.agent_header("me-mbp/cc-1").unwrap();
    assert_eq!(header.now, "⟶ the login page · 2h");
    assert_eq!(header.heard, "Active now");
    assert!(header.active);
    // Every app's one line: at work, what it's doing.
    assert_eq!(header.line, "⟶ the login page · 2h");
    // One that said nothing: nothing in progress, and its line when it was heard, with no
    // "Your agent": its AGENT tag says it.
    let quiet = chat.agent_header("me-mbp/cc-9").unwrap();
    assert_eq!(
        (quiet.now.as_str(), quiet.heard.as_str(), quiet.active),
        ("Nothing in progress", "Not heard yet", false)
    );
    assert_eq!(quiet.line, "Not heard yet");
    // A teammate's says whose it is first.
    assert_eq!(
        chat.agent_header("bob-laptop/cc-2").unwrap().line,
        "Bob Lee's agent · Not heard yet"
    );
    assert_eq!(chat.agent_header("bob"), None);
}
