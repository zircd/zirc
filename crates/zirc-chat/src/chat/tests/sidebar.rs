//! The sidebar as one flat list of rows ([`Chat::sidebar_list`]): its parts in order, what
//! nests under a channel, the folds and the rows that end a part.

use super::*;
use crate::chat::{ALL_MESSAGES, KeptGuest, ListKind, ListOptions, ListRow, SidebarSection};

fn all() -> ListOptions {
    ListOptions::default()
}

/// Each row as `kind shelf title`, indented by its depth.
fn lines(rows: &[ListRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            format!(
                "{}{:?} {:?} {}",
                "  ".repeat(usize::from(row.depth)),
                row.kind,
                row.shelf,
                row.title
            )
        })
        .collect()
}

fn row<'a>(rows: &'a [ListRow], name: &str) -> &'a ListRow {
    rows.iter()
        .find(|row| row.name == name)
        .unwrap_or_else(|| panic!("no row {name} in {:#?}", lines(rows)))
}

fn headings(rows: &[ListRow]) -> Vec<Shelf> {
    rows.iter()
        .filter(|row| row.kind == ListKind::Heading)
        .map(|row| row.shelf)
        .collect()
}

fn taking(session: &str, task: &str) -> Message {
    Message::new(
        session,
        Event::Text {
            body: format!("taking: {task}"),
        },
    )
}

/// An agent session's message to us from `machine`, two days ago, read.
fn quiet_agent(chat: &mut Chat, machine: &str, session: &str) {
    let old =
        dm(machine, session, "done long ago", 0).replace("2026-10-30T12:00", "2026-10-28T09:00");
    feed(chat, &[&old]);
    let _ = chat.mark_read(&format!("{machine}/{session}"));
}

/// `#api` with its room `#api/login`, `#web`, and direct messages with bob and carol, a
/// conversation with the guest Anna in `#lobby`.
fn busy() -> Chat {
    let mut chat = with_guest();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web",
            ":me!u@h JOIN #api/login",
            "@time=2026-10-30T11:05:00.000Z :carol!u@h PRIVMSG me :hey",
        ],
    );
    chat
}

#[test]
fn lists_starred_then_channels_with_guests_rooms_then_direct_messages() {
    let mut chat = busy();
    chat.star("carol", true);
    let rows = chat.sidebar_list(&all());
    assert_eq!(
        headings(&rows),
        [Shelf::Starred, Shelf::Channels, Shelf::Direct]
    );
    assert_eq!(
        lines(&rows),
        [
            "Heading Starred Starred",
            "Conversation Starred carol",
            "Heading Channels Channels",
            "Conversation Channels #api",
            "  Room Channels login",
            "Conversation Channels #lobby",
            "  GuestRoom Channels Anna",
            "Conversation Channels #web",
            "Browse Channels Browse channels",
            "NewChannel Channels New channel",
            "Heading Direct Direct messages",
            "Conversation Direct bob",
        ]
    );
    let heading = &rows[0];
    assert_eq!((heading.name.as_str(), heading.open), ("", Some(true)));
    // A direct message's row says what it's with, its dot and what was said.
    let bob = row(&rows, "bob");
    assert!(bob.direct && bob.joined && bob.opens && bob.unread);
    assert_eq!(bob.direct_kind, Some(DirectKind::Person));
    assert_eq!((bob.badge, bob.badge_label.as_deref()), (1, Some("1")));
    assert_eq!(bob.presence, Some(Presence::Gone));
    assert_eq!(bob.faces.len(), 1);
    assert_eq!(bob.preview.as_deref(), Some("hi"));
    assert!(bob.latest_ms.is_some() && bob.when.is_some());
    // A guest's room says whether it's answered, and whether it's ours.
    let anna = row(&rows, "#lobby/guest-7k2q");
    assert!(anna.guest && anna.unanswered && !anna.closed && !anna.mine);
}

#[test]
fn a_part_with_nothing_in_it_is_left_out() {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":bob!u@h PRIVMSG me :hi"]);
    let rows = chat.sidebar_list(&all());
    assert_eq!(headings(&rows), [Shelf::Channels, Shelf::Direct]);
}

#[test]
fn a_machine_running_on_another_of_ours_is_drawn_under_it() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":ci-1!u@machine/me JOIN #api me :me",
            ":build-2301!u@machine/me JOIN #api me :me on ci-1",
            ":build-77!u@machine/me JOIN #api me :me on ci-9",
            ":mbp!u@machine/me JOIN #api me :me",
            ":idle!u@machine/me JOIN #api me :me",
            // ci-1 has nothing of its own, but build-2301 under it does.
            ":build-2301!u@machine/me PRIVMSG me :tests green",
            ":build-77!u@machine/me PRIVMSG me :tests green",
            ":mbp!u@machine/me PRIVMSG me :hi",
        ],
    );
    let rows = chat.sidebar_list(&all());
    let machines: Vec<(&str, u8)> = rows
        .iter()
        .filter(|row| row.shelf == Shelf::Machines && row.kind == ListKind::Conversation)
        .map(|row| (row.name.as_str(), row.depth))
        .collect();
    // build-2301 is under ci-1; build-77's host isn't online, so it stands alone; idle,
    // with nothing under it or unread, is listed as it's connected.
    assert_eq!(
        machines,
        [
            ("build-77", 0),
            ("ci-1", 0),
            ("build-2301", 1),
            ("idle", 0),
            ("mbp", 0)
        ]
    );
    assert_eq!(
        chat.whose("build-2301"),
        Some(("me".into(), "build-2301".into()))
    );
}

#[test]
fn our_online_machines_are_under_machines_by_name_while_connected() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me-ci-2277!u@machine/me JOIN #api",
            ":me-mbp!u@machine/me JOIN #api",
            ":alice-mbp!u@machine/alice JOIN #api",
            ":bob!u@h PRIVMSG me :hi",
        ],
    );
    // Online, though with no session at work and nothing unread: listed, alice's not.
    let rows = chat.sidebar_list(&all());
    assert_eq!(
        headings(&rows),
        [Shelf::Channels, Shelf::Direct, Shelf::Machines]
    );
    assert_eq!(row(&rows, "me-mbp").shelf, Shelf::Machines);
    assert!(!rows.iter().any(|r| r.name == "alice-mbp"));
    feed(
        &mut chat,
        &[
            ":me-ci-2277!u@machine/me PRIVMSG me :deployed",
            &dm("me-mbp", "cc-1", "on it", 1),
        ],
    );
    let rows = chat.sidebar_list(&all());
    assert_eq!(
        headings(&rows),
        [Shelf::Channels, Shelf::Direct, Shelf::Machines]
    );
    let machines: Vec<(&str, &str)> = rows
        .iter()
        .filter(|row| {
            row.shelf == Shelf::Machines && row.kind == ListKind::Conversation && row.depth == 0
        })
        .map(|row| (row.name.as_str(), row.title.as_str()))
        .collect();
    assert_eq!(
        machines,
        [("me-ci-2277", "me-ci-2277"), ("me-mbp", "me-mbp")]
    );
    let ci = row(&rows, "me-ci-2277");
    assert!(ci.opens && ci.direct && ci.mine);
    assert_eq!(ci.direct_kind, Some(DirectKind::Machine));
    assert_eq!(ci.presence, Some(Presence::Here));

    // Offline once it leaves, as it's online only while it's in a channel we're in: still
    // under Machines while its thread is unread, its dot gone.
    feed(&mut chat, &[":me-ci-2277!u@machine/me QUIT :done"]);
    let rows = chat.sidebar_list(&all());
    let ci = row(&rows, "me-ci-2277");
    assert_eq!(
        (ci.shelf, ci.presence),
        (Shelf::Machines, Some(Presence::Gone))
    );
}

#[test]
fn an_online_machines_agent_sessions_are_under_it_the_quiet_ones_left_out() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            ":me-ci-9!u@machine/me JOIN #api",
            &dm("me-mbp", "cc-1", "on it", 1),
            &dm("me-laptop", "cc-2", "elsewhere", 1),
        ],
    );
    quiet_agent(&mut chat, "me-mbp", "cc-old");
    let rows = chat.sidebar_list(&all());
    let machines: Vec<String> = lines(&rows)
        .into_iter()
        .filter(|line| line.contains("Machines"))
        .collect();
    assert_eq!(
        machines,
        [
            "Heading Machines Machines",
            "Conversation Machines me-ci-9",
            "Conversation Machines me-mbp",
            "  Conversation Machines cc-1",
            // An offline machine of ours, by the session that said something.
            "Conversation Machines me-laptop",
            "  Conversation Machines cc-2",
        ]
    );
    // ci-9, with nothing under it and nothing unread, is listed as it's connected; cc-old,
    // quiet for over an hour, isn't.
    assert_eq!(row(&rows, "me-mbp/cc-1").shelf, Shelf::Machines);
    assert!(!listed(&chat, "me-mbp/cc-old"));
    // A session on a machine that isn't online is under it too.
    assert_eq!(row(&rows, "me-laptop/cc-2").shelf, Shelf::Machines);

    // Opened, the quiet one is back under its machine till something else is.
    let at = chat.position("me-mbp/cc-old").unwrap();
    let _ = chat.select(at);
    assert_eq!(row(&chat.sidebar_list(&all()), "me-mbp/cc-old").depth, 1);
    // And a machine with something unread is listed with nothing under it.
    feed(
        &mut chat,
        &[":me-ci-9!u@machine/me PRIVMSG me :build green"],
    );
    assert!(listed(&chat, "me-ci-9"));
}

/// Whether a direct message called `name` is anywhere in the sidebar.
fn listed(chat: &Chat, name: &str) -> bool {
    chat.sidebar_list(&all()).iter().any(|row| row.name == name)
}

#[test]
fn a_builds_machine_leaves_the_sidebar_a_while_after_it_goes_offline() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":build-2301!u@machine/me JOIN #api",
            ":laptop!u@machine/me JOIN #api",
            "@time=2026-10-30T11:40:00.000Z :build-2301!u@machine/me PRIVMSG me :all done",
            "@time=2026-10-30T11:40:00.000Z :laptop!u@machine/me PRIVMSG me :asleep soon",
        ],
    );
    let _ = chat.mark_read("build-2301");
    let _ = chat.mark_read("laptop");
    // Gone, with nothing unread and nothing under it, neither is listed under Machines.
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:58:00.000Z :build-2301!u@machine/me QUIT :done",
            "@time=2026-10-30T11:50:00.000Z :laptop!u@machine/me QUIT :asleep",
        ],
    );
    assert!(!listed(&chat, "build-2301"));
    assert!(!listed(&chat, "laptop"));
    chat.clock = || {
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 30, 12, 4, 0)
            .unwrap()
    };
    // With something unread, it's back.
    feed(
        &mut chat,
        &["@time=2026-10-30T12:03:00.000Z :build-2301!u@machine/me PRIVMSG me :one more thing"],
    );
    assert!(listed(&chat, "build-2301"));
    assert!(super::super::is_build_machine("build-2301"));
    assert!(super::super::is_build_machine("build-0a1b2c3d"));
    assert!(!super::super::is_build_machine("build-box"));
    assert!(!super::super::is_build_machine("buildbox"));
}

#[test]
fn a_closed_direct_message_leaves_until_something_new_is_said_or_it_is_opened() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &["@time=2026-10-30T11:00:00.000Z :bob!u@h PRIVMSG me :hi"],
    );
    let _ = chat.mark_read("bob");
    chat.close_direct("bob");
    assert!(!listed(&chat, "bob"));
    // Kept as an app keeps it, and given back.
    let closed = chat.closed();
    assert_eq!(closed[0].name, "bob");
    let mut again = Scene::in_api();
    feed(
        &mut again,
        &["@time=2026-10-30T11:00:00.000Z :bob!u@h PRIVMSG me :hi"],
    );
    let _ = again.mark_read("bob");
    again.set_closed(closed);
    assert!(!listed(&again, "bob"));
    // Something new said in it brings it back.
    feed(
        &mut chat,
        &["@time=2026-10-30T12:01:00.000Z :bob!u@h PRIVMSG me :still there?"],
    );
    assert!(listed(&chat, "bob"));
    // Opening it brings it back too, and it stays once something else is shown.
    let _ = again.open("bob");
    let _ = again.open("#api");
    assert!(listed(&again, "bob"));
    assert!(again.closed().is_empty());
}

#[test]
fn whats_starred_is_under_starred_alone_a_channel_with_its_rooms() {
    let mut chat = busy();
    chat.star("#api", true);
    chat.star("bob", true);
    chat.star("#lobby/guest-7k2q", true);
    let rows = chat.sidebar_list(&all());
    assert_eq!(
        lines(&rows),
        [
            "Heading Starred Starred",
            "Conversation Starred #api",
            "  Room Starred login",
            "Conversation Starred bob",
            "Conversation Starred Anna",
            "Heading Channels Channels",
            "Conversation Channels #lobby",
            "Conversation Channels #web",
            "Browse Channels Browse channels",
            "NewChannel Channels New channel",
            "Heading Direct Direct messages",
            "Conversation Direct carol",
        ]
    );
    assert!(row(&rows, "#api").starred);
    // A starred room is under Starred on its own, never under its channel.
    chat.star("#api", false);
    chat.star("#api/login", true);
    let rows = chat.sidebar_list(&all());
    let login: Vec<_> = rows.iter().filter(|r| r.name == "#api/login").collect();
    assert_eq!(login.len(), 1);
    assert_eq!(
        (login[0].shelf, login[0].kind, login[0].depth),
        (Shelf::Starred, ListKind::Conversation, 0)
    );
    assert_eq!(row(&rows, "#api").rooms, 1);
}

#[test]
fn a_channels_row_is_bold_for_itself_not_its_rooms_which_are_bold_for_themselves() {
    let mut chat = busy();
    feed(
        &mut chat,
        &["@time=2026-10-30T11:10:00.000Z :bob!u@h PRIVMSG #api/login :me: look"],
    );
    let rows = chat.sidebar_list(&all());
    let api = row(&rows, "#api");
    assert!(!api.unread && api.opens && api.joined);
    assert_eq!((api.badge, api.rooms), (0, 1));
    assert_eq!(api.preview.as_deref(), Some("bob: me: look"));
    let login = row(&rows, "#api/login");
    assert_eq!(
        (login.kind, login.depth, login.title.as_str()),
        (ListKind::Room, 1, "login")
    );
    assert!(login.unread);
    assert_eq!(login.badge, 1);
    // #web has nothing new.
    let web = row(&rows, "#web");
    assert!(!web.unread);
    assert_eq!((web.badge, web.badge_label.as_deref()), (0, None));

    // Something new in #api itself while another is shown, it's bold, its count its own.
    let at = chat.position("#web").unwrap();
    let _ = chat.select(at);
    feed(
        &mut chat,
        &["@time=2026-10-30T11:12:00.000Z :bob!u@h PRIVMSG #api :me: and here"],
    );
    let rows = chat.sidebar_list(&all());
    let api = row(&rows, "#api");
    assert!(api.unread);
    assert_eq!(api.badge, 1);
    // Read, it's not, though its room still is.
    let _ = chat.mark_read("#api");
    let rows = chat.sidebar_list(&all());
    assert!(!row(&rows, "#api").unread);
    assert!(row(&rows, "#api/login").unread);
}

fn kept(nick: &str, name: &str) -> KeptGuest {
    KeptGuest {
        nick: nick.into(),
        name: Some(name.into()),
        channels: vec!["#web".into()],
        rooms: Vec::new(),
        connected: false,
    }
}

#[test]
fn a_channels_guests_rooms_nest_under_it_folded_on_its_row() {
    let mut chat = busy();
    chat.set_role("admin");
    chat.set_kept_guests(vec![kept("guest-1111", "Sam")]);
    let web = |rows: &[ListRow]| rows.iter().find(|r| r.name == "#web").cloned().unwrap();
    // Folded by default, on #web's own row.
    let rows = chat.sidebar_list(&all());
    assert_eq!(web(&rows).open, Some(false));
    assert!(
        !rows
            .iter()
            .any(|r| r.kind == ListKind::GuestRoom && r.name.starts_with("#web/"))
    );
    chat.set_guest_rooms_open("#web", true);
    let rows = chat.sidebar_list(&all());
    assert_eq!(web(&rows).open, Some(true));
    let sam = row(&rows, "#web/guest-1111");
    assert_eq!(
        (sam.kind, sam.depth, sam.title.as_str()),
        (ListKind::GuestRoom, 1, "Sam")
    );
    assert!(sam.guest && sam.opens && !sam.joined);
    assert_eq!(sam.presence, Some(Presence::Gone));
    // Right under #web, before Browse channels: no row for the fold.
    let at = rows.iter().position(|r| r.name == "#web").unwrap();
    assert_eq!(rows[at + 1].name, "#web/guest-1111");
    assert_eq!(rows[at + 2].kind, ListKind::Browse);

    // However many, they fold.
    chat.set_guest_rooms_open("#web", false);
    let many: Vec<KeptGuest> = (1..=6)
        .map(|n| kept(&format!("guest-{n}{n}{n}{n}"), &format!("Guest {n}")))
        .collect();
    chat.set_kept_guests(many);
    let rows = chat.sidebar_list(&all());
    assert_eq!(web(&rows).open, Some(false));
    assert!(
        !rows
            .iter()
            .any(|r| r.kind == ListKind::GuestRoom && r.name.starts_with("#web/"))
    );
    chat.set_guest_rooms_open("#web", true);
    let rows = chat.sidebar_list(&all());
    assert_eq!(
        rows.iter()
            .filter(|r| r.kind == ListKind::GuestRoom && r.name.starts_with("#web/"))
            .count(),
        6
    );

    // Folded, a room with something new still shows, bold for itself, and its channel isn't.
    chat.set_guest_rooms_open("#web", false);
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web/guest-1111",
            "@time=2026-10-30T11:20:00.000Z :guest-1111!u@h PRIVMSG #web/guest-1111 :me: hi",
        ],
    );
    let rows = chat.sidebar_list(&all());
    let shown: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == ListKind::GuestRoom && r.name.starts_with("#web/"))
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(shown, ["#web/guest-1111"]);
    assert!(row(&rows, "#web/guest-1111").unread);
    let web = row(&rows, "#web");
    assert!(!web.unread);
    assert_eq!(web.badge, 0);

    // Open, the room is bold for itself, and its channel isn't.
    chat.set_guest_rooms_open("#web", true);
    let rows = chat.sidebar_list(&all());
    assert!(row(&rows, "#web/guest-1111").unread);
    let web = row(&rows, "#web");
    assert!(!web.unread);
    assert_eq!(web.badge, 0);
}

#[test]
fn browse_channels_then_new_channel_end_the_channels() {
    let chat = busy();
    let rows = chat.sidebar_list(&all());
    let channels: Vec<&ListRow> = rows.iter().filter(|r| r.shelf == Shelf::Channels).collect();
    // New channel is last, and there with no channel joined or listed: a server's first.
    let new = channels.last().unwrap();
    assert_eq!(
        (new.kind, new.title.as_str()),
        (ListKind::NewChannel, NEW_CHANNEL)
    );
    assert!(new.name.is_empty() && !new.opens);
    let empty = Chat::new("me", "irc.example", Vec::new(), Options::default()).sidebar_list(&all());
    let kinds: Vec<ListKind> = empty.iter().map(|r| r.kind).collect();
    assert_eq!(kinds, [ListKind::Heading, ListKind::NewChannel]);
    let browse = channels[channels.len() - 2];
    assert_eq!(browse.kind, ListKind::Browse);
    assert_eq!(browse.title, BROWSE_CHANNELS);
    assert!(browse.name.is_empty() && !browse.opens);
    assert_eq!(
        rows.iter().filter(|r| r.kind == ListKind::Browse).count(),
        1
    );

    // Every channel starred, Browse channels still ends the part.
    let mut chat = Scene::in_api();
    chat.star("#api", true);
    let rows = chat.sidebar_list(&all());
    assert_eq!(
        lines(&rows),
        [
            "Heading Starred Starred",
            "Conversation Starred #api",
            "Heading Channels Channels",
            "Browse Channels Browse channels",
            "NewChannel Channels New channel",
        ]
    );
}

#[test]
fn quiet_agent_sessions_that_ended_leave_the_direct_messages_till_opened() {
    let mut chat = busy();
    quiet_agent(&mut chat, "me-mini", "cc-2");
    quiet_agent(&mut chat, "me-mini", "cc-3");
    let rows = chat.sidebar_list(&all());
    assert!(!rows.iter().any(|r| r.name.starts_with("me-mini/")));

    // Opened, it's listed where its last message puts it, and stays put while it's open:
    // it doesn't leave the list under the person's cursor.
    let at = chat.position("me-mini/cc-2").unwrap();
    let _ = chat.select(at);
    let rows = chat.sidebar_list(&all());
    let opened = row(&rows, "me-mini/cc-2");
    assert_eq!(
        (opened.shelf, opened.direct_kind),
        (Shelf::Machines, Some(DirectKind::Agent))
    );
    assert!(!listed(&chat, "me-mini/cc-3"));
}

#[test]
fn a_folded_part_keeps_whats_new_and_whats_shown() {
    let mut chat = busy();
    feed(
        &mut chat,
        &["@time=2026-10-30T11:10:00.000Z :bob!u@h PRIVMSG #api/login :me: look"],
    );
    let _ = chat.mark_read("bob");
    let _ = chat.mark_read("carol");
    let at = chat.position("#web").unwrap();
    let _ = chat.select(at);
    chat.set_collapsed(Shelf::Channels, true);
    chat.set_collapsed(Shelf::Direct, true);
    let rows = chat.sidebar_list(&all());
    let channels: Vec<_> = rows
        .iter()
        .filter(|r| r.shelf == Shelf::Channels)
        .map(|r| (r.kind, r.name.as_str()))
        .collect();
    // #api's room is new, under #api; Anna's room is unanswered, under #lobby; #web is
    // shown; Browse channels folds away.
    assert_eq!(
        channels,
        [
            (ListKind::Heading, ""),
            (ListKind::Conversation, "#api"),
            (ListKind::Room, "#api/login"),
            (ListKind::Conversation, "#lobby"),
            (ListKind::GuestRoom, "#lobby/guest-7k2q"),
            (ListKind::Conversation, "#web"),
        ]
    );
    assert_eq!(rows[at_heading(&rows, Shelf::Channels)].open, Some(false));
    // Nothing new in direct messages: only the heading.
    let direct: Vec<_> = rows.iter().filter(|r| r.shelf == Shelf::Direct).collect();
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].kind, ListKind::Heading);
}

fn at_heading(rows: &[ListRow], shelf: Shelf) -> usize {
    rows.iter()
        .position(|r| r.kind == ListKind::Heading && r.shelf == shelf)
        .unwrap()
}

#[test]
fn a_direct_limit_lists_that_many_then_all_messages() {
    let mut chat = busy();
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:06:00.000Z :dave!u@h PRIVMSG me :yo",
            "@time=2026-10-30T11:07:00.000Z :erin!u@h PRIVMSG me :hey",
        ],
    );
    quiet_agent(&mut chat, "me-mini", "cc-2");
    let limited = ListOptions {
        direct_limit: Some(2),
        ..ListOptions::default()
    };
    let rows = chat.sidebar_list(&limited);
    let direct: Vec<_> = rows
        .iter()
        .filter(|r| r.shelf == Shelf::Direct)
        .map(|r| (r.kind, r.title.as_str()))
        .collect();
    assert_eq!(
        direct,
        [
            (ListKind::Heading, "Direct messages"),
            (ListKind::Conversation, "erin"),
            (ListKind::Conversation, "dave"),
            (ListKind::AllDirect, ALL_MESSAGES),
        ]
    );
    // It counts what it hides: carol's and bob's unread.
    let all_direct = rows.iter().find(|r| r.kind == ListKind::AllDirect).unwrap();
    assert!(all_direct.unread && all_direct.name.is_empty() && all_direct.open.is_none());
    assert_eq!(all_direct.badge, 2);
    // The quiet agent session isn't counted under it: it isn't listed at all.

    // When they all fit, no All messages.
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":bob!u@h PRIVMSG me :hi"]);
    let rows = chat.sidebar_list(&ListOptions {
        direct_limit: Some(3),
        ..ListOptions::default()
    });
    assert!(!rows.iter().any(|r| r.kind == ListKind::AllDirect));
}

#[test]
fn an_agent_at_work_has_its_direct_row_with_its_task() {
    let mut chat = busy();
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            ":bob-laptop!u@machine/bob JOIN #api",
            &format!(
                "@time=2026-10-30T11:00:00.000Z {}",
                zirc("me-mbp", &taking("cc-1", "the login page"))
            ),
            &format!(
                "@time=2026-10-30T11:30:00.000Z {}",
                zirc("bob-laptop", &taking("cc-9", "billing"))
            ),
            // cc-1 has said something to us too; cc-9 hasn't.
            &dm("me-mbp", "cc-1", "on it", 1),
        ],
    );
    let rows = chat.sidebar_list(&all());
    // Ours is under its machine, which is online, by what it goes by alone.
    let ours = row(&rows, "me-mbp/cc-1");
    assert_eq!(ours.shelf, Shelf::Machines);
    assert_eq!(ours.direct_kind, Some(DirectKind::Agent));
    assert_eq!(ours.note.as_deref(), Some("the login page"));
    assert!(ours.mine);
    assert_eq!(ours.title, "cc-1");
    assert_eq!(ours.faces.len(), 1);
    // A teammate's agent at work is listed too, with nothing said with it yet.
    let theirs = row(&rows, "bob-laptop/cc-9");
    assert_eq!(theirs.note.as_deref(), Some("billing"));
    assert!(!theirs.mine);
    // Under Machines, by bob's laptop; the direct messages are people's alone.
    assert_eq!(theirs.shelf, Shelf::Machines);
    let laptop = row(&rows, "bob-laptop");
    assert!(laptop.title.ends_with(" · bob-laptop"), "{laptop:?}");
    let direct: Vec<_> = rows
        .iter()
        .filter(|r| r.shelf == Shelf::Direct && r.kind == ListKind::Conversation)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(direct, ["carol", "bob"]);
}

#[test]
fn our_machines_thread_is_under_machines_online_or_not() {
    let mut chat = busy();
    // Without anything said, it's listed under Machines once, as it's connected, and not
    // in the direct messages.
    feed(&mut chat, &[":me-mbp!u@machine/me JOIN #api"]);
    let rows = chat.sidebar_list(&all());
    assert_eq!(row(&rows, "me-mbp").shelf, Shelf::Machines);
    assert_eq!(rows.iter().filter(|r| r.name == "me-mbp").count(), 1);
    feed(
        &mut chat,
        &[&dm(
            "me-mbp",
            zirc_proto::NO_SESSION,
            "all agents: pause",
            2,
        )],
    );
    let rows = chat.sidebar_list(&all());
    let thread = row(&rows, "me-mbp");
    assert_eq!(thread.shelf, Shelf::Machines);
    assert_eq!(rows.iter().filter(|r| r.name == "me-mbp").count(), 1);
    // Offline, it's still under Machines, its dot gone.
    feed(&mut chat, &[":me-mbp!u@machine/me QUIT :bye"]);
    let rows = chat.sidebar_list(&all());
    let thread = row(&rows, "me-mbp");
    assert_eq!(thread.shelf, Shelf::Machines);
    assert_eq!(thread.presence, Some(Presence::Gone));
    assert_eq!(thread.kind, ListKind::Conversation);
    assert_eq!(thread.direct_kind, Some(DirectKind::Machine));
    assert!(thread.mine && thread.direct);
    assert_eq!(thread.faces, [chat.face(&Chat::agents_avatar())]);
}

#[test]
fn a_row_serialises_for_the_web_in_camel_case() {
    let chat = busy();
    let rows = chat.sidebar_list(&all());
    let json = serde_json::to_value(row(&rows, "bob")).unwrap();
    assert_eq!(json["kind"], "conversation");
    assert_eq!(json["shelf"], "direct");
    assert_eq!(json["directKind"], "person");
    assert_eq!(json["badgeLabel"], "1");
    let options: ListOptions = serde_json::from_str(r#"{"directLimit": 3}"#).unwrap();
    assert_eq!(options.direct_limit, Some(3));
}

/// The sidebar as the switch shows `tab`.
fn tab(tab: SidebarSection) -> ListOptions {
    ListOptions {
        tab: Some(tab),
        ..ListOptions::default()
    }
}

#[test]
fn each_tab_lists_its_own_parts_and_whats_starred_in_it() {
    let mut chat = busy();
    chat.star("#web", true);
    chat.star("carol", true);
    let channels = chat.sidebar_list(&tab(SidebarSection::Channels));
    assert_eq!(headings(&channels), [Shelf::Starred, Shelf::Channels]);
    assert!(
        channels
            .iter()
            .any(|r| r.name == "#web" && r.shelf == Shelf::Starred)
    );
    assert!(!channels.iter().any(|r| r.name == "carol"));

    let messages = chat.sidebar_list(&tab(SidebarSection::Direct));
    assert_eq!(headings(&messages), [Shelf::Starred, Shelf::Direct]);
    assert!(
        messages
            .iter()
            .any(|r| r.name == "carol" && r.shelf == Shelf::Starred)
    );
    assert!(
        !messages
            .iter()
            .any(|r| r.name == "#web" || r.name == "#api")
    );

    // Without a tab, both, one after the other, as before.
    let both = chat.sidebar_list(&all());
    assert_eq!(
        both.len(),
        channels.len() + messages.len() - 1,
        "one Starred heading"
    );
    assert_eq!(
        SidebarSection::ALL.map(SidebarSection::label),
        ["Channels", "Messages", "Machines"]
    );
}

#[test]
fn each_tab_says_whats_new_in_it_and_opening_one_shows_its_tab() {
    let mut chat = busy();
    let mark = |chat: &Chat, section| {
        chat.tabs()
            .into_iter()
            .find(|tab| tab.section == section)
            .unwrap()
    };
    let tabs = chat.tabs();
    assert_eq!(
        tabs.iter()
            .map(|tab| (tab.label.as_str(), tab.chosen))
            .collect::<Vec<_>>(),
        [("Channels", true), ("Messages", false), ("Machines", false)]
    );
    // carol's message is unread and counted: a direct message counts everything.
    let messages = mark(&chat, SidebarSection::Direct);
    assert!(messages.unread && messages.badge > 0, "{messages:?}");
    assert_eq!(messages.badge_label, Some(messages.badge.to_string()));
    feed(&mut chat, &[":bob!u@h PRIVMSG #web :lunch?"]);
    let channels = mark(&chat, SidebarSection::Channels);
    assert!(channels.unread, "{channels:?}");
    assert_eq!(
        (channels.badge, channels.badge_label),
        (0, None),
        "only mentions count"
    );

    // Opened from anywhere, its tab is the one shown, and it's read: counted no more.
    let at = chat.position("carol").unwrap();
    let _ = chat.select(at);
    assert_eq!(chat.tab(), SidebarSection::Direct);
    assert!(mark(&chat, SidebarSection::Direct).chosen);
    assert!(mark(&chat, SidebarSection::Direct).badge < messages.badge);
    chat.set_tab(SidebarSection::Channels);
    assert_eq!(chat.tab(), SidebarSection::Channels);
}
