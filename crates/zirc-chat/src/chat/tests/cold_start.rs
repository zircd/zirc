//! Starting again, an app has no conversations but what the server's history gives back: an
//! agent session of ours that answered long ago, further back than the machine's latest
//! page, is looked for further back while its machine lists it as live (the user's choice,
//! 23 Nov 2026: "ask the server"; a reviewer's weekend with 14 agents, 22 Nov 2026: agents
//! "dropped out of the conversation list").

use super::*;
use zirc_proto::session::Live;

/// What `me-mbp` says of its live sessions, in #api.
fn live(sessions: &[&str]) -> String {
    let message = Message {
        level: Some(zirc_proto::Level::Debug),
        ..Message::new(
            zirc_proto::NO_SESSION,
            Live {
                sessions: sessions.iter().map(|&s| s.to_owned()).collect(),
                ..Default::default()
            }
            .event(),
        )
    };
    zirc("me-mbp", &message)
}

/// `session` on `me-mbp` writing to us `minutes` before noon, in the history batch `batch`.
fn wrote(batch: &str, session: &str, minutes: i64, n: usize) -> String {
    let at = noon() - chrono::Duration::minutes(minutes);
    let message = Message::new(
        session,
        Event::Text {
            body: format!("line {n}"),
        },
    );
    format!(
        "@batch={batch};time={};msgid={batch}-{n} :{} PRIVMSG me :{}",
        stamp(at),
        source("me-mbp"),
        zirc_proto::encode(&message).unwrap()
    )
}

/// A batch of `me-mbp`'s history: `count` lines from `session`, a minute apart, the newest
/// `from` minutes before noon.
fn page(batch: &str, session: &str, from: i64, count: usize) -> Vec<String> {
    let mut lines = vec![format!(":s BATCH +{batch} chathistory me-mbp")];
    lines.extend(
        (0..count)
            .rev()
            .map(|n| wrote(batch, session, from + i64::try_from(n).unwrap(), n)),
    );
    lines.push(format!(":s BATCH -{batch}"));
    lines
}

fn fed(chat: &mut Chat, lines: &[String]) -> Vec<String> {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    feed(chat, &lines)
}

/// Starting again with our machine in #api, its history 200 lines of `cc-busy`.
fn started() -> Scene {
    let mut chat = Scene::in_api();
    let sent = feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            ":s BATCH +t draft/chathistory-targets",
            ":s CHATHISTORY TARGETS me-mbp 2026-10-30T12:00:00.000Z",
            ":s BATCH -t",
        ],
    );
    assert_eq!(sent, ["CHATHISTORY LATEST me-mbp * 200"]);
    chat
}

#[test]
fn a_live_session_further_back_than_the_latest_page_is_looked_for() {
    let mut chat = started();
    let sent = fed(&mut chat, &page("h", "cc-busy", 0, 200));
    assert!(sent.is_empty(), "a screenful is in: {sent:?}");
    // The machine says cc-old is live too, which nothing loaded is with.
    let sent = feed(&mut chat, &[&live(&["cc-busy", "cc-old"])]);
    assert_eq!(sent, ["CHATHISTORY BEFORE me-mbp msgid=h-199 1000"]);
    // Three days back, it answered us: it's listed, and nothing more is asked.
    let mut older = page("o", "cc-busy", 200, 10);
    older.insert(1, wrote("o", "cc-old", 3 * 24 * 60, 0));
    let sent = fed(&mut chat, &older);
    assert!(sent.is_empty(), "{sent:?}");
    assert!(chat.conversation("me-mbp/cc-old").is_some());
    assert!(
        chat.direct_rows()
            .iter()
            .any(|row| row.name == "me-mbp/cc-old")
    );
}

#[test]
fn the_latest_page_looks_further_once_the_machine_has_said_whats_live() {
    let mut chat = started();
    // The machine says first, before its history is in.
    assert!(feed(&mut chat, &[&live(&["cc-busy", "cc-old"])]).is_empty());
    let sent = fed(&mut chat, &page("h", "cc-busy", 0, 200));
    assert_eq!(sent, ["CHATHISTORY BEFORE me-mbp msgid=h-199 1000"]);
}

#[test]
fn looking_stops_after_five_pages_or_thirty_days() {
    let mut chat = started();
    fed(&mut chat, &page("h", "cc-busy", 0, 200));
    let mut sent = feed(&mut chat, &[&live(&["cc-busy", "cc-gone"])]);
    let mut asked = 0;
    let mut from = 200;
    while let [line] = sent.as_slice() {
        assert!(line.starts_with("CHATHISTORY BEFORE me-mbp"), "{line}");
        asked += 1;
        sent = fed(
            &mut chat,
            &page(&format!("p{asked}"), "cc-busy", from, 1000),
        );
        from += 1000;
    }
    assert_eq!(asked, 5, "{sent:?}");

    // Held back to more than thirty days ago, it doesn't look further.
    let mut chat = started();
    fed(&mut chat, &page("h", "cc-busy", 31 * 24 * 60, 200));
    assert!(feed(&mut chat, &[&live(&["cc-busy", "cc-gone"])]).is_empty());
}
