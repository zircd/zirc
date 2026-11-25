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

/// `#ops` at its default, Warnings, holding a warning at 07:00, alice's line in `#api` at
/// 09:00 and an oncall report at info at 11:59, hidden in `#ops`.
fn ops_with_a_hidden_report() -> Scene {
    let mut chat = Scene::in_api();
    let warned = Message {
        level: Some(zirc_proto::Level::Warn),
        ..Message::new(
            "ci",
            Event::Text {
                body: "master is ahead of e2e".into(),
            },
        )
    };
    let report = Message::new(
        "oncall",
        Event::Text {
            body: "report to oncall: Sentry: TypeError".into(),
        },
    );
    feed(&mut chat, &[":me!u@h JOIN #ops"]);
    let warned = line_from("zirc-alerts", "#ops", &warned).to_string();
    let report = line_from("oncall", "#ops", &report).to_string();
    feed(
        &mut chat,
        &[
            &format!("@time=2026-10-30T07:00:00.000Z {warned}"),
            "@time=2026-10-30T09:00:00.000Z :alice!u@h PRIVMSG #api :lunch?",
            &format!("@time=2026-10-30T11:59:00.000Z {report}"),
        ],
    );
    chat
}

/// `#ops`'s row: what it says under its name, how, and its time.
fn ops_row(chat: &Chat) -> (Option<(String, UnderKind)>, Option<String>) {
    let rows = chat.sidebar_list(&ListOptions::default());
    let row = rows.iter().find(|row| row.name == "#ops").unwrap().clone();
    (row.under.map(|under| (under.text, under.kind)), row.when)
}

/// Where the channel called `name` is in the list of channels.
fn channel_at(chat: &Chat, name: &str) -> Option<usize> {
    chat.list(SidebarSection::Channels)
        .iter()
        .position(|&at| chat.conversations()[at].name == name)
}

/// A line below a channel's threshold is hidden in it, so its row neither previews it nor
/// sorts by it: `#ops` showed an oncall report at info, hidden in `#ops` at Warnings, as its
/// preview (25 Nov 2026).
#[test]
fn a_row_previews_and_sorts_by_only_what_its_threshold_shows() {
    let mut chat = ops_with_a_hidden_report();
    assert_eq!(chat.threshold("#ops"), zirc_proto::Level::Warn);
    assert_eq!(
        ops_row(&chat),
        (
            Some(("CI: master is ahead of e2e".to_owned(), UnderKind::Said)),
            Some("5h".to_owned())
        )
    );
    assert_eq!(
        chat.last_said("#ops"),
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 7, 0, 0).unwrap())
    );
    assert!(channel_at(&chat, "#api") < channel_at(&chat, "#ops"));
    // Info: the report shows, so it's previewed and moves #ops up.
    chat.set_threshold("#ops", zirc_proto::Level::Info);
    assert_eq!(
        ops_row(&chat).0.map(|(text, _)| text),
        Some("oncall: report to oncall: Sentry: TypeError".to_owned())
    );
    assert!(channel_at(&chat, "#ops") < channel_at(&chat, "#api"));
}

/// With nothing in it at or above its threshold, a row says so, faint, with no time, and
/// sorts by nothing, not by what the server's `TARGETS` counts of the hidden lines (the
/// user's choice, 25 Nov 2026).
#[test]
fn a_row_with_all_of_it_hidden_says_nothing_at_this_level() {
    let mut chat = ops_with_a_hidden_report();
    chat.set_threshold("#ops", zirc_proto::Level::Error);
    assert_eq!(
        ops_row(&chat),
        (
            Some((
                crate::chat::NOTHING_AT_THIS_LEVEL.to_owned(),
                UnderKind::Nothing
            )),
            None
        )
    );
    assert_eq!(chat.last_said("#ops"), None);
    assert!(channel_at(&chat, "#api") < channel_at(&chat, "#ops"));
}
