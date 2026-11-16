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
            "Conversation Server Server",
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
    let heading = &rows[1];
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
fn agents_are_direct_messages_once_they_have_said_something_and_our_machines_never() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            ":me-ci-9!u@machine/me JOIN #api",
            // cc-2 only posts in a channel: that never lists it.
            &format!(
                "@time=2026-10-30T11:00:00.000Z {}",
                zirc("me-mbp", &taking("cc-2", "the login page"))
            ),
            &dm("me-mbp", "cc-1", "on it", 1),
            &dm("me-laptop", "cc-3", "elsewhere", 1),
        ],
    );
    quiet_agent(&mut chat, "me-mbp", "cc-old");
    let rows = chat.sidebar_list(&all());
    // No Machines part: the switch's halves are Channels and Messages.
    assert_eq!(headings(&rows), [Shelf::Channels, Shelf::Direct]);
    // The sessions that wrote to us, among the direct messages, on their own rows.
    for name in ["me-mbp/cc-1", "me-laptop/cc-3"] {
        let agent = row(&rows, name);
        assert_eq!(
            (agent.shelf, agent.depth, agent.direct_kind),
            (Shelf::Direct, 0, Some(DirectKind::Agent))
        );
    }
    // Online machines with nothing said in their threads, a session that only posted in a
    // channel, and one quiet for over an hour aren't listed.
    for name in ["me-mbp", "me-ci-9", "me-mbp/cc-2", "me-mbp/cc-old"] {
        assert!(!listed(&chat, name), "{name}");
    }
    // Our machine itself isn't, even once something's said to us as it.
    feed(
        &mut chat,
        &[":me-ci-9!u@machine/me PRIVMSG me :build green"],
    );
    assert!(!listed(&chat, "me-ci-9"));
    // Opened, the quiet one is listed till something else is.
    let at = chat.position("me-mbp/cc-old").unwrap();
    let _ = chat.select(at);
    assert_eq!(
        row(&chat.sidebar_list(&all()), "me-mbp/cc-old").shelf,
        Shelf::Direct
    );
}

/// Whether a direct message called `name` is anywhere in the sidebar.
fn listed(chat: &Chat, name: &str) -> bool {
    chat.sidebar_list(&all()).iter().any(|row| row.name == name)
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
            "Conversation Server Server",
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
    // What's said in a room is its own row's preview, not its channel's.
    assert_ne!(api.preview.as_deref(), Some("bob: me: look"));
    let login = row(&rows, "#api/login");
    assert_eq!(login.preview.as_deref(), Some("bob: me: look"));
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
    assert_eq!(
        kinds,
        [
            ListKind::Conversation,
            ListKind::Heading,
            ListKind::NewChannel
        ]
    );
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
            "Conversation Server Server",
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
        (Shelf::Direct, Some(DirectKind::Agent))
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
fn an_agent_at_work_that_wrote_to_us_has_its_direct_row_with_its_task() {
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
    // Ours, among the direct messages, with what it's taking on.
    let ours = row(&rows, "me-mbp/cc-1");
    assert_eq!(ours.shelf, Shelf::Direct);
    assert_eq!(ours.direct_kind, Some(DirectKind::Agent));
    assert_eq!(ours.note.as_deref(), Some("the login page"));
    assert!(ours.mine);
    assert_eq!(ours.faces.len(), 1);
    // A teammate's agent at work that never wrote to us isn't listed, nor is its machine.
    assert!(!listed(&chat, "bob-laptop/cc-9"));
    assert!(!listed(&chat, "bob-laptop"));
    let direct: Vec<_> = rows
        .iter()
        .filter(|r| r.shelf == Shelf::Direct && r.kind == ListKind::Conversation)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(direct, ["me-mbp/cc-1", "carol", "bob"]);
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
        channels.len() + messages.len() - 2,
        "one Server row and one Starred heading"
    );
    assert_eq!(
        SidebarSection::ALL.map(SidebarSection::label),
        ["Channels", "Messages"]
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
        [("Channels", true), ("Messages", false)]
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

#[test]
fn an_empty_conversation_says_no_messages_yet_once_its_history_is_in() {
    let mut chat = super::chat();
    connected(&mut chat);
    let says = |chat: &Chat, name: &str| {
        let rows = chat.sidebar_list(&all());
        let row = row(&rows, name);
        (row.preview.clone(), row.placeholder, row.when.clone())
    };
    // Its history on its way: nothing yet, so a row doesn't say it's empty and then change.
    assert_eq!(says(&chat, "#api"), (None, false, None));
    feed(&mut chat, &[":s BATCH +e chathistory #api", ":s BATCH -e"]);
    assert_eq!(
        says(&chat, "#api"),
        (Some(crate::chat::NO_MESSAGES.to_owned()), true, None)
    );
    // A direct message alike.
    chat.open("bob");
    assert_eq!(says(&chat, "bob"), (None, false, None));
    feed(&mut chat, &[":s BATCH +b chathistory bob", ":s BATCH -b"]);
    assert_eq!(
        says(&chat, "bob"),
        (Some(crate::chat::NO_MESSAGES.to_owned()), true, None)
    );
    // A room under its channel, a thread, alike, and in its channel's Threads list.
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api/fix-login",
            ":s BATCH +r chathistory #api/fix-login",
            ":s BATCH -r",
        ],
    );
    assert_eq!(
        says(&chat, "#api/fix-login"),
        (Some(crate::chat::NO_MESSAGES.to_owned()), true, None)
    );
    let thread = &chat.threads("#api")[0];
    assert_eq!(
        (thread.preview.as_deref(), thread.placeholder),
        (Some(crate::chat::NO_MESSAGES), true)
    );
    // Once something's said, it's what's previewed.
    feed(&mut chat, &[":bob!u@h PRIVMSG #api :hello"]);
    let (preview, placeholder, when) = says(&chat, "#api");
    assert_eq!(
        (preview.as_deref(), placeholder),
        (Some("bob: hello"), false)
    );
    assert!(when.is_some());
}

#[test]
fn a_channels_threads_list_those_the_server_lists_that_we_arent_in() {
    let mut chat = super::chat();
    connected(&mut chat);
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":srv 322 me #api 3 :the API",
            ":srv 322 me #api/fix-login 1 :",
            ":srv 322 me #web/other 1 :",
            ":srv 323 me :End of /LIST",
        ],
    );
    let names: Vec<String> = chat.threads("#api").into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["#api/fix-login"]);
}

#[test]
fn a_channel_previews_its_own_words_and_each_thread_its_own() {
    let mut chat = super::chat();
    connected(&mut chat);
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api/fix-login",
            "@time=2026-10-30T11:00:00.000Z :bob!u@h PRIVMSG #api :shipping at 3",
            "@time=2026-10-30T11:30:00.000Z :carol!u@h PRIVMSG #api/fix-login :retried with a 30s cap",
        ],
    );
    let rows = chat.sidebar_list(&all());
    // The channel's own line, though its thread's is newer: no line shows on two rows.
    assert_eq!(
        row(&rows, "#api").preview.as_deref(),
        Some("bob: shipping at 3")
    );
    let thread = row(&rows, "#api/fix-login");
    assert_eq!(
        (thread.preview.as_deref(), thread.when.as_deref()),
        (Some("carol: retried with a 30s cap"), Some("30m"))
    );
    assert_eq!(
        chat.threads("#api")[0].preview.as_deref(),
        Some("carol: retried with a 30s cap")
    );
}
