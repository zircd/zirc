//! Which agent sessions are online, away or ended, as their machines say
//! ([`Chat::listening`]): listed from their first watch, online strictly while watching,
//! saying when they aren't listening with messages waiting, folded away once their task
//! lands or after 12 hours away, and as before where a machine's zircd doesn't say.

use super::*;
use crate::chat::{ListKind, ListOptions, ListRow};
use crate::chat::{Listening, UnderKind};
use zirc_proto::session::{Away, Live};

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

/// What the machine says when `away` have no watch open, each since `since` minutes past
/// noon with `waiting` messages for it, and `live` do.
fn away(live: &[&str], away: &[(&str, u32, u32)]) -> Message {
    let at = |minute: u32| (noon() + chrono::Duration::minutes(minute.into())).timestamp();
    Message {
        level: Some(zirc_proto::Level::Debug),
        ..Message::new(
            zirc_proto::NO_SESSION,
            Live {
                sessions: live.iter().map(|&s| s.to_owned()).collect(),
                away: away
                    .iter()
                    .map(|&(session, since, waiting)| {
                        let since = u64::try_from(at(since)).unwrap();
                        (session.to_owned(), Away { since, waiting })
                    })
                    .collect(),
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
        "still listed within 12 hours of its last watch"
    );
    chat.clock = || noon() + chrono::Duration::minutes(62);
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_some(), "an hour on");
    chat.clock = || noon() + chrono::Duration::hours(12);
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_none(), "not listed");
    assert!(
        chat.position("me-mbp/cc-1").is_some(),
        "its conversation is kept, for Jump to… and search"
    );
    assert_eq!(
        chat.agent_header("me-mbp/cc-1").unwrap().heard,
        "Ended · last heard 11h ago"
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
    chat.clock = || noon() + chrono::Duration::hours(13);
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
    // Twelve hours after it was last active, it goes.
    chat.clock = || noon() + chrono::Duration::minutes(11 * 60 + 59);
    assert_eq!(agents(&chat), before);
    chat.clock = || noon() + chrono::Duration::hours(12);
    assert!(agents(&chat).is_empty());
}

#[test]
fn a_machine_that_left_makes_its_sessions_away_at_once() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            "@time=2026-10-30T12:00:00.000Z :me-mbp!u@machine/me QUIT :restarting",
        ],
    );
    // Nothing reaches it: away, though not ended.
    chat.clock = || noon() + chrono::Duration::seconds(60);
    assert_eq!(
        chat.listening("me-mbp", "cc-1"),
        Some(Listening::Away {
            since: noon(),
            waiting: 0
        })
    );
    assert_eq!(chat.session_live("me-mbp", "cc-1"), Some(true));
    // Back, saying its list again, it's online.
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            &zirc("me-mbp", &live(&["cc-1"])),
        ],
    );
    assert_eq!(chat.listening("me-mbp", "cc-1"), Some(Listening::Online));
}

#[test]
fn reconnecting_counts_a_list_until_its_machine_says_it_again() {
    let mut chat = with_mbp();
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-1"]))]);
    chat.disconnected("gone");
    connected(&mut chat);
    assert_eq!(chat.listening("me-mbp", "cc-1"), Some(Listening::Online));
    chat.clock = || noon() + chrono::Duration::minutes(3);
    assert!(matches!(
        chat.listening("me-mbp", "cc-1"),
        Some(Listening::Away { .. })
    ));
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
fn an_ended_session_says_so_and_folds_twelve_hours_after_its_last_watch_unless_unread() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "have a look", 1),
            // Its watch closed at 12:02.
            &zirc("me-mbp", &away(&["cc-1"], &[("cc-1", 2, 0)])),
        ],
    );
    let two = noon() + chrono::Duration::minutes(2);
    assert_eq!(chat.listened_at("me-mbp", "cc-1"), Some(two));
    // Twelve hours on, its machine no longer names it: ended, last watching at 12:02.
    chat.clock = || noon() + chrono::Duration::minutes(12 * 60 + 2);
    feed(&mut chat, &[&zirc("me-mbp", &live(&[]))]);
    assert_eq!(
        chat.listening("me-mbp", "cc-1"),
        Some(Listening::Ended { since: Some(two) })
    );
    let listed = rows(&chat);
    let row = find(&listed, "me-mbp/cc-1").expect("listed while unread");
    // Where its time was, it says it ended; ours, it goes by its name alone.
    assert_eq!(row.when.as_deref(), Some(crate::chat::ENDED));
    assert!(!row.title.contains(" · "), "{}", row.title);
    // Read, it folds.
    let _ = chat.mark_read("me-mbp/cc-1");
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_none());
    // Listed again, it lives on.
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-1"]))]);
    assert_eq!(chat.listening("me-mbp", "cc-1"), Some(Listening::Online));
    assert_eq!(chat.listened_at("me-mbp", "cc-1"), None);
}

#[test]
fn a_session_is_online_strictly_while_its_watch_is_open() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "on it", 0),
        ],
    );
    let _ = chat.mark_read("me-mbp/cc-1");
    let row = |chat: &Chat| find(&rows(chat), "me-mbp/cc-1").cloned().expect("listed");
    assert_eq!(row(&chat).presence, Some(Presence::Here));
    assert_eq!(
        chat.agent_header("me-mbp/cc-1").unwrap().heard,
        "Active now"
    );
    // Its --once watch woke: away at once, though readers from before still see it live.
    feed(
        &mut chat,
        &[&zirc("me-mbp", &away(&["cc-1"], &[("cc-1", 0, 0)]))],
    );
    assert_eq!(row(&chat).presence, Some(Presence::Away));
    assert_eq!(
        row(&chat).under.map(|under| under.kind),
        Some(UnderKind::Said),
        "nothing waits: its row previews as ever"
    );
    let header = chat.agent_header("me-mbp/cc-1").unwrap();
    assert_eq!(header.heard, "Away · last heard just now");
    assert!(!header.active);
    // Watching again, it's online.
    feed(&mut chat, &[&zirc("me-mbp", &live(&["cc-1"]))]);
    assert_eq!(row(&chat).presence, Some(Presence::Here));
}

#[test]
fn an_agent_away_with_messages_waiting_says_it_isnt_listening() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "on it", 0),
        ],
    );
    let _ = chat.mark_read("me-mbp/cc-1");
    // Away since 12:00, a message of ours waiting, at once (the user's choice, 23 Nov 2026).
    feed(
        &mut chat,
        &[&zirc("me-mbp", &away(&["cc-1"], &[("cc-1", 0, 1)]))],
    );
    let under = |chat: &Chat| {
        find(&rows(chat), "me-mbp/cc-1")
            .and_then(|row| row.under.clone())
            .expect("a line under it")
    };
    // Short, to fit a sidebar's line (the user's choice, 23 Nov 2026).
    assert_eq!(
        (under(&chat).text.as_str(), under(&chat).kind),
        ("Away · 1 waiting", UnderKind::NotListening)
    );
    chat.clock = || noon() + chrono::Duration::minutes(17);
    assert_eq!(under(&chat).text, "17m away · 1 waiting");
    // Its header says it whole.
    assert_eq!(
        chat.agent_header("me-mbp/cc-1").unwrap().heard,
        "Not listening for 17 min · 1 message waiting"
    );
    let line = chat.header_line("me-mbp/cc-1").unwrap();
    assert_eq!(
        (line.text.as_str(), line.kind),
        (
            "Not listening for 17 min · 1 message waiting",
            crate::chat::HeaderLineKind::Away
        )
    );
    // In its place even of what it's taking on.
    let taking = Message::new(
        "cc-1",
        Event::Text {
            body: "taking: the login bug".into(),
        },
    );
    feed(&mut chat, &[&zirc("me-mbp", &taking)]);
    assert_eq!(under(&chat).kind, UnderKind::NotListening);
}

#[test]
fn a_dot_is_filled_here_and_a_ring_away_or_gone_each_in_its_own_role() {
    let dots = [Presence::Here, Presence::Away, Presence::Gone].map(|presence| {
        let dot = presence.dot();
        (dot.tone, dot.hollow)
    });
    assert_eq!(
        dots,
        [
            ("presence-here".to_owned(), false),
            ("presence-away".to_owned(), true),
            ("presence-gone".to_owned(), true),
        ]
    );
}

#[test]
fn our_agent_is_named_by_its_task_until_it_says_a_name() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "on it", 0),
        ],
    );
    let title = |chat: &Chat| {
        find(&rows(chat), "me-mbp/cc-1")
            .map(|row| row.title.clone())
            .expect("listed")
    };
    assert_eq!(title(&chat), "cc-1");
    let taking = Message::new(
        "cc-1",
        Event::Text {
            body: "taking: the login page".into(),
        },
    );
    feed(&mut chat, &[&zirc("me-mbp", &taking)]);
    // Its row, its header and its composer all go by its task.
    assert_eq!(title(&chat), "the login page");
    assert_eq!(chat.title("me-mbp/cc-1"), "the login page");
    assert_eq!(chat.short_title("me-mbp/cc-1"), "the login page");
    assert_eq!(
        chat.agent_intro("me-mbp/cc-1").map(|intro| intro.title),
        Some("the login page".to_owned())
    );
    // A name it says comes first.
    let named = Message {
        name: Some("checkout".into()),
        ..Message::new(
            "cc-1",
            Event::Text {
                body: "renamed".into(),
            },
        )
    };
    feed(&mut chat, &[&zirc("me-mbp", &named)]);
    assert_eq!(title(&chat), "checkout");
}

#[test]
fn a_row_folds_once_its_task_lands_and_is_back_when_spoken_to() {
    let mut chat = with_mbp();
    let said = |session: &str, text: &str, minute: u32| {
        let message = Message::new(
            session,
            Event::Text {
                body: text.to_owned(),
            },
        );
        format!(
            "@time=2026-10-30T12:{minute:02}:00.000Z {}",
            zirc("me-mbp", &message)
        )
    };
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &said("cc-1", "taking: the login bug", 0),
            &dm("me-mbp", "cc-1", "on it", 1),
        ],
    );
    let _ = chat.mark_read("me-mbp/cc-1");
    let listed = |chat: &Chat| find(&rows(chat), "me-mbp/cc-1").is_some();
    assert!(listed(&chat));
    // zircd says done: for it once its landing lands: it folds, online or not.
    feed(&mut chat, &[&said("cc-1", "done: the login bug", 5)]);
    chat.clock = || noon() + chrono::Duration::minutes(6);
    assert!(!listed(&chat));
    assert!(chat.position("me-mbp/cc-1").is_some(), "kept for search");
    // Dropped isn't landed.
    feed(
        &mut chat,
        &[
            &said("cc-1", "taking: the logout bug", 7),
            &said("cc-1", "dropped: the logout bug", 8),
        ],
    );
    assert!(listed(&chat));
    feed(&mut chat, &[&said("cc-1", "done: the logout bug", 9)]);
    assert!(!listed(&chat));
    // Written to again, it's back.
    feed(&mut chat, &[&dm("me-mbp", "cc-1", "one more thing", 10)]);
    let _ = chat.mark_read("me-mbp/cc-1");
    assert!(listed(&chat));
}

/// A server stamps lines to the millisecond, and the `done:` line is itself something the
/// session said: its row still folds, though `done:` is kept to the second.
#[test]
fn a_row_folds_once_its_task_lands_at_a_time_with_milliseconds() {
    let mut chat = with_mbp();
    let done = Message::new(
        "cc-1",
        Event::Text {
            body: "done: the login bug".to_owned(),
        },
    );
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "on it", 1),
        ],
    );
    let _ = chat.mark_read("me-mbp/cc-1");
    feed(
        &mut chat,
        &[&format!(
            "@time=2026-10-30T12:05:00.500Z {}",
            zirc("me-mbp", &done)
        )],
    );
    chat.clock = || noon() + chrono::Duration::minutes(6);
    assert!(find(&rows(&chat), "me-mbp/cc-1").is_none());
}

#[test]
fn a_row_folds_after_twelve_hours_away() {
    let mut chat = with_mbp();
    feed(
        &mut chat,
        &[
            &zirc("me-mbp", &live(&["cc-1"])),
            &dm("me-mbp", "cc-1", "on it", 0),
            &zirc("me-mbp", &away(&[], &[("cc-1", 30, 0)])),
        ],
    );
    let _ = chat.mark_read("me-mbp/cc-1");
    let listed = |chat: &Chat| find(&rows(chat), "me-mbp/cc-1").is_some();
    chat.clock = || noon() + chrono::Duration::minutes(12 * 60 + 29);
    assert!(listed(&chat), "not after minutes");
    chat.clock = || noon() + chrono::Duration::minutes(12 * 60 + 30);
    assert!(!listed(&chat));
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
