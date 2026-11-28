//! Stepping in ([`Chat::desk_menu`]): a working Office row's ⋯ offers Message, Pause (Resume
//! while paused) to its own person, and Open; a paused session's row says so.

use std::collections::BTreeMap;

use zirc_proto::session::{Away, Live};

use super::office::{office, past_noon, utc};
use super::*;

/// What machine `nick` says of its sessions: `live` online, `away` away since 11:58, and
/// `paused` paused.
fn said(nick: &str, live: &[&str], away: &[&str], paused: &[&str]) -> String {
    let message = Message {
        level: Some(zirc_proto::Level::Debug),
        ..Message::new(
            zirc_proto::NO_SESSION,
            Live {
                sessions: live.iter().map(|&s| s.to_owned()).collect(),
                away: away
                    .iter()
                    .map(|&s| {
                        let since = past_noon(-2);
                        (s.to_owned(), Away { since, waiting: 0 })
                    })
                    .collect(),
                paused: paused
                    .iter()
                    .map(|&s| (s.to_owned(), past_noon(-1)))
                    .collect::<BTreeMap<_, _>>(),
            }
            .event(),
        )
    };
    zirc(nick, &message)
}

/// Each item of `menu`: what it does, its words, the line under them, whether it can be
/// pressed and what it opens.
fn items(menu: &DeskMenu) -> Vec<(DeskAction, &str, &str, bool, Option<&str>)> {
    menu.items
        .iter()
        .map(|item| {
            (
                item.action,
                item.title.as_str(),
                item.under.as_str(),
                item.enabled,
                item.opens.as_deref(),
            )
        })
        .collect()
}

#[test]
fn our_own_sessions_row_offers_message_pause_and_open() {
    let mut scene = office();
    scene.hears(&[&said("me-mbp", &["cc-9"], &[], &[])]);
    let office = scene.office(utc());
    assert_eq!(office.hot_desks[0].menu, "Step in with cc-9");
    let menu = scene.desk_menu("me-mbp/cc-9").unwrap();
    assert_eq!(menu.name, "cc-9");
    assert_eq!(
        items(&menu),
        [
            (
                DeskAction::Message,
                "Message cc-9",
                "It hears it on its watch, between steps",
                true,
                Some("me-mbp/cc-9")
            ),
            (
                DeskAction::Pause,
                "Pause",
                "Stops at a safe point and waits for you",
                true,
                None
            ),
            (
                DeskAction::Open,
                "Open the conversation",
                "What it's said and done today",
                true,
                Some("me-mbp/cc-9")
            ),
        ]
    );

    // Pause is one line to its machine, which only its owner's zircd takes.
    let lines: Vec<String> = scene
        .pause_desk("me-mbp/cc-9", true)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(lines.len(), 1);
    let body = lines[0].strip_prefix("PRIVMSG me-mbp :").unwrap();
    let message = zirc_proto::decode(body).unwrap().unwrap();
    assert_eq!(
        zirc_proto::pause::Pause::of(&message.event),
        Some(zirc_proto::pause::Pause {
            session: "cc-9".into(),
            paused: true,
        })
    );
    assert_eq!(message.level, Some(zirc_proto::Level::Debug));
}

#[test]
fn pause_is_greyed_while_no_watch_is_open() {
    let mut scene = office();
    scene.hears(&[&said("me-mbp", &[], &["cc-9"], &[])]);
    let menu = scene.desk_menu("me-mbp/cc-9").unwrap();
    assert_eq!(
        items(&menu)[1],
        (
            DeskAction::Pause,
            "Pause",
            "Not watching zirc now, so it can't hear a pause",
            false,
            None
        )
    );
}

#[test]
fn someone_elses_session_offers_no_pause() {
    let mut scene = office();
    scene.hears(&[&said("henrik-mbp", &["cc-1", "cc-2"], &[], &[])]);
    // John's desk, where henrik's cc-1 sits: Message reaches John, Open the session.
    let john = scene.desk_menu("john").unwrap();
    assert_eq!(
        (john.name.as_str(), john.under.as_str()),
        ("John", "Fixer · henrik's Claude Code")
    );
    assert_eq!(
        items(&john)
            .iter()
            .map(|(action, _, _, _, opens)| (*action, *opens))
            .collect::<Vec<_>>(),
        [
            (DeskAction::Message, Some("john")),
            (DeskAction::Open, Some("henrik-mbp/cc-1")),
        ]
    );
    assert!(scene.pause_desk("john", true).is_empty());
    assert!(scene.pause_desk("henrik-mbp/cc-2", true).is_empty());
    // An empty desk has no menu.
    assert!(scene.desk_menu("noor").is_none());
    let office = scene.office(utc());
    assert_eq!(
        office.coworkers[0].menu.as_deref(),
        Some("Step in with John")
    );
    assert_eq!(office.coworkers[1].menu, None);
}

#[test]
fn a_paused_sessions_row_says_who_paused_it_and_offers_resume() {
    let mut scene = office();
    scene.hears(&[
        &said("me-mbp", &["cc-9"], &[], &["cc-9"]),
        &said("henrik-mbp", &["cc-1"], &[], &["cc-1"]),
    ]);
    let office = scene.office(utc());
    let ours = office
        .hot_desks
        .iter()
        .find(|desk| desk.conversation == "me-mbp/cc-9")
        .unwrap();
    assert_eq!(ours.doing.as_deref(), Some("Paused by you"));
    assert_eq!(
        (office.coworkers[0].state.as_str(), office.coworkers[0].desk),
        ("Paused by henrik", DeskState::Paused)
    );
    let menu = scene.desk_menu("me-mbp/cc-9").unwrap();
    assert_eq!(
        (menu.doing.as_str(), menu.desk),
        ("Paused by you", DeskState::Paused)
    );
    assert_eq!(
        items(&menu)[1],
        (
            DeskAction::Resume,
            "Resume",
            "Carries on where it stopped",
            true,
            None
        )
    );
}
