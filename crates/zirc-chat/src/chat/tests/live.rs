//! Which agent sessions are live, as their machines say ([`Chat::session_live`]): listed
//! from their first watch, folded away once ended, and as before where a machine's zircd
//! doesn't say.

use super::*;
use crate::chat::{ListKind, ListOptions, ListRow};
use zirc_proto::session::Live;

/// What the machine says of its live sessions, as zircd sends it.
fn live(sessions: &[&str]) -> Message {
    Message {
        level: Some(zirc_proto::Level::Debug),
        ..Message::new(
            zirc_proto::NO_SESSION,
            Live {
                sessions: sessions.iter().map(|&s| s.to_owned()).collect(),
                ..Default::default()
            }
            .event(),
        )
    }
}

fn rows(chat: &Chat) -> Vec<ListRow> {
    chat.sidebar_list(&ListOptions::default())
}

fn find<'a>(rows: &'a [ListRow], name: &str) -> Option<&'a ListRow> {
    rows.iter().find(|row| row.name == name)
}

/// `#api` with our machine `me-mbp` in it.
fn with_mbp() -> Scene {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":me-mbp!u@machine/me JOIN #api"]);
    chat
}

#[test]
fn a_session_is_known_from_its_first_watch_and_listed_once_it_writes_to_us() {
    let mut chat = with_mbp();
    assert!(find(&rows(&chat), "me-mbp/cc-new").is_none());
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-new"]))]);
    assert_eq!(chat.session_live("me-mbp", "cc-new"), Some(true));
    // Live, but it hasn't written to us: no row (the user's choice, 15 Nov 2026).
    assert!(find(&rows(&chat), "me-mbp/cc-new").is_none());
    assert!(chat.agents().contains(&"me-mbp/cc-new".to_owned()));
    // The People list has it under its machine, and its card says it's here.
    let people = chat.people_rows("#api");
    let sessions: Vec<&str> = people
        .iter()
        .flat_map(|row| &row.machines)
        .flat_map(|machine| &machine.sessions)
        .map(|session| session.short.as_str())
        .collect();
    assert_eq!(sessions, ["cc-new"]);
    let header = chat.agent_header("me-mbp/cc-new").unwrap();
    assert_eq!(header.heard, "Active now");
    assert!(header.active);
    // Once it writes to us, it's a direct message.
    feed(&mut chat, &[&dm("me-mbp", "cc-new", "on it", 1)]);
    let rows = rows(&chat);
    let row = find(&rows, "me-mbp/cc-new").expect("listed");
    assert_eq!(
        (row.shelf, row.kind),
        (Shelf::Direct, ListKind::Conversation)
    );
    // The list itself is no line of the channel.
    assert!(
        !chat
            .conversation("#api")
            .unwrap()
            .entries
            .iter()
            .any(|entry| matches!(entry.kind, EntryKind::Zirc(_)))
    );
}

#[test]
fn a_session_that_ended_leaves_the_list_its_conversation_kept() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1", "cc-2"])),
            &dm("me-mbp", "cc-1", "on it", 1),
        ],
    );
    let _ = chat.mark_read("me-mbp/cc-1");
    assert!(
        find(&rows(&chat), "me-mbp/cc-1").is_some(),
        "a live session is listed, though it's quiet"
    );

    // Its watch lapsed: zircd's list no longer names it.
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-2"]))]);
    assert_eq!(chat.session_live("me-mbp", "cc-1"), Some(false));
    assert!(chat.session_ended("me-mbp/cc-1"));
    assert!(
        find(&rows(&chat), "me-mbp/cc-1").is_some(),
        "still listed while it was active within the hour"
    );
    chat.clock = || noon() + chrono::Duration::minutes(62);
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_none(), "not listed");
    assert!(
        chat.position("me-mbp/cc-1").is_some(),
        "its conversation is kept, for Jump to… and search"
    );
    assert_eq!(
        chat.agent_header("me-mbp/cc-1").unwrap().heard,
        "Ended · last heard 1h ago"
    );
    assert!(!chat.agent_header("me-mbp/cc-1").unwrap().active);
    // An ended session that never said anything leaves the list altogether.
    feed(&mut chat, &[&zirc("me-mbp", &live(&[]))]);
    assert!(find(&rows(&chat), "me-mbp/cc-2").is_none());
    // It leaves the People list too.
    assert!(
        chat.people_rows("#api")
            .iter()
            .flat_map(|row| &row.machines)
            .all(|machine| machine.sessions.is_empty())
    );
}

#[test]
fn an_ended_session_with_something_unread_stays_until_read() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "finished", 1),
            &zirc("me-mbp", &live(&[])),
        ],
    );
    chat.clock = || noon() + chrono::Duration::hours(2);
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_some());
    let _ = chat.mark_read("me-mbp/cc-1");
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_none());
}

#[test]
fn an_ended_session_is_not_at_work_whatever_it_last_took_on() {
    let mut chat = with_mbp();
    let taking = Message::new(
        "cc-1",
        Event::Text {
            body: "taking: the login bug".into(),
        },
    );
    feed(
        &mut chat,
        &[&zirc("me-mbp", &live(&["cc-1"])), &zirc("me-mbp", &taking)],
    );
    assert_eq!(chat.active_now().len(), 1);
    feed(&mut chat, &[&zirc("me-mbp", &live(&[]))]);
    assert!(chat.active_now().is_empty());
    assert!(!chat.agents().contains(&"me-mbp/cc-1".to_owned()));
}

#[test]
fn the_list_is_the_same_before_and_after_a_machine_says_which_sessions_are_live() {
    // Connected, a session of ours took something on two days ago and never said it was
    // done, and another said something ten minutes ago. Then the machine says neither is
    // live: no row comes or goes, so the list doesn't flash at start-up.
    let mut chat = with_mbp();
    let taking = Message::new(
        "cc-old",
        Event::Text {
            body: "taking: the login bug".into(),
        },
    );
    feed(
        &mut chat,
        &[
            &format!("@time=2026-10-28T09:00:00.000Z {}", zirc("me-mbp", &taking)),
            &dm("me-mbp", "cc-new", "on it", 0),
        ],
    );
    let _ = chat.mark_read("me-mbp/cc-new");
    chat.clock = || noon() + chrono::Duration::minutes(10);
    let agents = |chat: &Chat| -> Vec<String> {
        rows(chat)
            .into_iter()
            .filter(|row| row.direct_kind == Some(DirectKind::Agent))
            .map(|row| row.name)
            .collect()
    };
    let before = agents(&chat);
    assert_eq!(before, ["me-mbp/cc-new"]);
    feed(&mut chat, &[&zirc("me-mbp", &live(&[]))]);
    assert_eq!(agents(&chat), before);
    // An hour after it was last active, it goes.
    chat.clock = || noon() + chrono::Duration::minutes(61);
    assert!(agents(&chat).is_empty());
}

#[test]
fn a_machine_that_left_ends_its_sessions_after_the_grace() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            "@time=2026-10-30T12:00:00.000Z :me-mbp!u@machine/me QUIT :restarting",
        ],
    );
    // A zircd restarting is back well within it.
    chat.clock = || noon() + chrono::Duration::seconds(60);
    assert_eq!(chat.session_live("me-mbp", "cc-1"), Some(true));
    chat.clock = || noon() + chrono::Duration::minutes(3);
    assert_eq!(chat.session_live("me-mbp", "cc-1"), Some(false));
    // Back, saying its list again, it's here.
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            &zirc("me-mbp", &live(&["cc-1"])),
        ],
    );
    assert_eq!(chat.session_live("me-mbp", "cc-1"), Some(true));
}

#[test]
fn reconnecting_counts_a_list_until_its_machine_says_it_again() {
    let mut chat = with_mbp();
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-1"]))]);
    chat.disconnected("gone");
    connected(&mut chat);
    assert_eq!(chat.session_live("me-mbp", "cc-1"), Some(true));
    chat.clock = || noon() + chrono::Duration::minutes(3);
    assert_eq!(chat.session_live("me-mbp", "cc-1"), Some(false));
}

#[test]
fn a_machine_that_never_says_folds_its_sessions_as_before() {
    let mut chat = with_mbp();
    feed(&mut chat, &[&dm("me-mbp", "cc-1", "on it", 1)]);
    let _ = chat.mark_read("me-mbp/cc-1");
    assert_eq!(chat.session_live("me-mbp", "cc-1"), None);
    assert!(
        find(&rows(&chat), "me-mbp/cc-1").is_some(),
        "lately said: listed"
    );
    assert_eq!(
        chat.agent_header("me-mbp/cc-1").unwrap().heard,
        "Last heard just now"
    );
}

#[test]
fn a_session_chip_knows_the_machine_that_runs_it() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            ":dana-x1!u@machine/dana JOIN #api",
            &zirc("dana-x1", &live(&["cc-dana01"])),
        ],
    );
    // The machine that says it's live, wherever it's named.
    assert_eq!(
        chat.session_machine("cc-dana01", None).as_deref(),
        Some("dana-x1")
    );
    assert_eq!(
        chat.session_machine("cc-dana01", Some("me-mbp")).as_deref(),
        Some("dana-x1")
    );
    // One no machine says it runs: the machine whose conversation it's named in, as zircd's
    // notices to its person name its sessions; nothing anywhere else.
    assert_eq!(
        chat.session_machine("cc-gone01", Some("me-mbp")).as_deref(),
        Some("me-mbp")
    );
    assert_eq!(
        chat.session_machine("cc-gone01", Some("me-mbp/cc-other"))
            .as_deref(),
        Some("me-mbp")
    );
    assert_eq!(chat.session_machine("cc-gone01", Some("#api")), None);
    assert_eq!(chat.session_machine("cc-gone01", None), None);
    // The chip carries it.
    let blocks = chat.flat_in(
        "me-mbp",
        crate::chat::rich("wakes of cc-dana01 and cc-gone01"),
    );
    let [crate::chat::RichBlock::Paragraph { spans }] = blocks.as_slice() else {
        panic!("one paragraph: {blocks:?}");
    };
    let machines: Vec<Option<&str>> = spans
        .iter()
        .filter_map(|span| match &span.kind {
            crate::chat::RichSpanKind::Session { machine, .. } => Some(machine.as_deref()),
            _ => None,
        })
        .collect();
    assert_eq!(machines, [Some("dana-x1"), Some("me-mbp")]);
}

#[test]
fn an_ended_session_says_so_and_folds_ten_minutes_later_unless_unread() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "done, have a look", 1),
        ],
    );
    // Its machine stops listing it at 12:02: ended then.
    feed(
        &mut chat,
        &[&format!(
            "@time=2026-10-30T12:02:00.000Z {}",
            zirc("me-mbp", &live(&[]))
        )],
    );
    assert_eq!(
        chat.session_ended_at("me-mbp", "cc-1"),
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 12, 2, 0).unwrap())
    );
    chat.clock = || Utc.with_ymd_and_hms(2026, 10, 30, 12, 5, 0).unwrap();
    let listed = rows(&chat);
    let row = find(&listed, "me-mbp/cc-1").expect("listed while unread");
    // Where its time was, it says it ended; ours, it goes by its name alone.
    assert_eq!(row.when.as_deref(), Some(crate::chat::live::ENDED));
    assert!(!row.title.contains(" · "), "{}", row.title);
    // Unread, it stays past the ten minutes; read, it folds.
    chat.clock = || Utc.with_ymd_and_hms(2026, 10, 30, 12, 13, 0).unwrap();
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_some());
    let _ = chat.mark_read("me-mbp/cc-1");
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_none());
    // Listed again, it lives on, with no end.
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-1"]))]);
    assert_eq!(chat.session_ended_at("me-mbp", "cc-1"), None);
}

/// What we said to `session` on our machine `me-mbp`, at 11:`minute`, as the machine's
/// history gives it back after the app starts again.
fn said_to(session: &str, text: &str, minute: u32) -> String {
    format!(
        "@time=2026-10-30T11:{minute:02}:00.000Z;msgid=me{minute} :me!u@h PRIVMSG me-mbp :{session} {text}"
    )
}

#[test]
fn a_session_we_wrote_to_is_listed_after_starting_again_though_it_never_answered() {
    // The iPhone's app started again, as iOS ends it in the background: what we said to a
    // session that hasn't answered comes back in the machine's history, and the session
    // dropped out of the list once the machine said it's live (a reviewer's weekend with
    // 14 agents, 22 Nov 2026: "agents I'd already messaged dropped out of the conversation
    // list, so I had to search to find them again").
    let mut chat = with_mbp();
    feed(&mut chat, &[&said_to("cc-scout", "are you there?", 58)]);
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-scout"]))]);
    let listed = rows(&chat);
    let row = find(&listed, "me-mbp/cc-scout").expect("listed, as before starting again");
    assert_eq!(row.shelf, Shelf::Direct);
    assert_eq!(
        said(chat.conversation("me-mbp/cc-scout").unwrap()),
        ["<me> are you there?"]
    );
    assert!(said(chat.conversation("me-mbp").unwrap()).is_empty());

    // The same when the machine says it first and the history comes after.
    let mut chat = with_mbp();
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-scout"]))]);
    feed(&mut chat, &[&said_to("cc-scout", "are you there?", 58)]);
    assert_eq!(
        said(
            chat.conversation("me-mbp/cc-scout")
                .expect("its own conversation")
        ),
        ["<me> are you there?"]
    );
    // What we said to everyone on the machine stays the machine's.
    feed(&mut chat, &[&said_to("everyone", "stop", 59)]);
    assert_eq!(
        said(chat.conversation("me-mbp").unwrap()),
        ["<me> everyone stop"]
    );
}
