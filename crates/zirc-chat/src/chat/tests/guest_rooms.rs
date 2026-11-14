//! Guests' rooms under their channels, for owners and admins: listed from zirc.dev's kept
//! guests, those not waiting on us folded, suggestions hidden on request, and joined only
//! when opened.

use super::*;
use crate::chat::{GuestRooms, KeptGuest, kept_guests_path, parse_kept_guests};

fn kept(nick: &str, name: Option<&str>, channels: &[&str], connected: bool) -> KeptGuest {
    KeptGuest {
        rooms: Vec::new(),
        nick: nick.into(),
        name: name.map(str::to_owned),
        channels: channels.iter().map(|c| (*c).to_owned()).collect(),
        connected,
    }
}

/// An admin in `#web` and `#api`.
fn admin() -> Chat {
    let mut chat = chat();
    connected(&mut chat);
    feed(&mut chat, &[":me!u@h JOIN #web"]);
    chat.set_role("admin");
    chat
}

fn guests_of(chat: &Chat, channel: &str) -> GuestRooms {
    chat.channel_groups()
        .into_iter()
        .find(|g| g.name == channel)
        .map(|g| g.guests)
        .unwrap_or_default()
}

fn labels(rooms: &GuestRooms) -> Vec<(&str, &str)> {
    rooms
        .rooms
        .iter()
        .map(|room| (room.name.as_str(), room.label.as_str()))
        .collect()
}

#[test]
fn reads_zirc_devs_kept_guests() {
    let json = r##"{"guests": [
        {"nick": "guest-21042", "name": "Sam", "kind": "guest", "email": null,
         "channels": ["#web", "#web/guest-21042"], "lastSeen": "2026-11-08T10:00:00Z",
         "connected": true, "banned": false, "conversations": []},
        {"nick": "guest-7k2q", "name": null, "channels": []}
    ], "settings": {"replyText": true}}"##;
    let guests = parse_kept_guests(json).unwrap();
    assert_eq!(
        guests,
        [
            kept(
                "guest-21042",
                Some("Sam"),
                &["#web", "#web/guest-21042"],
                true
            ),
            kept("guest-7k2q", None, &[], false),
        ]
    );
    // Its public channel and its room are the one room.
    assert_eq!(guests[0].rooms(), ["#web/guest-21042"]);
    assert_eq!(kept_guests_path("acme"), "/api/organisations/acme/guests");
}

#[test]
fn a_guest_has_a_room_beside_each_public_channel_never_anothers() {
    let guest = kept(
        "Guest-21042",
        None,
        &[
            "#Support",
            "#web/guest-21042",
            "#web/guest-99999",
            "#api/login",
            "me",
        ],
        false,
    );
    assert_eq!(guest.rooms(), ["#support/guest-21042", "#web/guest-21042"]);
}

#[test]
fn owners_and_admins_see_every_kept_guests_room_under_its_channel() {
    let mut chat = admin();
    chat.set_kept_guests(vec![
        kept("guest-1111", Some("Guest 1111"), &["#web"], false),
        kept("guest-2222", Some("Sam"), &["#web"], false),
        kept("guest-3333", Some("Ana"), &["#web/guest-3333"], true),
        kept("guest-4444", None, &["#support"], false),
    ]);
    let web = guests_of(&chat, "#web");
    // Connected first, then as zirc.dev listed them; by the name they gave, else their nick.
    assert_eq!(
        labels(&web),
        [
            ("#web/guest-3333", "Ana"),
            ("#web/guest-1111", "guest-1111"),
            ("#web/guest-2222", "Sam"),
        ]
    );
    assert!(web.rooms.iter().all(|room| !room.joined));
    // Folded under #web's row by default: with nothing new in any, none shows.
    assert!(web.folded);
    assert_eq!(web.shown().count(), 0);
    // A channel we aren't in is listed for its guests, with nothing of its own to open.
    let support = chat
        .channel_groups()
        .into_iter()
        .find(|g| g.name == "#support")
        .unwrap();
    assert!(chat.conversation("#support").is_none());
    assert_eq!(
        labels(&support.guests),
        [("#support/guest-4444", "guest-4444")]
    );
    // Nothing joined: nothing to hear from them.
    assert!(chat.conversation("#web/guest-2222").is_none());
}

#[test]
fn members_see_no_rooms_they_didnt_join() {
    let mut chat = admin();
    chat.set_role("member");
    chat.set_kept_guests(vec![kept("guest-2222", Some("Sam"), &["#web"], true)]);
    assert!(guests_of(&chat, "#web").rooms.is_empty());
    assert!(!chat.channel_groups().iter().any(|g| g.name == "#support"));
    // A room they joined is still under its channel, apart from its threads.
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web/guest-2222",
            ":me!u@h JOIN #web/login",
            ":guest-2222!u@h JOIN #web/guest-2222",
        ],
    );
    let group = chat
        .channel_groups()
        .into_iter()
        .find(|g| g.name == "#web")
        .unwrap();
    assert_eq!(group.rooms, ["#web/login"]);
    assert_eq!(labels(&group.guests), [("#web/guest-2222", "Sam")]);
    assert!(group.guests.rooms[0].joined && group.guests.rooms[0].connected);
}

#[test]
fn opening_a_guests_room_joins_it() {
    let mut chat = admin();
    chat.set_kept_guests(vec![kept("guest-2222", Some("Sam"), &["#web"], false)]);
    let lines: Vec<String> = chat
        .open("#web/guest-2222")
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(lines, ["JOIN #web/guest-2222"]);
    feed(&mut chat, &[":me!u@h JOIN #web/guest-2222"]);
    let web = guests_of(&chat, "#web");
    assert_eq!(labels(&web), [("#web/guest-2222", "Sam")]);
    assert!(web.rooms[0].joined);
    // Listed once, and never as one of the channel's threads.
    assert!(chat.threads("#web").is_empty());
}

#[test]
fn many_guests_rooms_fold_but_what_is_new_still_shows() {
    let mut chat = admin();
    let guests: Vec<KeptGuest> = (0..7)
        .map(|n| kept(&format!("guest-{n}{n}{n}{n}"), None, &["#web"], false))
        .collect();
    chat.set_kept_guests(guests);
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web/guest-3333",
            "@time=2026-10-30T11:00:00.000Z :guest-3333!u@h PRIVMSG #web/guest-3333 :hello?",
        ],
    );
    let web = guests_of(&chat, "#web");
    assert_eq!(web.rooms.len(), 7);
    // Folded, what's new still shows: the room the guest wrote in.
    assert!(web.folded);
    let shown: Vec<&str> = web.shown().map(|room| room.name.as_str()).collect();
    assert_eq!(shown, ["#web/guest-3333"]);

    chat.set_guest_rooms_open("#WEB", true);
    let web = guests_of(&chat, "#web");
    assert!(!web.folded);
    assert_eq!(web.shown().count(), 7);
}

#[test]
fn a_guest_room_listed_under_its_channel_is_bold_for_itself_not_the_channel() {
    let mut chat = admin();
    chat.set_kept_guests(vec![kept("guest-2222", Some("Sam"), &["#web"], true)]);
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web/guest-2222",
            "@time=2026-10-30T11:00:00.000Z :guest-2222!u@h PRIVMSG #web/guest-2222 :me: hi",
        ],
    );
    let rows = chat.sidebar_list(&ListOptions::default());
    let row = rows
        .iter()
        .find(|row| row.name == "#web" && row.kind == ListKind::Conversation)
        .unwrap();
    let room = rows
        .iter()
        .find(|row| row.name == "#web/guest-2222" && row.kind == ListKind::GuestRoom)
        .unwrap();
    assert!(room.unread);
    assert!(!row.unread);
    assert_eq!(row.badge, 0);
}

#[test]
fn a_guest_sees_their_own_room_as_ever() {
    let mut chat = Chat::new(
        "guest-2222",
        "irc.zirc.test",
        Vec::new(),
        Options::default(),
    );
    chat.clock = noon;
    chat.connected(Link {
        nick: "guest-2222".into(),
        ..link(true)
    });
    // Without an account, as the server's extended-join says: a guest.
    feed(&mut chat, &[":guest-2222!u@h JOIN #web/guest-2222 * :Sam"]);
    let groups = chat.channel_groups();
    assert!(groups.iter().all(|g| g.guests.rooms.is_empty()));
}

#[test]
fn a_guest_away_is_listed_by_their_own_room_once_the_server_forgot_where_they_were() {
    let mut chat = admin();
    let away = KeptGuest {
        rooms: vec!["#web/guest-21042".into()],
        ..kept("guest-21042", Some("Sam"), &[], false)
    };
    chat.set_kept_guests(vec![away]);
    assert_eq!(
        labels(&guests_of(&chat, "#web")),
        [("#web/guest-21042", "Sam")]
    );
    assert_eq!(
        parse_kept_guests(
            r##"{"guests": [{"nick": "guest-12345", "rooms": ["#web/guest-12345"]}]}"##
        )
        .unwrap()[0]
            .rooms(),
        ["#web/guest-12345"]
    );
}

#[test]
fn a_suggestion_hides_behind_the_fold_until_something_new_or_its_opened() {
    let mut chat = admin();
    chat.set_kept_guests(vec![kept("guest-2222", Some("Sam"), &["#web"], true)]);
    assert!(chat.is_suggestion("#web/guest-2222"));
    chat.close_direct("#web/guest-2222");
    // Hidden, it's out of the list, the channel's guests open or not.
    chat.set_guest_rooms_open("#web", true);
    assert!(guests_of(&chat, "#web").rooms.is_empty());
    // Kept for next time, as closed direct messages are.
    assert!(chat.closed().iter().any(|c| c.name == "#web/guest-2222"));

    // Opened, it's joined, and no longer a suggestion: it's left, not hidden.
    let _ = chat.open("#web/guest-2222");
    feed(&mut chat, &[":me!u@h JOIN #web/guest-2222"]);
    assert!(!chat.is_suggestion("#web/guest-2222"));
    assert!(guests_of(&chat, "#web").rooms[0].joined);
}

#[test]
fn a_member_has_no_suggestions_nor_does_a_guest() {
    let mut chat = admin();
    chat.set_role("member");
    feed(&mut chat, &[":me!u@h JOIN #web/guest-2222"]);
    assert!(!chat.is_suggestion("#web/guest-2222"));
    assert!(!chat.is_suggestion("#web"));
    assert!(!chat.is_suggestion("dana"));
}

#[test]
fn a_room_we_left_whose_guest_is_gone_leaves_the_list_for_an_admin_and_a_member() {
    const ROOM: &str = "#web/guest-21042";
    let listed = |chat: &Chat| {
        chat.sidebar_list(&ListOptions::default())
            .iter()
            .any(|row| row.name == ROOM && row.kind == ListKind::GuestRoom)
    };
    for admin in [true, false] {
        let mut chat = self::admin();
        if !admin {
            chat.set_role("member");
        }
        // zirc.dev keeps Sam, gone now; we're in their room, which they've left.
        chat.set_kept_guests(vec![kept("guest-21042", Some("Sam"), &["#web"], false)]);
        feed(
            &mut chat,
            &[
                &format!(":me!u@h JOIN {ROOM}"),
                &format!(":guest-21042!u@h JOIN {ROOM} * :Sam"),
                &format!(":guest-21042!u@h PRIVMSG {ROOM} :thanks, bye"),
                &format!(":guest-21042!u@h PART {ROOM}"),
            ],
        );
        let _ = chat.select(chat.position(ROOM).unwrap());
        let _ = chat.select(chat.position("#web").unwrap());
        assert!(listed(&chat), "in it, it's listed");

        // Leave, as the menu's Leave does, and the server says we've gone.
        let parted = chat.part(ROOM, None);
        assert_eq!(texts(&parted), [format!("PART {ROOM}")]);
        feed(&mut chat, &[&format!(":me!u@h PART {ROOM}")]);
        assert!(
            !listed(&chat),
            "admin {admin}: left, it's gone from the list"
        );
        // Hidden too: not even with #web's guests open, till Sam writes again.
        chat.set_guest_rooms_open("#web", true);
        assert!(!listed(&chat), "admin {admin}: open, it stays gone");
    }
}

#[test]
fn a_channels_row_folds_its_guests_rooms_and_rolls_up_whats_new_in_them() {
    let mut chat = admin();
    let head = |chat: &Chat| {
        chat.sidebar_list(&ListOptions::default())
            .into_iter()
            .find(|row| row.name == "#web" && row.kind == ListKind::Conversation)
            .unwrap()
    };
    // No guests: nothing to fold.
    assert_eq!(head(&chat).open, None);
    chat.set_kept_guests(vec![kept("guest-2222", Some("Sam"), &["#web"], true)]);
    // Folded by default, on #web's own row: no row of its own for the fold.
    assert_eq!(head(&chat).open, Some(false));
    let rows = chat.sidebar_list(&ListOptions::default());
    assert!(!rows.iter().any(|row| row.kind == ListKind::GuestRoom));
    chat.set_guest_rooms_open("#web", true);
    assert_eq!(head(&chat).open, Some(true));
    assert!(
        chat.sidebar_list(&ListOptions::default())
            .iter()
            .any(|row| row.name == "#web/guest-2222")
    );
}
