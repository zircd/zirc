//! Every list ordered by recency sorts by the time its rows show: a row showing `5h` was
//! listed above one showing `3m` (Android, 19 Nov 2026), sorted by what no row shows.

use super::*;

/// Each direct-message row's time as shown, in the list's order.
fn shown(chat: &Chat) -> Vec<(String, Option<String>)> {
    chat.direct_rows()
        .into_iter()
        .map(|row| (row.name, row.when))
        .collect()
}

#[test]
fn a_newer_message_lists_above_what_only_the_server_says_is_newer() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T07:00:00.000Z :bob!u@h PRIVMSG me :see you tomorrow",
            "@time=2026-10-30T11:57:00.000Z :carol!u@h PRIVMSG me :are you there?",
            // On reconnecting, the server says bob's is newer, for something no row shows.
            ":s BATCH +t draft/chathistory-targets",
            ":s CHATHISTORY TARGETS bob 2026-10-30T11:59:00.000Z",
            ":s BATCH -t",
        ],
    );
    assert_eq!(
        shown(&chat),
        [
            ("carol".to_owned(), Some("3m".to_owned())),
            ("bob".to_owned(), Some("5h".to_owned())),
        ]
    );
    // Before any of a conversation's history is here, the server's word orders it.
    feed(
        &mut chat,
        &[
            ":s BATCH +u draft/chathistory-targets",
            ":s CHATHISTORY TARGETS dave 2026-10-30T11:58:00.000Z",
            ":s BATCH -u",
        ],
    );
    assert_eq!(
        chat.last_said("dave"),
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 11, 58, 0).unwrap())
    );
}

#[test]
fn a_cards_live_update_moves_no_conversation_up() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            &format!(
                "@time=2026-10-30T07:00:00.000Z {}",
                posted(&stepped("running", 0, false))
            ),
            "@time=2026-10-30T07:01:00.000Z :alice!u@h PRIVMSG #api :lunch?",
            &format!(
                "@time=2026-10-30T11:59:00.000Z {}",
                posted(&stepped("running", 40, true))
            ),
        ],
    );
    let preview = chat.preview("#api").unwrap();
    assert_eq!(preview.text, "alice: lunch?");
    assert_eq!(
        chat.last_said("#api"),
        Some(preview.at),
        "the time it shows"
    );
}
