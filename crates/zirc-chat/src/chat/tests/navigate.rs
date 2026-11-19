//! Moving through the conversations, as every app does: Jump to's rows, Next and Previous
//! conversation, and Next unread.

use super::*;
use crate::chat::{JUMP_ROWS, ListOptions, SidebarSection};

/// Something said in `channel` at `minute` past noon, by bob.
fn said_at(channel: &str, minute: u32, text: &str) -> String {
    format!("@time=2026-11-06T12:{minute:02}:00.000Z :bob!u@h PRIVMSG {channel} :{text}")
}

/// `#api`, `#docs` and `#web` joined, `#api` shown.
fn three() -> Chat {
    let mut chat = Scene::in_api().into_chat();
    feed(&mut chat, &[":me!u@h JOIN #docs", ":me!u@h JOIN #web"]);
    let _ = chat.open("#api");
    chat
}

#[test]
fn jump_to_lists_at_most_twenty_whatever_is_typed() {
    let mut chat = Scene::in_api().into_chat();
    let joins: Vec<String> = (0..25).map(|n| format!(":me!u@h JOIN #c{n:02}")).collect();
    feed_all(&mut chat, &joins);
    assert_eq!(JUMP_ROWS, 20);
    assert_eq!(chat.switcher("c").len(), JUMP_ROWS);
    assert_eq!(chat.switcher("").len(), JUMP_ROWS);
    assert_eq!(chat.switcher("  ").len(), JUMP_ROWS);
    // Best first: the ones starting with it.
    assert_eq!(chat.switcher("c1")[..2], ["#c10", "#c11"]);
}

#[test]
fn jump_to_with_nothing_typed_lists_the_most_recent_first() {
    let mut chat = three();
    feed(
        &mut chat,
        &[
            &said_at("#docs", 1, "older"),
            &said_at("#web", 5, "newer"),
            &said_at("#docs", 3, "between"),
        ],
    );
    // #web, then #docs, then #api, where nothing's been said, by what it shows.
    assert_eq!(chat.switcher(""), ["#web", "#docs", "#api"]);
    // Typed, as it ranks: what starts with it.
    assert_eq!(chat.switcher("d"), ["#docs"]);
}

#[test]
fn jump_to_finds_an_agent_session_by_what_it_is_taking_on() {
    let mut chat = Scene::in_api().into_chat();
    let task = Message::new(
        "cc-1",
        Event::Text {
            body: "taking: the coupon field".into(),
        },
    );
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            &format!("@time=2026-10-30T11:00:00.000Z {}", zirc("me-mbp", &task)),
            &dm("me-mbp", "cc-1", "on it", 1),
        ],
    );
    assert_eq!(chat.switcher("coupon"), ["me-mbp/cc-1"]);
}

#[test]
fn next_and_previous_conversation_go_round_the_ends() {
    let mut chat = three();
    let channels = ListOptions {
        tab: Some(SidebarSection::Channels),
        ..ListOptions::default()
    };
    let next = |chat: &Chat, by| chat.next_conversation(&channels, by).unwrap();
    // Down the sidebar: the channels A–Z, the Server channel not among them.
    assert_eq!(next(&chat, 1), "#docs");
    assert_eq!(next(&chat, -1), "#web");
    let _ = chat.open("#web");
    // Past the end, back to the top; and up from the top, to the end.
    assert_eq!(next(&chat, 1), "#api");
    // From the Server channel, which has no row, to either end.
    chat.show_server();
    assert_eq!(next(&chat, -1), "#web");
    assert_eq!(next(&chat, 1), "#api");
}

#[test]
fn next_conversation_passes_over_a_channel_we_left() {
    let mut chat = three();
    feed(&mut chat, &[":me!u@h PART #docs"]);
    let next = chat.next_conversation(&ListOptions::default(), 1);
    assert_eq!(next.as_deref(), Some("#web"));
}

#[test]
fn next_unread_goes_to_what_mentions_us_first_round_the_ends() {
    let mut chat = three();
    feed(
        &mut chat,
        &[
            &said_at("#docs", 1, "a change"),
            &said_at("#web", 2, "me: a question"),
        ],
    );
    // #docs is next down, but #web mentions us.
    assert_eq!(chat.next_unread(1).as_deref(), Some("#web"));
    assert_eq!(chat.next_unread(-1).as_deref(), Some("#web"));
    let _ = chat.open("#web");
    // Then what's unread, round the end from #web.
    assert_eq!(chat.next_unread(1).as_deref(), Some("#docs"));
    let _ = chat.open("#docs");
    assert_eq!(chat.next_unread(1), None);
    assert_eq!(chat.next_unread(-1), None);
}

#[test]
fn next_unread_goes_up_the_sidebar_for_previous_unread() {
    let mut chat = three();
    feed(
        &mut chat,
        &[
            &said_at("#docs", 1, "a change"),
            &said_at("#web", 2, "another"),
        ],
    );
    // From #api, up and round the end, #web comes before #docs.
    assert_eq!(chat.next_unread(-1).as_deref(), Some("#web"));
    assert_eq!(chat.next_unread(1).as_deref(), Some("#docs"));
}
