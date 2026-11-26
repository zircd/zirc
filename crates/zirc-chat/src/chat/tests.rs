use chrono::TimeZone as _;

use super::*;

mod agent_activity;
mod busy;
mod cold_start;
mod drafts;
mod finder;
mod list_order;
mod live;
mod navigate;
mod onboarding;
mod perf;
mod scene;
mod sidebar;
mod waiting;
mod working;
use scene::Scene;

fn noon() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
}

fn chat_with(options: Options) -> Chat {
    let mut chat = Chat::new("me", "irc.zirc.test", vec!["#extra".into()], options);
    chat.clock = noon;
    chat
}

/// A chat that asked to join nothing besides what the server lists.
fn bare() -> Chat {
    let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
    chat.clock = noon;
    chat
}

fn chat() -> Chat {
    chat_with(Options::default())
}

fn link(echo: bool) -> Link {
    Link {
        nick: "me".into(),
        metadata: false,
        chathistory: true,
        echo,
        read_marker: false,
        trim: true,
        multiline: None,
    }
}

fn connected(chat: &mut Chat) {
    chat.connected(link(true));
    feed(chat, &[":me!u@h JOIN #api"]);
}

fn feed(chat: &mut Chat, lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| chat.receive(merged(line).parse().unwrap()))
        .map(|line| line.to_string())
        .collect()
}

/// `line` with two leading groups of tags made one, as when a test puts `@time=…` before a
/// working set's TAGMSG ([`zirc`]), which has tags of its own.
fn merged(line: &str) -> String {
    match line.split_once(' ') {
        Some((first, rest)) if first.starts_with('@') && rest.starts_with('@') => {
            format!("{first};{}", &rest[1..])
        }
        _ => line.to_owned(),
    }
}

fn texts(lines: &[Line]) -> Vec<String> {
    lines.iter().map(ToString::to_string).collect()
}

/// What's said to the server, leaving out what the tenant's pusher is told of reads.
fn to_server(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(String::as_str)
        .filter(|line| !line.ends_with(&format!("TAGMSG {}", zirc_proto::push::PUSHER)))
        .collect()
}

fn names(chat: &Chat) -> Vec<&str> {
    chat.conversations()
        .iter()
        .map(|c| c.name.as_str())
        .collect()
}

fn api(chat: &Chat) -> &Conversation {
    chat.conversation("#api").unwrap()
}

fn said(conversation: &Conversation) -> Vec<String> {
    conversation.entries.iter().map(entry_text).collect()
}

/// `message` from `nick` in #api as a PRIVMSG, as zircds before 0.1.24 sent working sets
/// and history still holds them.
fn old_zirc(nick: &str, message: &Message) -> String {
    format!(
        ":{} PRIVMSG #api :{}",
        source(nick),
        zirc_proto::encode(message).unwrap()
    )
}

/// `message` from `nick` in #api, as it reaches a client ([`line_from`]).
fn zirc(nick: &str, message: &Message) -> String {
    line_from(nick, "#api", message).to_string()
}

/// `message` from `nick` in `channel`, as it reaches a client ([`crate::presence::line_from`]),
/// from `nick`'s host ([`host`]).
fn line_from(nick: &str, channel: &str, message: &Message) -> Line {
    let mut line = crate::presence::line_from(nick, channel, message).unwrap();
    line.source = Some(source(nick));
    line
}

/// `nick`'s source, as lines from it have it: its nick, a user and its [`host`].
fn source(nick: &str) -> String {
    format!("{nick}!u@{}", host(nick))
}

/// `nick`'s host, as the server gives it. These tests name their machines `<owner>-<name>`,
/// and a machine's host says whose it is (`machine/<owner>`, which only the server sets): its
/// name alone never does. A person's and zirc's own accounts' hosts say nothing.
fn host(nick: &str) -> String {
    match nick.split_once('-') {
        Some((owner, _)) if owner != "zirc" => zirc_proto::names::machine_host(owner),
        _ => "h".to_owned(),
    }
}

fn working(session: &str, paths: &[&str]) -> Message {
    Message::new(
        session,
        Event::Working {
            paths: paths.iter().map(|&p| p.to_owned()).collect(),
            continued: false,
            worktree: None,
            branch: None,
            part: Some([1, 1]),
            base: None,
            committed: Vec::new(),
        },
    )
}

/// A batch `reference` of history in #api, with messages `ids` a minute apart from 11:00.
fn page(reference: &str, ids: std::ops::RangeInclusive<u32>) -> Vec<String> {
    let mut lines = vec![format!(":s BATCH +{reference} chathistory #api")];
    lines.extend(ids.map(|id| {
        format!(
            "@batch={reference};time=2026-10-30T11:{:02}:00.000Z;msgid=m{id} :bob!u@h PRIVMSG #api :{id}",
            id % 60
        )
    }));
    lines.push(format!(":s BATCH -{reference}"));
    lines
}

fn feed_all(chat: &mut Chat, lines: &[String]) -> Vec<String> {
    feed(chat, &lines.iter().map(String::as_str).collect::<Vec<_>>())
}

#[test]
fn catches_up_from_the_newest_message_on_connecting_again() {
    let mut chat = chat_with(Options {
        history: 3,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 1..=2));
    assert_eq!(said(api(&chat)), ["<bob> 1", "<bob> 2"]);

    chat.disconnected("gone");
    chat.connected(link(true));
    // Rejoined: only what came after m2, asked from a second before it was said.
    let sent = feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert_eq!(
        sent.last().unwrap(),
        "CHATHISTORY LATEST #api timestamp=2026-10-30T11:01:59.000Z 3"
    );
    // Seven since: a full page of the newest, then the pages before it, back to m2.
    let sent = feed_all(&mut chat, &page("b", 7..=9));
    assert_eq!(sent, ["CHATHISTORY BEFORE #api msgid=m7 3"]);
    let sent = feed_all(&mut chat, &page("c", 4..=6));
    assert_eq!(sent, ["CHATHISTORY BEFORE #api msgid=m4 3"]);
    // This one reaches m2, held already: the gap's closed, and nothing's twice.
    let sent = feed_all(&mut chat, &page("d", 1..=3));
    assert!(
        !sent.iter().any(|line| line.starts_with("CHATHISTORY")),
        "{sent:?}"
    );
    assert_eq!(
        said(api(&chat)),
        (1..=9).map(|n| format!("<bob> {n}")).collect::<Vec<_>>()
    );
}

/// A server that no longer has the newest message held (trimmed, past what it keeps, its
/// history lost in a restart) still gives what was said since: the catch-up asks from a
/// time, which it can always answer, where from an id it doesn't know it answers nothing.
#[test]
fn catches_up_though_the_server_lost_the_newest_message_held() {
    let mut chat = chat_with(Options {
        history: 3,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 1..=2));
    chat.disconnected("gone");
    chat.connected(link(true));
    let sent = feed(&mut chat, &[":me!u@h JOIN #api"]);
    let asked = sent.last().unwrap();
    assert!(
        asked.starts_with("CHATHISTORY LATEST #api timestamp="),
        "{asked}"
    );
    assert!(!asked.contains("msgid="), "{asked}");
    // All it has now: one message, said since, and neither of the two held.
    let sent = feed_all(&mut chat, &page("b", 5..=5));
    assert!(
        !sent.iter().any(|line| line.starts_with("CHATHISTORY")),
        "{sent:?}"
    );
    assert_eq!(said(api(&chat)), ["<bob> 1", "<bob> 2", "<bob> 5"]);
}

#[test]
fn opened_at_a_message_it_pages_back_until_the_message_is_there() {
    let mut chat = chat_with(Options {
        history: 3,
        screenful: 1,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 7..=9));
    // Loaded already: found at once.
    chat.open_at("#api", "m8");
    assert_eq!(chat.found().as_deref(), Some("m8"));
    assert_eq!(chat.found(), None);
    // Further back: a page at a time, until it's there.
    let sent = texts(&chat.open_at("#api", "m2"));
    assert_eq!(sent.last().unwrap(), "CHATHISTORY BEFORE #api msgid=m7 3");
    assert_eq!(chat.found(), None);
    let sent = feed_all(&mut chat, &page("b", 4..=6));
    assert_eq!(sent, ["CHATHISTORY BEFORE #api msgid=m4 3"]);
    let sent = feed_all(&mut chat, &page("c", 1..=3));
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(chat.found().as_deref(), Some("m2"));
}

#[test]
fn opened_at_a_message_before_joining_it_pages_back_once_joined() {
    // As a conversation's link opens on connecting: the channel isn't joined yet.
    let mut chat = chat_with(Options {
        history: 3,
        screenful: 1,
        ..Options::default()
    });
    chat.connected(link(true));
    let sent = texts(&chat.open_at("#api", "m2"));
    assert!(sent.iter().any(|line| line == "JOIN #api"), "{sent:?}");
    let sent = feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert!(
        sent.iter()
            .any(|line| line.starts_with("CHATHISTORY LATEST #api")),
        "{sent:?}"
    );
    let sent = feed_all(&mut chat, &page("a", 7..=9));
    assert_eq!(sent, ["CHATHISTORY BEFORE #api msgid=m7 3"]);
    let sent = feed_all(&mut chat, &page("b", 4..=6));
    assert_eq!(sent, ["CHATHISTORY BEFORE #api msgid=m4 3"]);
    feed_all(&mut chat, &page("c", 1..=3));
    assert_eq!(chat.found().as_deref(), Some("m2"));
}

/// A chat that opens at a message with one window around it, joined to #api with its
/// newest page, m7 to m9, in.
fn around() -> Chat {
    let mut chat = chat_with(Options {
        history: 3,
        screenful: 1,
        around: true,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 7..=9));
    chat
}

#[test]
fn opened_at_an_older_message_it_asks_for_those_around_it_once_and_leaves_a_hole() {
    let mut chat = around();
    let sent = texts(&chat.open_at("#api", "m2"));
    assert_eq!(sent.last().unwrap(), "CHATHISTORY AROUND #api msgid=m2 3");
    assert_eq!(chat.found(), None);
    // One page, not one after another back to it.
    let sent = feed_all(&mut chat, &page("b", 1..=3));
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(chat.found().as_deref(), Some("m2"));
    assert_eq!(
        said(api(&chat)),
        [
            "<bob> 1", "<bob> 2", "<bob> 3", "<bob> 7", "<bob> 8", "<bob> 9"
        ]
    );
    assert_eq!(chat.hole("#api"), Some("m3"));
}

#[test]
fn the_hole_fills_a_page_at_a_time_until_it_meets_what_is_newest() {
    let mut chat = around();
    chat.open_at("#api", "m2");
    feed_all(&mut chat, &page("b", 1..=3));
    let asked = chat.newer("#api").map(|line| line.to_string());
    assert_eq!(asked.as_deref(), Some("CHATHISTORY AFTER #api msgid=m3 3"));
    // Once at a time.
    assert!(chat.newer("#api").is_none());
    feed_all(&mut chat, &page("c", 4..=6));
    assert_eq!(chat.hole("#api"), Some("m6"));
    let asked = chat.newer("#api").map(|line| line.to_string());
    assert_eq!(asked.as_deref(), Some("CHATHISTORY AFTER #api msgid=m6 3"));
    // It has m7, which is held: the hole is closed.
    feed_all(&mut chat, &page("d", 7..=9));
    assert_eq!(chat.hole("#api"), None);
    assert!(chat.newer("#api").is_none());
    assert_eq!(said(api(&chat)).len(), 9);
}

#[test]
fn a_short_page_after_the_hole_closes_it() {
    let mut chat = around();
    chat.open_at("#api", "m2");
    feed_all(&mut chat, &page("b", 1..=3));
    chat.newer("#api");
    feed_all(&mut chat, &page("c", 4..=5));
    assert_eq!(chat.hole("#api"), None);
}

#[test]
fn a_window_that_meets_what_is_held_leaves_no_hole() {
    let mut chat = around();
    let sent = texts(&chat.open_at("#api", "m5"));
    assert_eq!(sent.last().unwrap(), "CHATHISTORY AROUND #api msgid=m5 3");
    feed_all(&mut chat, &page("b", 5..=7));
    assert_eq!(chat.found().as_deref(), Some("m5"));
    assert_eq!(chat.hole("#api"), None);
}

#[test]
fn a_message_held_already_is_found_without_asking() {
    let mut chat = around();
    let sent = texts(&chat.open_at("#api", "m8"));
    assert!(
        !sent.iter().any(|line| line.starts_with("CHATHISTORY")),
        "{sent:?}"
    );
    assert_eq!(chat.found().as_deref(), Some("m8"));
}

#[test]
fn a_message_the_server_does_not_have_shows_the_end_quietly() {
    let mut chat = around();
    chat.open_at("#api", "gone");
    feed(
        &mut chat,
        &[":s FAIL CHATHISTORY INVALID_TARGET AROUND #api :Messages could not be retrieved"],
    );
    assert_eq!(chat.found(), None);
    assert_eq!(chat.hole("#api"), None);
    assert!(
        !said(api(&chat))
            .iter()
            .any(|text| text.contains("CHATHISTORY"))
    );
    // And with an empty page instead.
    chat.open_at("#api", "gone");
    feed(&mut chat, &[":s BATCH +e chathistory #api", ":s BATCH -e"]);
    assert_eq!(chat.found(), None);
    assert_eq!(chat.hole("#api"), None);
    // Nothing's left looked for, or in the way of asking for older history.
    assert!(chat.older("#api").is_some());
}

#[test]
fn opened_at_a_message_it_never_finds_it_stops_at_the_beginning() {
    let mut chat = chat_with(Options {
        history: 3,
        screenful: 1,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 7..=9));
    chat.open_at("#api", "gone");
    // A short page: the history begins there, and there's nothing more to ask.
    let sent = feed_all(&mut chat, &page("b", 6..=6));
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(chat.found(), None);
}

#[test]
fn what_is_said_offline_waits_and_goes_once_its_channel_is_joined_again() {
    let mut chat = chat_with(Options::default());
    connected(&mut chat);
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    chat.disconnected("gone");
    // Offline: nothing goes yet, and nothing is refused.
    assert!(chat.say("#api", "said while offline", false).is_empty());
    assert!(chat.say("bob", "and to bob", false).is_empty());
    // Connected, the direct message goes; the channel's waits till it's joined again.
    let up = texts(&chat.connected(link(true)));
    assert!(
        up.iter()
            .any(|line| line.ends_with("PRIVMSG bob :and to bob")),
        "{up:?}"
    );
    assert!(
        !up.iter().any(|line| line.contains("said while offline")),
        "{up:?}"
    );
    let joined = feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert!(
        joined
            .iter()
            .any(|line| line.ends_with("PRIVMSG #api :said while offline")),
        "{joined:?}"
    );
}

#[test]
fn a_short_page_closes_the_gap() {
    let mut chat = chat_with(Options {
        history: 3,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 1..=1));
    chat.disconnected("gone");
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    let sent = feed_all(&mut chat, &page("b", 2..=3));
    assert!(
        !sent.iter().any(|line| line.starts_with("CHATHISTORY")),
        "{sent:?}"
    );
    assert_eq!(said(api(&chat)), ["<bob> 1", "<bob> 2", "<bob> 3"]);
}

#[test]
fn keeps_each_repositorys_work_in_agents_apart_and_names_it_once_there_are_several() {
    let mut chat = chat();
    let _ = chat.connected(link(true));
    let in_repo = |repo: &str, event: Event| Message {
        repo: Some(repo.into()),
        ..Message::new("cc-1", event)
    };
    let working = |repo: &str, path: &str| {
        in_repo(
            repo,
            Event::Working {
                paths: vec![path.into()],
                continued: false,
                worktree: Some(repo.into()),
                branch: Some("main".into()),
                part: Some(zirc_proto::ALONE),
                base: None,
                committed: Vec::new(),
            },
        )
    };
    let say = |from: &str, message: &Message| line_from(from, "#agents", message).to_string();
    let taking = in_repo(
        "api",
        Event::Text {
            body: "taking: the login bug".into(),
        },
    );
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #agents",
            ":alice-mbp!u@machine/alice JOIN #agents",
            &say("alice-mbp", &taking),
        ],
    );
    // What an agent takes on in #agents is on its machine's card, and its claim.
    let card = chat.profile("alice-mbp").card(noon(), |at| at.naive_utc());
    assert!(
        card.work.iter().any(|work| work
            .lines
            .iter()
            .any(|line| line.ends_with("the login bug"))),
        "{:?}",
        card.work
    );
    assert_eq!(
        chat.claim("alice-mbp", "cc-1")
            .map(|(task, _)| task)
            .as_deref(),
        Some("the login bug")
    );
    // One repository: its lines say nothing of it.
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let detail = |chat: &Chat, text: &str| {
        let lines = chat.lines("#agents", utc, false);
        let line = lines.iter().find(|line| line.text.contains(text)).unwrap();
        line.speaker.clone().unwrap().detail.unwrap_or_default()
    };
    assert!(!detail(&chat, "login bug").contains("api"));

    // Two repositories' working sets from one machine, each said in turn: both stand, and
    // the same path in each is no file both change.
    feed(
        &mut chat,
        &[
            &say("alice-mbp", &working("api", "README.md")),
            &say("bob-mbp", &working("web", "README.md")),
        ],
    );
    assert_eq!(chat.presence.members("#agents").len(), 2);
    assert!(chat.both_changing("#agents").is_empty());
    // Several repositories: each line says which it's about.
    let said = detail(&chat, "login bug");
    assert!(said.ends_with(" · api") || said == "api", "{said}");
}

#[test]
fn shows_bold_what_is_unread_unless_muted_or_left() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[":me!u@h JOIN #web", ":bob!u@h PRIVMSG #web :lunch?"],
    );
    assert!(chat.is_unread("#web"));
    // Muted: its mentions still count, but it isn't bold.
    chat.mute("#web", Some(Muted::Off));
    assert!(!chat.is_unread("#web"));
    chat.mute("#web", None);
    assert!(chat.is_unread("#web"));
    // Left: what came before stays, but a channel we're not in isn't bold.
    feed(&mut chat, &[":me!u@h PART #web"]);
    assert!(!chat.is_unread("#web"));
}

#[test]
fn lists_joins_and_fetches_history() {
    let mut chat = chat();
    let first = texts(&chat.connected(link(true)));
    assert_eq!(first[0], "LIST");
    assert!(first[1].starts_with("CHATHISTORY TARGETS timestamp=2026-09-30T12:00:00.000Z"));
    let sent = feed(
        &mut chat,
        &[
            ":s 322 me #api 3 :the API",
            ":s 322 me #web 1 :",
            ":s 323 me :End of LIST",
        ],
    );
    assert_eq!(sent, ["JOIN #extra"]);
    assert_eq!(
        chat.listed(),
        [("#api".to_owned(), 3), ("#web".to_owned(), 1)]
    );
    let sent = feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s 353 me = #api :@alice me bob",
            ":s 366 me #api :End",
        ],
    );
    // Its machines are asked what they're working on, which no history keeps.
    assert_eq!(
        sent,
        [
            "WHO #api %tnfhar,42",
            "CHATHISTORY LATEST #api * 200",
            "@+zirc/presence=ask TAGMSG #api"
        ]
    );
    assert_eq!(names(&chat), ["irc.zirc.test", "#api"]);
    assert_eq!(
        chat.selected().name,
        "#api",
        "the first channel joined shows"
    );
    let members: Vec<_> = api(&chat)
        .sorted_members()
        .iter()
        .map(|m| m.nick.clone())
        .collect();
    assert_eq!(members, ["alice", "bob", "me"]);
}

#[test]
fn shows_the_channel_first_in_order_whichever_the_server_joins_first() {
    let mut chat = chat();
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #web", ":me!u@h JOIN #api"]);
    assert_eq!(chat.selected().name, "#api");
    feed(&mut chat, &[":me!u@h JOIN #zz"]);
    assert_eq!(chat.selected().name, "#api");
    // Once the person shows one, joins leave it be.
    let web = chat.position("#web").unwrap();
    let _ = chat.select(web);
    feed(&mut chat, &[":me!u@h JOIN #aa"]);
    assert_eq!(chat.selected().name, "#web");
}

#[test]
fn a_channel_joined_again_leaves_what_is_shown() {
    let mut chat = Scene::in_api();
    assert_eq!(chat.selected().name, "#api");
    let _ = chat.select(0);
    // As Ergo can say when another of the person's apps joins it too.
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert_eq!(chat.selected().name, "irc.zirc.test");
}

#[test]
fn can_leave_the_servers_conversation_shown_when_channels_join() {
    let mut chat = chat_with(Options {
        show_first_channel: false,
        ..Options::default()
    });
    connected(&mut chat);
    feed(&mut chat, &[":alice!u@h PRIVMSG #api :hi"]);
    assert_eq!(chat.selected().name, "irc.zirc.test");
    assert_eq!(api(&chat).unread, 1);
    // A channel the person asks for still shows.
    chat.join(&["#web".to_owned()]);
    feed(&mut chat, &[":me!u@h JOIN #web"]);
    assert_eq!(chat.selected().name, "#web");
}

#[test]
fn history_fills_in_without_counting_as_unread() {
    let mut chat = Scene::in_api();
    let _ = chat.select(0);
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:59:00.000Z;msgid=2 :alice!u@h PRIVMSG #api :live",
            ":s BATCH +h1 chathistory #api",
            "@batch=h1;time=2026-10-30T10:00:00.000Z;msgid=1 :bob!u@h PRIVMSG #api :old",
            "@batch=h1;time=2026-10-30T11:59:00.000Z;msgid=2 :alice!u@h PRIVMSG #api :live",
            "@batch=h1;time=2026-10-30T11:59:30.000Z;msgid=3 :HistServ!HistServ@s PRIVMSG #api :bob joined the channel",
            ":s BATCH -h1",
        ],
    );
    assert_eq!(said(api(&chat)), ["<bob> old", "<alice> live"]);
    assert!(api(&chat).entries[0].history);
    assert_eq!((api(&chat).unread, api(&chat).mentions), (1, 0));
    assert_eq!(
        chat.take_fresh().len(),
        1,
        "only what arrived live is fresh"
    );
}

#[test]
fn keeps_irc_formatting_for_rich_text_and_strips_it_everywhere_else() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":alice!u@h PRIVMSG #api :\x02ship\x02 it \x0304now\x03\x1b[2J",
            ":alice!u@h PRIVMSG #api :plain",
            ":alice!u@h PRIVMSG #api :\x01ACTION \x1dwaves\x1d\x01",
        ],
    );
    let entries = &api(&chat).entries;
    // The text everything else reads: no codes, and no terminal escape.
    assert_eq!(entries[0].kind, EntryKind::Message("ship it now[2J".into()));
    assert_eq!(
        entries[0].rich(),
        [Block::Paragraph(vec![
            Span {
                text: "ship".into(),
                bold: true,
                ..Span::default()
            },
            Span {
                text: " it now[2J".into(),
                ..Span::default()
            },
        ])]
    );
    assert_eq!(entries[1].formatted, None);
    assert_eq!(
        entries[2].rich(),
        [Block::Paragraph(vec![Span {
            text: "waves".into(),
            italic: true,
            ..Span::default()
        }])]
    );
    // What we say keeps its formatting too, and only codes.
    assert_eq!(
        formatted("\x02mine\x02\x07").as_deref(),
        Some("\x02mine\x02")
    );
    assert_eq!(formatted("plain"), None);
}

#[test]
fn history_replays_a_multiline_message_as_one() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":s BATCH +h1 chathistory #api",
            "@batch=h1;time=2026-10-30T10:00:00.000Z;msgid=m1 :s BATCH +ml draft/multiline #api",
            "@batch=ml :bob!u@h PRIVMSG #api :first line",
            "@batch=ml :bob!u@h PRIVMSG #api :```rust",
            "@batch=ml :bob!u@h PRIVMSG #api :fn main() {}",
            "@batch=ml :bob!u@h PRIVMSG #api :```",
            ":s BATCH -ml",
            ":s BATCH -h1",
        ],
    );
    let entries = &api(&chat).entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id.as_deref(), Some("m1"));
    assert!(entries[0].history);
    assert_eq!(
        entries[0].kind,
        EntryKind::Message("first line\n```rust\nfn main() {}\n```".into())
    );
}

#[test]
fn counts_mentions_and_private_messages() {
    let mut chat = Scene::in_api();
    let _ = chat.select(0);
    feed(
        &mut chat,
        &[
            ":alice!u@h PRIVMSG #api :ME: look",
            ":alice!u@h PRIVMSG #api :\x02unrelated\x02",
            ":bob!u@h PRIVMSG me :psst",
        ],
    );
    assert_eq!(names(&chat), ["irc.zirc.test", "#api", "bob"]);
    assert_eq!((api(&chat).unread, api(&chat).mentions), (2, 1));
    assert_eq!(
        said(api(&chat))[1],
        "<alice> unrelated",
        "formatting is stripped"
    );
    let bob = chat.conversation("bob").unwrap();
    assert_eq!((bob.unread, bob.mentions), (1, 1));
    // Down the sidebar from the server's: #api, then bob, both mentioning us.
    assert_eq!(chat.next_unread(1).as_deref(), Some("#api"));
    let _ = chat.open("#api");
    assert_eq!(chat.selected().unread, 0);
    assert_eq!(chat.next_unread(1).as_deref(), Some("bob"));
    let _ = chat.open("bob");
    assert!(chat.next_unread(1).is_none());
}

#[test]
fn reads_zirc_events_and_keeps_working_sets_out() {
    let commit = Message::new(
        "cc-12",
        Event::Commit {
            paths: Vec::new(),
            branch: "main".into(),
            sha: "abc".into(),
            subject: "Fix".into(),
            repo: None,
        },
    );
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            &zirc("mbp", &commit),
            &zirc("mbp", &working("cc-12", &["a.rs"])),
            &zirc("mbp", &working("cc-12", &["a.rs", "b.rs"])),
            &zirc("air", &working("cc-9", &["b.rs"])),
        ],
    );
    assert_eq!(said(api(&chat))[0], "[commit] mbp: Fix  (main abc)");
    let both = chat.both_changing("#api");
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].path, "b.rs");
    let names: Vec<&str> = both[0].who.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["cc-12", "cc-9"]);
    assert!(!both[0].same_lines && !both[0].mine);
}

/// Working sets from two of `nick`'s worktrees, `cc-1` in `api-login` and `cc-2` in
/// `api-hooks`, each with its paths.
fn two_worktrees(nick: &str, one: &[&str], two: &[&str]) -> [String; 2] {
    let part = |session: &str, worktree: &str, paths: &[&str], part: [u32; 2]| {
        Message::new(
            session,
            Event::Working {
                paths: paths.iter().map(|&p| p.to_owned()).collect(),
                continued: part[0] > 1,
                worktree: Some(worktree.into()),
                branch: Some(worktree.into()),
                part: Some(part),
                base: None,
                committed: Vec::new(),
            },
        )
    };
    [
        zirc(nick, &part("cc-1", "api-login", one, [1, 2])),
        zirc(nick, &part("cc-2", "api-hooks", two, [2, 2])),
    ]
}

fn both_rows(chat: &Chat) -> Vec<Parts> {
    api(chat)
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Zirc(message) => super::both::row_words(&message.event).map(|_| {
                view::event_parts(
                    &entry.from,
                    entry.owner.as_deref(),
                    message,
                    entry.time,
                    view::Names {
                        owner: &str::to_owned,
                        session: &str::to_owned,
                    },
                )
            }),
            _ => None,
        })
        .collect()
}

#[test]
fn two_sessions_on_one_machine_are_both_changing_a_file() {
    let mut chat = Scene::in_api();
    // cc-1 was changing a.rs, then cc-2 began to.
    feed_all(
        &mut chat,
        &two_worktrees("bob-mbp", &["a.rs", "b.rs"], &["c.rs"]),
    );
    assert!(chat.both_changing("#api").is_empty());
    feed_all(
        &mut chat,
        &two_worktrees("bob-mbp", &["a.rs", "b.rs"], &["a.rs"]),
    );
    let both = chat.both_changing("#api");
    assert_eq!(both.len(), 1, "a machine is no longer one: {both:?}");
    assert_eq!(both[0].path, "a.rs");
    assert_eq!(chat.both_changing_for("cc-2"), both);
    assert!(chat.both_changing_for("cc-3").is_empty());

    let line = chat.both_changing_line(&both[0], "cc-2");
    assert_eq!(line.icon, Icon::Overlap);
    assert_eq!(line.lead(), "! also changing");
    assert_eq!(
        (line.paths.as_slice(), line.details.as_str()),
        (&["a.rs".to_owned()][..], "with cc-1")
    );

    // The stream says so once, from the session that made it two, however often it's heard.
    feed_all(
        &mut chat,
        &two_worktrees("bob-mbp", &["a.rs", "b.rs"], &["a.rs"]),
    );
    let rows = both_rows(&chat);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session.as_deref(), Some("cc-2"));
    // Who first, then what, its path listed under it as every event's are (the user's
    // choice, 14 Nov 2026).
    assert_eq!(
        (rows[0].lead().as_str(), rows[0].details.as_str()),
        ("! cc-1 is also changing", "")
    );
    assert_eq!(rows[0].paths, ["a.rs"]);
    let drawn = chat.lines("#api", chrono::FixedOffset::east_opt(0).unwrap(), false);
    let overlap = drawn
        .iter()
        .filter_map(|line| line.system.as_ref())
        .find(|system| system.icon == Icon::Overlap)
        .expect("an overlap row");
    assert!(overlap.paths_listed && !overlap.details_apart);
    assert_eq!(overlap.marker_tone, "warning");
    assert_eq!(api(&chat).unread, 0, "it's no news to count or notify");

    // It clears when one stops, and the row stays.
    feed_all(&mut chat, &two_worktrees("bob-mbp", &["b.rs"], &["a.rs"]));
    assert!(chat.both_changing("#api").is_empty());
    assert_eq!(both_rows(&chat).len(), 1);
}

#[test]
fn a_lease_said_live_is_no_row() {
    let mut chat = Scene::in_api();
    for text in ["", "cc-x wants lease simulator"] {
        let lease = Message::new(
            "cc-x",
            Event::Custom {
                kind: zirc_proto::LEASE_KIND.into(),
                data: serde_json::Value::Null,
                text: text.into(),
            },
        );
        feed(&mut chat, &[&zirc("bob-mbp", &lease)]);
    }
    assert!(said(api(&chat)).is_empty(), "{:?}", said(api(&chat)));
}

#[test]
fn says_the_same_lines_only_when_zircd_does() {
    let mut chat = Scene::in_api();
    feed_all(&mut chat, &two_worktrees("me-mbp", &["a.rs"], &["a.rs"]));
    let both = chat.both_changing("#api");
    assert!(both[0].mine && !both[0].same_lines);
    let worktree = |name: &str| crate::presence::Worktree {
        path: format!("/src/{name}").into(),
        branch: Some(name.into()),
    };
    chat.learn_overlap(&crate::presence::LocalOverlap {
        channel: "#api".into(),
        worktrees: [worktree("api-hooks"), worktree("api-login")],
        paths: vec!["a.rs".into()],
        same_lines: [("a.rs".to_owned(), vec![[3, 9]])].into(),
        apart: Vec::new(),
        why: None,
    });
    let both = chat.both_changing("#api");
    assert!(both[0].same_lines);
    let line = chat.both_changing_line(&both[0], "cc-1");
    assert_eq!(
        (line.lead().as_str(), line.details.as_str()),
        ("! changing the same lines of", "as cc-2")
    );
}

/// zircd names a repository by its scope, `#monorepo`, which is no channel since every
/// repository's work is said in #agents: what it says of one shows there.
#[test]
fn what_zircd_says_of_a_repositorys_scope_shows_in_the_agents_room() {
    let mut chat = Scene::in_api();
    let part = |session: &str, worktree: &str, part: [u32; 2]| Message {
        repo: Some("monorepo".into()),
        ..Message::new(
            session,
            Event::Working {
                paths: vec!["a.rs".into()],
                continued: part[0] > 1,
                worktree: Some(worktree.into()),
                branch: Some(worktree.into()),
                part: Some(part),
                base: None,
                committed: Vec::new(),
            },
        )
    };
    let say = |message: &Message| line_from("me-mbp", "#agents", message).to_string();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #agents",
            &say(&part("cc-1", "api-login", [1, 2])),
            &say(&part("cc-2", "api-hooks", [2, 2])),
        ],
    );
    assert_eq!(chat.zircd_room("#monorepo"), Some("#agents"));
    assert_eq!(chat.zircd_room("#api"), Some("#api"));
    assert_eq!(chat.zircd_room("#nowhere"), None);
    assert!(!chat.both_changing("#agents")[0].same_lines);
    let worktree = |name: &str| crate::presence::Worktree {
        path: format!("/src/{name}").into(),
        branch: Some(name.into()),
    };
    chat.learn_overlap(&crate::presence::LocalOverlap {
        channel: "#monorepo".into(),
        worktrees: [worktree("api-hooks"), worktree("api-login")],
        paths: vec!["a.rs".into()],
        same_lines: [("a.rs".to_owned(), vec![[3, 9]])].into(),
        apart: Vec::new(),
        why: None,
    });
    assert!(chat.both_changing("#agents")[0].same_lines);
}

#[test]
fn a_caption_names_one_file_or_counts_several() {
    let mut chat = Scene::in_api();
    feed_all(
        &mut chat,
        &two_worktrees("bob-mbp", &["web/a.rs", "x/b.rs"], &["web/a.rs", "x/b.rs"]),
    );
    let both = chat.both_changing_for("cc-2");
    let one = chat.both_changing_caption(&both[..1], "cc-2").unwrap();
    assert_eq!(
        (
            one.lead().as_str(),
            one.paths.as_slice(),
            one.details.as_str()
        ),
        ("! also changing", &["a.rs".to_owned()][..], "with cc-1")
    );
    let two = chat.both_changing_caption(&both, "cc-2").unwrap();
    assert_eq!(
        (two.lead().as_str(), two.paths.len(), two.details.as_str()),
        ("! also changing", 0, "2 files others are changing")
    );
    assert!(chat.both_changing_caption(&[], "cc-2").is_none());
    // Ready to draw: the one line every app's caption is, a file's name or none.
    assert_eq!(
        BothChangingLine::from(one),
        BothChangingLine {
            glyph: "!".to_owned(),
            lead: "! also changing".to_owned(),
            verb: "also changing".to_owned(),
            path: "a.rs".to_owned(),
            details: "with cc-1".to_owned(),
        }
    );
    assert_eq!(BothChangingLine::from(two).path, "");
}

#[test]
fn a_fake_says_a_both_changing_line_as_the_chat_does() {
    let mut chat = Scene::in_api();
    feed_all(
        &mut chat,
        &two_worktrees("bob-mbp", &["web/a.rs"], &["web/a.rs"]),
    );
    let both = chat.both_changing_for("cc-2");
    let said = BothChangingLine::said("web/a.rs".to_owned(), &["cc-1".to_owned()], false);
    assert_eq!(
        BothChangingLine::from(chat.both_changing_line(&both[0], "cc-2")),
        said
    );
    assert_eq!(
        BothChangingLine::said("a.rs".to_owned(), &["cc-1".to_owned()], true).lead,
        "! changing the same lines of"
    );
}

#[test]
fn an_integration_says_its_line_without_coming_or_going() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":srv 353 me = #api :me @zirc-ci bob",
            ":srv 366 me #api :End",
            ":bob!u@h PRIVMSG #api :hi",
            ":zirc-github!u@h JOIN #api",
            ":zirc-github!u@h PRIVMSG #api :PR 12 merged",
            ":zirc-github!u@h QUIT :done",
            ":zirc-sentry!u@h JOIN #api",
            ":zirc-sentry!u@h PART #api",
            ":bob!u@h KICK #api zirc-ci :bye",
            ":carol!u@h JOIN #api",
            ":carol!u@h PRIVMSG #api :ok",
        ],
    );
    assert_eq!(
        said(api(&chat)),
        [
            "<bob> hi",
            "<zirc-github> PR 12 merged",
            "carol joined",
            "<carol> ok"
        ]
    );
    // It's in no one's way when we leave, and neither is an app that stays.
    feed(
        &mut chat,
        &[
            ":bob!u@h PART #api",
            ":carol!u@h PART #api",
            ":zirc-ci!u@h JOIN #api",
        ],
    );
    assert!(chat.leave_prompt("#api").contains("the last one here"));
}

#[test]
fn names_three_then_counts_the_rest() {
    let names = |n: &[&str]| {
        super::both_changing_words(&n.iter().map(|&n| n.to_owned()).collect::<Vec<_>>(), false).1
    };
    assert_eq!(
        names(&["checkout", "rate-limits"]),
        "with checkout and rate-limits"
    );
    assert_eq!(
        names(&["checkout", "rate-limits", "docs"]),
        "with checkout and 2 others"
    );
}

#[test]
fn knows_a_machines_worktrees_apart() {
    let part = |session: &str, worktree: &str, branch: &str, paths: &[&str], part: [u32; 2]| {
        Message::new(
            session,
            Event::Working {
                paths: paths.iter().map(|&p| p.to_owned()).collect(),
                continued: part[0] > 1,
                worktree: Some(worktree.into()),
                branch: Some(branch.into()),
                part: Some(part),
                base: None,
                committed: Vec::new(),
            },
        )
    };
    let mut chat = Scene::in_api();
    for paths in [&["a.rs"][..], &["a.rs", "c.rs"]] {
        feed(
            &mut chat,
            &[
                &zirc(
                    "mbp",
                    &part("cc-1", "api-login", "fix-login", paths, [1, 2]),
                ),
                &zirc(
                    "mbp",
                    &part("cc-2", "api-hooks", "retry-hooks", &["b.rs"], [2, 2]),
                ),
            ],
        );
    }
    let members = chat.presence().members("#api");
    let branches: Vec<_> = members.iter().map(|m| m.branch.as_deref()).collect();
    assert_eq!(branches, [Some("retry-hooks"), Some("fix-login")]);

    // One worktree's set said alone replaces its paths and leaves the other's.
    feed(
        &mut chat,
        &[&zirc(
            "mbp",
            &part(
                "cc-2",
                "api-hooks",
                "retry-hooks",
                &["b.rs", "d.rs"],
                zirc_proto::ALONE,
            ),
        )],
    );
    let paths: Vec<_> = chat
        .presence()
        .members("#api")
        .into_iter()
        .map(|m| m.paths)
        .collect();
    assert_eq!(paths, [vec!["b.rs", "d.rs"], vec!["a.rs", "c.rs"]]);
}

#[test]
fn follows_people_coming_going_and_renamed() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":s 353 me = #api :me @alice",
            ":s 366 me #api :End",
            ":bob!u@h JOIN #api acct :Bob Builder",
            ":alice!u@h NICK alicia",
            ":carol!u@h PRIVMSG me :hi",
            ":carol!u@h NICK caz",
            ":bob!u@h QUIT :gone",
        ],
    );
    let members: Vec<_> = api(&chat)
        .sorted_members()
        .iter()
        .map(|m| (m.prefix, m.nick.clone()))
        .collect();
    assert_eq!(members, [(Some('@'), "alicia".into()), (None, "me".into())]);
    assert_eq!(
        said(api(&chat)),
        ["bob joined", "alice is now alicia", "bob quit (gone)"]
    );
    assert_eq!(chat.realname("BOB"), Some("Bob Builder"));
    assert!(
        chat.conversation("caz").is_some(),
        "a private one follows the rename"
    );
    feed(
        &mut chat,
        &[
            ":alicia!u@h KICK #api me :out",
            ":s 354 me 42 h dave H dave :Dave D",
        ],
    );
    assert!(!api(&chat).joined && api(&chat).members.is_empty());
    assert_eq!(said(api(&chat)).last().unwrap(), "alicia removed me (out)");
    assert_eq!(chat.realname("dave"), Some("Dave D"));
}

#[test]
fn leaves_and_closes_and_doesnt_rejoin_what_was_left() {
    let mut chat = Scene::in_api();
    assert_eq!(texts(&chat.part("#api", Some("bye"))), ["PART #api bye"]);
    feed(&mut chat, &[":me!u@h PART #api :bye"]);
    assert!(!api(&chat).joined);
    assert_eq!(
        feed(&mut chat, &[":s 322 me #api 2 :", ":s 323 me :End"]),
        ["JOIN #extra"]
    );
    assert!(texts(&chat.close(1)).is_empty());
    assert_eq!(names(&chat), ["irc.zirc.test"]);
    assert_eq!(chat.selected_index(), 0);
    // Asking for it again joins it again.
    assert_eq!(texts(&chat.join(&["#api".into()])), ["JOIN #api"]);
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert_eq!(chat.selected().name, "#api");
}

#[test]
fn opens_private_conversations_and_their_history() {
    let mut chat = Scene::in_api();
    assert_eq!(texts(&chat.open("bob")), ["CHATHISTORY LATEST bob * 200"]);
    assert_eq!(chat.selected().name, "bob");
    assert!(texts(&chat.open("Bob")).is_empty());
    let sent = feed(
        &mut chat,
        &[
            ":s BATCH +t draft/chathistory-targets",
            ":s CHATHISTORY TARGETS carol 2026-10-30T11:00:00.000Z",
            ":s CHATHISTORY TARGETS NickServ 2026-10-30T11:00:00.000Z",
            ":s BATCH -t",
        ],
    );
    assert_eq!(sent, ["CHATHISTORY LATEST carol * 200"]);
    assert_eq!(names(&chat), ["irc.zirc.test", "#api", "bob", "carol"]);
    assert_eq!(chat.selected().name, "bob", "the one shown stays shown");
}

#[test]
fn keeps_services_and_the_server_in_the_status_and_the_server_channel() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":NickServ!NickServ@s NOTICE me :You are now logged in",
            ":irc.zirc.test NOTICE * :Looking up your hostname",
            ":me!u@h PRIVMSG NickServ :IDENTIFY me secret",
        ],
    );
    assert_eq!(names(&chat), ["irc.zirc.test", "#api"]);
    // In the Server channel too, but never what we said to a service, a password among it.
    let server: Vec<(&str, &EntryKind)> = chat.conversations()[0]
        .entries
        .iter()
        .map(|e| (e.from.as_str(), &e.kind))
        .collect();
    assert_eq!(
        server,
        [
            (
                "NickServ",
                &EntryKind::Notice("You are now logged in".into())
            ),
            (
                "irc.zirc.test",
                &EntryKind::Notice("Looking up your hostname".into())
            ),
        ]
    );
    assert!(
        chat.conversations()[1..]
            .iter()
            .all(|c| c.entries.is_empty())
    );
    let notices: Vec<String> = chat.status().notices.iter().map(Notice::line).collect();
    assert_eq!(
        notices,
        [
            "NickServ: You are now logged in",
            "Looking up your hostname"
        ]
    );
    assert_eq!(
        chat.status().recent(noon()).map(Notice::line).as_deref(),
        Some("Looking up your hostname")
    );
    assert!(!format!("{:?}", chat.status()).contains("secret"));
}

/// An item as a test reads it.
fn shown(item: &Item<'_>) -> String {
    match item {
        Item::Day(date) => format!("--- {date}"),
        Item::Message {
            entry,
            text,
            continues,
        } => {
            if *continues {
                format!("  {text}")
            } else {
                format!("{}: {text}", entry.from)
            }
        }
        Item::Notice { entry, text, .. } => format!("-{}- {text}", entry.from),
        Item::Action { entry, text, .. } => format!("* {} {text}", entry.from),
        Item::Event { text, .. } => format!("{EVENT_MARK} {text}"),
        Item::File { file, from, .. } => format!("[file] {from}: {} {}", file.name, file.url),
        Item::Membership(membership) => format!("· {}", membership.text()),
        Item::Landing(landing) => format!("= {:?} {}", landing.outcome, landing.text),
        Item::Pointer(pointer) => format!("↑ {}", pointer.text()),
        Item::Note { text, .. } => format!("! {text}"),
    }
}

#[test]
fn shows_shared_files_as_files() {
    let mut chat = Scene::in_api();
    let photo = Message::new(
        "desktop",
        Event::File(zirc_proto::File {
            url: "https://zirc.dev/m/acme/7Hq2xV9k".into(),
            name: "IMG_2041.jpeg".into(),
            mime: "image/jpeg".into(),
            size: 2_345_678,
            width: Some(4032),
            height: Some(3024),
            duration_ms: None,
        }),
    );
    feed(
        &mut chat,
        &[
            ":s 353 me = #api :me alice",
            ":s 366 me #api :End",
            &format!("@time=2026-10-30T11:50:00.000Z {}", zirc("alice", &photo)),
        ],
    );
    let conversation = chat.conversation("#api").unwrap();
    let items = chat.items(conversation, |time| time.date_naive());
    let shown: Vec<String> = items.iter().map(shown).collect();
    assert_eq!(
        shown,
        [
            "--- 2026-10-30",
            "[file] alice: IMG_2041.jpeg https://zirc.dev/m/acme/7Hq2xV9k"
        ]
    );
    assert_eq!(
        crate::chat::describe(&photo.event).headline,
        "shared IMG_2041.jpeg (2.3 MB)"
    );
}

#[test]
fn shows_a_channel_as_every_app_does() {
    let mut chat = Scene::in_api();
    chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
    let at = |time: &str, line: &str| format!("@time=2026-10-30T{time}.000Z {line}");
    let commit = Message::new(
        "cc-1",
        Event::Commit {
            paths: Vec::new(),
            branch: "main".into(),
            sha: "abc1234".into(),
            subject: "Fix".into(),
            repo: None,
        },
    );
    let post = Message::new(
        "desktop",
        Event::Text {
            body: "hello!".into(),
        },
    );
    let before = [
        at("10:59:00", ":mattias-mbp!u@machine/mattias JOIN #api"),
        at("11:00:00", ":alice!u@h JOIN #api"),
        at("11:00:05", ":bob!u@h JOIN #api"),
        at("11:00:10", ":carol!u@h JOIN #api"),
        at("11:01:00", ":alice!u@h PRIVMSG #api :morning"),
        at("11:01:30", ":alice!u@h PRIVMSG #api :anyone around?"),
        // A machine's reconnect: neither line shows.
        at(
            "11:02:00",
            ":mattias-mbp!u@machine/mattias QUIT :Ping timeout",
        ),
        at("11:02:20", ":mattias-mbp!u@machine/mattias JOIN #api"),
        at("11:03:00", &zirc("mattias-mbp", &commit)),
        at("11:04:00", &zirc("mattias", &post)),
        at("11:05:00", ":bob!u@h PART #api :lunch"),
        at("11:05:30", ":carol!u@h QUIT :bye"),
        // From zirc's apps: the app shows at the start of a run and where it changes.
        at("11:06:00", ":mattias!u@h PRIVMSG #api :hi all")
            .replace(".000Z ", ".000Z;+zirc.dev/client=desktop "),
        at("11:06:10", ":mattias!u@h PRIVMSG #api :back in a bit")
            .replace(".000Z ", ".000Z;+zirc.dev/client=desktop "),
        at("11:06:20", ":mattias!u@h PRIVMSG #api :on my phone now")
            .replace(".000Z ", ".000Z;+zirc.dev/client=android "),
        ":s 422 me :MOTD File is missing".to_owned(),
        ":irc.zirc.test NOTICE * :Server restarting".to_owned(),
        "ERROR :Closing link".to_owned(),
    ];
    let before: Vec<&str> = before.iter().map(String::as_str).collect();
    feed(&mut chat, &before);
    chat.disconnected("the server closed the connection");
    assert_eq!(
        chat.status().connection,
        Connection::Disconnected("the server closed the connection".into())
    );
    chat.connecting();
    chat.connected(link(true));
    let after = [
        ":me!u@h JOIN #api".to_owned(),
        at("11:10:00", ":alice!u@h PRIVMSG #api :\x01ACTION waves\x01"),
        at("11:11:00", ":dave!u@h JOIN #api"),
    ];
    let after: Vec<&str> = after.iter().map(String::as_str).collect();
    feed(&mut chat, &after);

    let lines: Vec<String> = chat
        .items(api(&chat), |time| time.date_naive())
        .iter()
        .map(shown)
        .collect();
    assert_eq!(
        lines,
        [
            "--- 2026-10-30",
            "· mattias-mbp, alice and 2 others joined",
            "alice: morning",
            "  anyone around?",
            "◆ cc-1 committed on main: Fix",
            "◆ mattias: hello!",
            "· bob and carol left",
            "mattias: hi all",
            "  back in a bit",
            "  on my phone now",
            "* alice waves",
            "· dave joined",
        ]
    );
    assert_eq!(chat.status().connection, Connection::Connected);
    let notices: Vec<&str> = chat
        .status()
        .notices
        .iter()
        .map(|n| n.text.as_str())
        .collect();
    assert_eq!(notices, ["Server restarting", "Closing link"]);
    assert_eq!(
        chat.conversations()[0].entries.len(),
        3,
        "the Server channel's: those two, and what connecting said"
    );
}

#[test]
fn sends_what_is_said() {
    let mut chat = Scene::in_api();
    assert_eq!(
        texts(&chat.say("#api", "hello", false)),
        ["PRIVMSG #api hello"]
    );
    assert_eq!(
        texts(&chat.say("#api", "waves", true)),
        ["PRIVMSG #api :\x01ACTION waves\x01"]
    );
    let post = texts(&chat.post("#api", "desktop", "on it"));
    assert!(
        post[0].starts_with("PRIVMSG #api :\x01ACTION ZIRC "),
        "{post:?}"
    );
    // With echo, what we say shows when the server sends it back.
    assert!(said(api(&chat)).is_empty());
    feed(&mut chat, &["@msgid=9 :me!u@h PRIVMSG #api :hello"]);
    assert_eq!(said(api(&chat)), ["<me> hello"]);
    assert_eq!(api(&chat).unread, 0);
}

#[test]
fn shows_its_own_messages_without_echo() {
    let mut chat = chat();
    chat.connected(link(false));
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    chat.say("#api", "hello", false);
    chat.say("bob", "hi", false);
    assert_eq!(said(api(&chat)), ["<me> hello"]);
    assert_eq!(said(chat.conversation("bob").unwrap()), ["<me> hi"]);
}

#[test]
fn says_which_app_sent_what_the_person_says() {
    let mut chat = chat_with(Options {
        client: Some("tui".into()),
        ..Options::default()
    });
    chat.connected(Link {
        multiline: Some(Limits {
            max_bytes: 4096,
            max_lines: 100,
        }),
        ..link(false)
    });
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert_eq!(
        texts(&chat.say("#api", "hello", false)),
        ["@+zirc.dev/client=tui PRIVMSG #api hello"]
    );
    // A multiline message says it once, where it opens.
    let long = texts(&chat.say("#api", "word ".repeat(100).trim(), false));
    assert!(
        long[0].starts_with("@+zirc.dev/client=tui BATCH +zirc1"),
        "{long:?}"
    );
    assert!(long[1..].iter().all(|line| !line.contains("client")));
    // zirc's own messages don't.
    let post = texts(&chat.post("#api", "cc-1", "on it"));
    assert!(!post[0].contains("client"), "{post:?}");
    // Shown at once without echo-message, as sent from here.
    assert_eq!(api(&chat).entries[0].sent_from.as_deref(), Some("tui"));
}

#[test]
fn replies_to_a_message_and_quotes_it() {
    let mut chat = chat_with(Options {
        client: Some("desktop".into()),
        ..Options::default()
    });
    chat.connected(Link {
        multiline: Some(Limits {
            max_bytes: 4096,
            max_lines: 100,
        }),
        ..link(false)
    });
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            "@msgid=m1 :bob!u@h PRIVMSG #api :can someone look at the login bug?\nit's in auth.rs",
        ],
    );
    // An ordinary message, with the message it answers in +draft/reply: plain clients
    // show the line, zirc's apps the quote.
    assert_eq!(
        texts(&chat.say_reply("#api", "m1", "on it")),
        ["@+zirc.dev/client=desktop;+draft/reply=m1 PRIVMSG #api :on it"]
    );
    // Shown at once without echo-message, as a reply.
    let mine = api(&chat).entries.last().unwrap().clone();
    assert_eq!(mine.reply_to.as_deref(), Some("m1"));
    let quote = chat.quote(api(&chat), &mine).unwrap();
    assert_eq!(quote.id, "m1");
    assert_eq!(quote.from, "bob");
    assert_eq!(quote.text, "can someone look at the login bug?");

    // A long reply goes as one multiline message, the tag where it opens.
    let long = texts(&chat.say_reply("#api", "m1", "word ".repeat(100).trim()));
    assert!(
        long[0].starts_with("@+zirc.dev/client=desktop;+draft/reply=m1 BATCH +"),
        "{long:?}"
    );
    assert!(long[1..].iter().all(|line| !line.contains("draft/reply")));

    // Someone else's reply, to a message cut short in the quote, and to one before the
    // history fetched, which has nothing to quote.
    let said = "x".repeat(QUOTE_CHARS + 20);
    feed(
        &mut chat,
        &[
            &format!("@msgid=m2 :carol!u@h PRIVMSG #api :{said}"),
            "@msgid=m3;+draft/reply=m2 :bob!u@h PRIVMSG #api :agreed",
            "@msgid=m4;+draft/reply=gone :bob!u@h PRIVMSG #api :and that",
        ],
    );
    let entry = |id: &str| api(&chat).entry(id).unwrap().clone();
    let quote = chat.quote(api(&chat), &entry("m3")).unwrap();
    assert_eq!(quote.from, "carol");
    assert_eq!(quote.text.chars().count(), QUOTE_CHARS);
    assert!(quote.text.ends_with('…'));
    assert_eq!(entry("m4").reply_to.as_deref(), Some("gone"));
    assert_eq!(chat.quote(api(&chat), &entry("m4")), None);
    // A reply to an agent's post names the session and quotes its words.
    let mut post = Message::new(
        "cc-1",
        Event::Text {
            body: "landed the fix".into(),
        },
    );
    post.name = Some("analytics".into());
    feed(
        &mut chat,
        &[
            &format!("@msgid=m5 {}", zirc("mattias-matt", &post)),
            "@msgid=m6;+draft/reply=m5 :bob!u@h PRIVMSG #api :thanks",
        ],
    );
    let entry = |id: &str| api(&chat).entry(id).unwrap().clone();
    let quote = chat.quote(api(&chat), &entry("m6")).unwrap();
    assert_eq!(
        (
            quote.from.as_str(),
            quote.name.as_str(),
            quote.text.as_str()
        ),
        ("mattias-matt", "analytics", "landed the fix")
    );
    assert_eq!(entry("m2").reply_to, None);
    assert_eq!(chat.quote(api(&chat), &entry("m2")), None);
    // A plain line from a machine, no session: the machine's name, never its nick.
    feed(
        &mut chat,
        &[
            "@msgid=m7 :mattias-mbp!u@machine/mattias PRIVMSG #api :typed by hand",
            "@msgid=m8;+draft/reply=m7 :bob!u@h PRIVMSG #api :ok",
        ],
    );
    let reply = api(&chat).entry("m8").unwrap().clone();
    let quote = chat.quote(api(&chat), &reply).unwrap();
    assert_eq!(
        (quote.from.as_str(), quote.name.as_str()),
        ("mattias-mbp", "mattias-mbp")
    );
}

#[test]
fn sends_long_text_as_one_multiline_message() {
    let mut chat = chat_with(Options {
        batch_prefix: "tui".into(),
        ..Options::default()
    });
    chat.connected(Link {
        multiline: Some(Limits {
            max_bytes: 4096,
            max_lines: 100,
        }),
        ..link(true)
    });
    let long = "word ".repeat(100);
    let lines = texts(&chat.say("#api", long.trim(), false));
    assert!(
        lines[0].starts_with("BATCH +tui1 draft/multiline #api"),
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap(), "BATCH -tui1");
    // Received, it reads as one message again.
    let mut other = self::chat();
    connected(&mut other);
    for line in &lines {
        let mut line: Line = line.parse().unwrap();
        line.source = Some("alice!u@h".into());
        other.receive(line);
    }
    assert_eq!(api(&other).entries.len(), 1);
    // Without multiline, in pieces.
    let mut plain = self::chat();
    connected(&mut plain);
    assert_eq!(plain.say("#api", long.trim(), false).len(), 2);
}

#[test]
fn sends_text_over_the_limit_as_several_messages_cut_between_paragraphs() {
    let mut chat = chat_with(Options {
        batch_prefix: "tui".into(),
        ..Options::default()
    });
    chat.connected(Link {
        multiline: Some(Limits {
            max_bytes: 60,
            max_lines: 10,
        }),
        ..link(true)
    });
    let text = "a first paragraph of some words\n\na second paragraph of more words";
    assert_eq!(chat.sends_as(text), 2);
    assert_eq!(chat.sends_as("short"), 1);
    let lines = texts(&chat.say("#api", text, false));
    let batches: Vec<&String> = lines.iter().filter(|l| l.starts_with("BATCH +")).collect();
    assert_eq!(batches.len(), 2, "{lines:?}");
}

#[test]
fn says_how_many_messages_text_goes_as_without_multiline() {
    let mut chat = Scene::in_api();
    let text = "one line\nanother line\n\nand a third";
    assert_eq!(chat.sends_as(text), 3);
    assert_eq!(chat.say("#api", text, false).len(), 3);
    let long = "word ".repeat(100);
    assert_eq!(
        chat.sends_as(long.trim()),
        chat.say("#api", long.trim(), false).len()
    );
    assert_eq!(chat.sends_as("short"), 1);
}

#[test]
fn activity_lists_mentions_replies_and_reactions_to_ours_newest_first() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            "@time=2026-11-10T09:00:00.000Z;msgid=m1 :me!u@h PRIVMSG #api :the deploy is out",
            "@time=2026-11-10T09:01:00.000Z;msgid=m2 :bob!u@h PRIVMSG #api :me: can you look?",
            "@time=2026-11-10T09:02:00.000Z;msgid=m3;+draft/reply=m1 :carol!u@h PRIVMSG #api :nice work",
            "@time=2026-11-10T09:03:00.000Z;+draft/react=🎉;+draft/reply=m1 :dave!u@h TAGMSG #api",
            // Our own reaction, and what neither names nor answers us, aren't Activity.
            "@time=2026-11-10T09:04:00.000Z;+draft/react=👍;+draft/reply=m1 :me!u@h TAGMSG #api",
            "@time=2026-11-10T09:05:00.000Z;msgid=m4 :bob!u@h PRIVMSG #api :lunch?",
        ],
    );
    let rows: Vec<(ActivityKind, String, String, String)> = chat
        .activity()
        .into_iter()
        .map(|row| (row.kind, row.what, row.key, row.text))
        .collect();
    assert_eq!(
        rows,
        [
            (
                ActivityKind::Reaction,
                "reacted 🎉".into(),
                "m1".into(),
                "the deploy is out".into()
            ),
            (
                ActivityKind::Reply,
                "replied to you".into(),
                "m3".into(),
                "nice work".into()
            ),
            (
                ActivityKind::Mention,
                "mentioned you".into(),
                "m2".into(),
                "me: can you look?".into()
            ),
        ]
    );
    // Taken back, a reaction leaves Activity.
    feed(
        &mut chat,
        &["@time=2026-11-10T09:06:00.000Z;+draft/unreact=🎉;+draft/reply=m1 :dave!u@h TAGMSG #api"],
    );
    assert!(
        chat.activity()
            .iter()
            .all(|row| row.kind != ActivityKind::Reaction)
    );
}

#[test]
fn activity_leaves_out_direct_messages_and_counts_what_is_unread() {
    let mut chat = Scene::in_api();
    chat.set_level("#api", Level::All);
    feed(
        &mut chat,
        &[
            // What a phone pushes without it being aimed at us isn't Activity: a channel set
            // to notify everything, a direct message, a reply in one.
            "@time=2026-11-10T09:00:00.000Z;msgid=m1 :bob!u@h PRIVMSG #api :lunch?",
            "@time=2026-11-10T09:01:00.000Z;msgid=m2 :carol!u@h PRIVMSG me :got a minute?",
            "@time=2026-11-10T09:02:00.000Z;msgid=m3 :me!u@h PRIVMSG carol :sure",
            "@time=2026-11-10T09:03:00.000Z;msgid=m4;+draft/reply=m3 :carol!u@h PRIVMSG me :thanks",
            // A reaction there is, as nothing else says it.
            "@time=2026-11-10T09:04:00.000Z;+draft/react=👍;+draft/reply=m3 :carol!u@h TAGMSG me",
            "@time=2026-11-10T09:05:00.000Z;msgid=m5 :bob!u@h PRIVMSG #api :me: can you look?",
        ],
    );
    let rows: Vec<(ActivityKind, String, String)> = chat
        .activity()
        .into_iter()
        .map(|row| (row.kind, row.headline, row.key))
        .collect();
    assert_eq!(
        rows,
        [
            (
                ActivityKind::Mention,
                "mentioned you in #api".into(),
                "m5".into()
            ),
            (ActivityKind::Reaction, "reacted 👍".into(), "m3".into()),
        ]
    );
    // Read up to before bob's mention, the desktops' bell has its dot, and says how many.
    let api = chat.position("#api").unwrap();
    let earlier = "2026-11-10T09:00:00.000Z".parse().unwrap();
    assert!(chat.conversations[api].mark_read(earlier, "me"));
    let button = chat.inbox_button();
    assert!(button.dot);
    assert_eq!(
        button.label,
        format!("Activity, {} unread", chat.activity_unread())
    );
    // Read up to bob's mention, in any app: it's no longer counted.
    let before = chat.badges().activity;
    assert_eq!(usize::try_from(before).unwrap(), chat.activity_unread());
    let read = "2026-11-10T09:05:00.000Z".parse().unwrap();
    assert!(chat.conversations[api].mark_read(read, "me"));
    assert!(
        chat.activity()
            .iter()
            .all(|row| row.key != "m5" || !row.unread)
    );
}

#[test]
fn keeps_reactions() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            "@msgid=m1 :alice!u@h PRIVMSG #api :ship it?",
            "@+draft/react=👍;+draft/reply=m1 :bob!u@h TAGMSG #api",
            "@+draft/react=👍;+draft/reply=m1 :me!u@h TAGMSG #api",
        ],
    );
    let reactions = api(&chat).reactions("m1");
    assert_eq!(reactions[0].nicks, ["bob", "me"]);
    assert_eq!(chat.take_touched("#api"), ["m1"]);
    let sent = texts(&chat.react("#api", "m1", "👍"));
    assert!(sent[0].contains("+draft/unreact=👍"), "{sent:?}");
}

#[test]
fn keeps_reactions_from_history() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":s 353 me = #api :me bob",
            ":s 366 me #api :End",
            ":s BATCH +h1 chathistory #api",
            "@batch=h1;msgid=m1 :alice!u@h PRIVMSG #api :ship it?",
            "@batch=h1;msgid=r1;+draft/react=👍;+draft/reply=m1 :bob!u@h TAGMSG #api",
            "@batch=h1;msgid=r2;+draft/react=🚀;+draft/reply=m1 :me!u@h TAGMSG #api",
            "@batch=h1;msgid=r3;+draft/unreact=🚀;+draft/reply=m1 :me!u@h TAGMSG #api",
            // With event-playback the past's joins and parts come too, and change nothing.
            "@batch=h1;msgid=j1 :carol!u@h JOIN #api",
            "@batch=h1;msgid=p1 :bob!u@h PART #api :later",
            "@batch=h1;msgid=p2 :me!u@h PART #api",
            ":s BATCH -h1",
        ],
    );
    let reactions = api(&chat).reactions("m1");
    assert_eq!(reactions.len(), 1, "{reactions:?}");
    assert_eq!(reactions[0].nicks, ["bob"]);
    assert_eq!(said(api(&chat)), ["<alice> ship it?"]);
    assert!(api(&chat).joined);
    assert!(api(&chat).has_member("bob"));
    assert!(!api(&chat).has_member("carol"));
}

#[test]
fn drops_a_message_trimmed_from_history_and_its_reactions() {
    let mut chat = Scene::in_api();
    let _ = chat.select(0);
    feed(
        &mut chat,
        &[
            "@msgid=m1 :alice!u@h PRIVMSG #api :the key is hunter2",
            "@msgid=m2 :alice!u@h PRIVMSG #api :me: oops",
            "@+draft/react=😱;+draft/reply=m1 :bob!u@h TAGMSG #api",
        ],
    );
    assert_eq!((api(&chat).unread, api(&chat).mentions), (2, 1));
    assert_eq!(
        texts(&chat.trim("#api", "m1", Some("a secret"))),
        ["REDACT #api m1 :a secret"]
    );
    let before = api(&chat).revision;
    feed(&mut chat, &[":alice!u@h REDACT #api m2"]);
    assert_eq!(said(api(&chat)), ["<alice> the key is hunter2"]);
    assert_eq!((api(&chat).unread, api(&chat).mentions), (1, 0));
    assert!(api(&chat).revision > before);
    // Whoever trims it, us included.
    feed(&mut chat, &[":me!u@h REDACT #api m1 :a secret"]);
    assert!(said(api(&chat)).is_empty());
    assert!(api(&chat).reactions("m1").is_empty());
    assert_eq!(api(&chat).unread, 0);
    // Nothing trimmed is news.
    assert!(chat.take_fresh().is_empty());

    // History from before it was trimmed doesn't bring it back, nor one trimmed before it
    // arrived.
    feed(
        &mut chat,
        &[
            ":bob!u@h REDACT #api m3",
            ":s BATCH +h1 chathistory #api",
            "@batch=h1;msgid=m1 :alice!u@h PRIVMSG #api :the key is hunter2",
            "@batch=h1;msgid=m3 :alice!u@h PRIVMSG #api :and the other",
            "@batch=h1;msgid=m4 :alice!u@h PRIVMSG #api :lunch?",
            ":s BATCH -h1",
        ],
    );
    assert_eq!(said(api(&chat)), ["<alice> lunch?"]);
}

#[test]
fn a_correction_takes_the_place_of_its_authors_recent_message() {
    let mut chat = Scene::in_api().into_chat();
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:50:00.000Z;msgid=m1 :alice!u@h PRIVMSG #api :lnuch?",
            "@+draft/react=👍;+draft/reply=m1 :bob!u@h TAGMSG #api",
            "@time=2026-10-30T11:51:00.000Z;msgid=m2;+draft/edit=m1 :alice!u@h PRIVMSG #api :lunch?",
        ],
    );
    assert_eq!(said(api(&chat)), ["<alice> lunch?"]);
    assert!(api(&chat).entries[0].edited);
    assert_eq!(api(&chat).reactions("m1").len(), 1);
    assert_eq!(chat.take_touched("#api"), ["m1"]);
    // History repeating both shows the correction once, in place.
    feed(
        &mut chat,
        &[
            ":s BATCH +h1 chathistory #api",
            "@batch=h1;time=2026-10-30T11:50:00.000Z;msgid=m1 :alice!u@h PRIVMSG #api :lnuch?",
            "@batch=h1;time=2026-10-30T11:51:00.000Z;msgid=m2;+draft/edit=m1 :alice!u@h PRIVMSG #api :lunch?",
            ":s BATCH -h1",
        ],
    );
    assert_eq!(said(api(&chat)), ["<alice> lunch?"]);

    // Someone else's, or one too late, or of a message not here, is an ordinary message.
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:52:00.000Z;msgid=m3;+draft/edit=m1 :bob!u@h PRIVMSG #api :not yours",
            "@time=2026-10-30T12:11:00.000Z;msgid=m4;+draft/edit=m1 :alice!u@h PRIVMSG #api :too late",
            "@time=2026-10-30T12:12:00.000Z;msgid=m5;+draft/edit=gone :alice!u@h PRIVMSG #api :orphan",
        ],
    );
    assert_eq!(
        said(api(&chat)),
        [
            "<alice> lunch?",
            "<bob> not yours",
            "<alice> too late",
            "<alice> orphan"
        ]
    );
}

#[test]
fn corrects_our_own_message_while_it_is_recent() {
    let mut chat = Scene::in_api().into_chat();
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:30:00.000Z;msgid=m1 :me!u@h PRIVMSG #api :old",
            "@time=2026-10-30T11:50:00.000Z;msgid=m2 :me!u@h PRIVMSG #api :\x02teh\x02 fix",
            "@time=2026-10-30T11:51:00.000Z;msgid=m3 :alice!u@h PRIVMSG #api :theirs",
        ],
    );
    assert!(chat.can_edit("#api", "m2"));
    assert!(!chat.can_edit("#api", "m1"));
    assert!(!chat.can_edit("#api", "m3"));
    // Up in an empty composer corrects our newest still ours, past someone else's after it.
    assert_eq!(chat.last_editable("#api").as_deref(), Some("m2"));
    assert_eq!(chat.last_editable("#nowhere"), None);
    assert_eq!(chat.edit_text("#api", "m3"), None);
    let sent = texts(&chat.say_edit("#api", "m2", "the fix"));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("+draft/edit=m2"), "{sent:?}");
    assert!(sent[0].ends_with("PRIVMSG #api :the fix"), "{sent:?}");
    // Too late: nothing sent, and a note says why.
    assert!(chat.say_edit("#api", "m1", "new").is_empty());
    assert_eq!(
        said(api(&chat)).last().map(String::as_str),
        Some(crate::chat::TOO_LATE_TO_EDIT)
    );
    // The server's echo puts it in place.
    feed(
        &mut chat,
        &["@time=2026-10-30T11:59:00.000Z;msgid=m4;+draft/edit=m2 :me!u@h PRIVMSG #api :the fix"],
    );
    assert_eq!(said(api(&chat))[1], "<me> the fix");
}

#[test]
fn drops_a_private_message_trimmed_by_either_side() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            "@msgid=p1 :bob!u@h PRIVMSG me :my password is swordfish",
            "@msgid=p2 :me!u@h PRIVMSG bob :mine is hunter2",
            ":bob!u@h REDACT me p1",
            ":me!u@h REDACT bob p2",
        ],
    );
    assert!(said(chat.conversation("bob").unwrap()).is_empty());
}

#[test]
fn says_why_the_server_wont_trim_a_message() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            "@msgid=m1 :alice!u@h PRIVMSG #api :hunter2",
            ":s FAIL REDACT REDACT_FORBIDDEN #api m1 :You are not authorized to delete this message",
        ],
    );
    assert_eq!(
        said(api(&chat)),
        [
            "<alice> hunter2",
            "couldn't trim the message: You are not authorized to delete this message"
        ]
    );
}

#[test]
fn says_nothing_of_a_server_that_wont_share_profiles() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            "@msgid=m1 :alice!u@h PRIVMSG #api :hi",
            ":s FAIL METADATA KEY_NO_PERMISSION * :You can't subscribe to metadata here",
        ],
    );
    assert_eq!(said(api(&chat)), ["<alice> hi"]);
}

#[test]
fn leaves_out_working_sets_in_plain_privmsgs() {
    let mut chat = Scene::in_api();
    let lines = [
        ":s 353 me = #api :me mac2 mac3".to_owned(),
        ":s 366 me #api :End".to_owned(),
        ":s BATCH +h1 chathistory #api".to_owned(),
        format!(
            "@batch=h1;msgid=w1;time=2026-10-01T11:00:00.000Z {}",
            old_zirc("mac2", &working("cc-2", &["old.rs"]))
        ),
        ":s BATCH -h1".to_owned(),
        // From before zircd 0.1.24, live: no longer what a machine is changing.
        old_zirc("mac3", &working("cc-3", &["x.rs"])),
        zirc("mac2", &working("cc-2", &["README.md"])),
    ];
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    feed(&mut chat, &lines);
    let members = chat.presence().members("#api");
    let sets: Vec<(&str, &[String])> = members
        .iter()
        .map(|member| (member.nick.as_str(), member.paths.as_slice()))
        .collect();
    assert_eq!(sets, [("mac2", &["README.md".to_owned()][..])]);
}

#[test]
fn explains_errors_where_the_person_is_looking() {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":s 401 me bob :No such nick"]);
    assert_eq!(said(api(&chat)).last().unwrap(), "bob No such nick");
}

#[test]
fn says_nothing_of_a_server_without_a_pusher() {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":s 401 me zirc-pusher :No such nick"]);
    assert!(said(api(&chat)).is_empty());
}

#[test]
fn rejoins_after_reconnecting() {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":bob!u@h PRIVMSG me :hi"]);
    chat.disconnected("gone");
    assert!(!api(&chat).joined);
    assert!(
        api(&chat).entries.is_empty(),
        "the connection isn't news here"
    );
    assert_eq!(
        chat.status().connection,
        Connection::Disconnected("gone".into())
    );
    let first = texts(&chat.connected(link(true)));
    assert!(
        first.contains(&"CHATHISTORY LATEST bob * 200".to_owned()),
        "{first:?}"
    );
    // The server put us in #api unasked, so it keeps our channels and puts us back itself:
    // only what was asked for at the start is still to join.
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert_eq!(feed(&mut chat, &[":s 323 me :End"]), ["JOIN #extra"]);
    assert!(api(&chat).joined);
}

#[test]
fn a_channel_left_on_another_device_while_away_stays_left() {
    let listed = [
        ":s 322 me #general 4 :",
        ":s 322 me #api 3 :",
        ":s 322 me #ops 2 :",
        ":s 323 me :End",
    ];
    // The server keeps our channels: it puts us in them before we ask for any.
    let mut chat = chat();
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #api", ":me!u@h JOIN #ops"]);
    assert_eq!(feed(&mut chat, &listed), ["JOIN #extra"]);
    feed(&mut chat, &[":me!u@h JOIN #extra"]);
    let _ = chat.select(chat.position("#ops").unwrap());

    // Away, #ops and #extra are left on another device: the server puts us back in #api
    // alone, and neither is joined again, not even the one asked for at the start.
    chat.disconnected("gone");
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert!(feed(&mut chat, &listed).is_empty());
    assert!(api(&chat).joined);
    let ops = chat.conversation("#ops").unwrap();
    assert!(!ops.joined);
    assert_eq!(ops.standing, Attendance::Left);
    assert_eq!(chat.selected().name, "#api", "what was left isn't shown");

    // Every channel left elsewhere: none is joined again, nor the defaults.
    chat.disconnected("gone");
    chat.connected(link(true));
    assert!(feed(&mut chat, &listed).is_empty());
    assert!(!api(&chat).joined);

    // Joining one again is the person's choice, as ever.
    assert_eq!(texts(&chat.join(&["#ops".into()])), ["JOIN #ops"]);
}

#[test]
fn a_channel_opened_offline_is_joined_on_connecting() {
    let listed = [
        ":s 322 me #general 4 :",
        ":s 322 me #api 3 :",
        ":s 322 me #ops 2 :",
        ":s 323 me :End",
    ];
    // The server keeps our channels: it puts us in them before we ask for any.
    let mut chat = chat();
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert_eq!(feed(&mut chat, &listed), ["JOIN #extra"]);
    feed(&mut chat, &[":me!u@h JOIN #extra"]);

    // Opened while the connection is down (the server restarting, say): nothing can go, and
    // nothing is dropped. It goes first on connecting, as ours, not as one the server kept.
    chat.disconnected("gone");
    assert!(chat.open("#ops").is_empty());
    let sent = texts(&chat.connected(link(true)));
    assert_eq!(
        sent.first().map(String::as_str),
        Some("JOIN #ops"),
        "{sent:?}"
    );
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #extra",
            ":me!u@h JOIN #ops",
        ],
    );
    assert!(feed(&mut chat, &listed).is_empty());
    let ops = chat.conversation("#ops").unwrap();
    assert!(ops.joined);
    assert_eq!(ops.standing, Attendance::Fresh);
    assert_eq!(
        chat.selected().name,
        "#ops",
        "what was opened is shown once joined"
    );

    // Opened and then left, offline: not joined after all.
    chat.disconnected("gone");
    assert!(chat.open("#general").is_empty());
    let _ = chat.part("#general", None);
    let sent = texts(&chat.connected(link(true)));
    assert!(
        !sent.iter().any(|line| line.contains("#general")),
        "{sent:?}"
    );
}

#[test]
fn a_server_that_keeps_no_channels_has_them_joined_again() {
    let listed = [":s 322 me #api 3 :", ":s 323 me :End"];
    let mut chat = chat();
    chat.connected(link(true));
    assert_eq!(feed(&mut chat, &listed), ["JOIN #extra"]);
    assert_eq!(texts(&chat.join(&["#api".into()])), ["JOIN #api"]);
    feed(&mut chat, &[":me!u@h JOIN #extra", ":me!u@h JOIN #api"]);
    chat.disconnected("gone");
    chat.connected(link(true));
    assert_eq!(feed(&mut chat, &listed), ["JOIN #extra,#api"]);
    // One asked for before the list is answered is our doing, not the server's keeping.
    feed(&mut chat, &[":me!u@h JOIN #extra", ":me!u@h JOIN #api"]);
    chat.disconnected("gone");
    chat.connected(link(true));
    assert_eq!(texts(&chat.join(&["#api".into()])), ["JOIN #api"]);
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert_eq!(feed(&mut chat, &listed), ["JOIN #extra"]);
}

#[test]
fn joins_only_by_choice_and_defaults_once() {
    let listed = [
        ":s 322 me #general 4 :",
        ":s 322 me #api 3 :the API",
        ":s 322 me #api/login 2 :",
        ":s 323 me :End of LIST",
    ];
    // Asked for: joined. Listed: left for the person to pick.
    let mut chosen = chat();
    chosen.connected(link(true));
    assert_eq!(feed(&mut chosen, &listed), ["JOIN #extra"]);

    // A branch's channel joined by choice is joined again after a reconnect, as are the
    // channels that were open.
    assert_eq!(texts(&chosen.open("#api/login")), ["JOIN #api/login"]);
    feed(&mut chosen, &[":me!u@h JOIN #api/login"]);
    chosen.disconnected("gone");
    chosen.connected(link(true));
    assert_eq!(feed(&mut chosen, &listed), ["JOIN #extra,#api/login"]);

    // Once the server no longer has it, it isn't joined again, which would make it anew.
    feed(&mut chosen, &[":me!u@h JOIN #api/login"]);
    chosen.disconnected("gone");
    chosen.connected(link(true));
    let without = [":s 322 me #general 4 :", ":s 323 me :End of LIST"];
    assert_eq!(feed(&mut chosen, &without), ["JOIN #extra"]);
}

#[test]
fn leaving_a_channel_sticks() {
    let listed = [
        ":s 322 me #general 4 :",
        ":s 322 me #ops 2 :",
        ":s 323 me :End",
    ];
    let mut chat = bare();
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #general", ":me!u@h JOIN #ops"]);
    assert_eq!(texts(&chat.part("#ops", None)), ["PART #ops"]);
    feed(&mut chat, &[":me!u@h PART #ops"]);
    // Out of the lists, with its history still here for browse.
    let rows = |chat: &Chat| -> Vec<String> {
        chat.list(SidebarSection::Channels)
            .into_iter()
            .map(|at| chat.conversations()[at].name.clone())
            .collect()
    };
    assert_eq!(rows(&chat), ["#general"]);
    // Neither a later LIST nor a reconnect joins it again; #general is still ours.
    assert!(feed(&mut chat, &listed).is_empty());
    chat.disconnected("gone");
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #general"]);
    assert!(feed(&mut chat, &listed).is_empty());
    // Until the person joins it themselves.
    assert_eq!(texts(&chat.join(&["#ops".to_owned()])), ["JOIN #ops"]);
    feed(&mut chat, &[":me!u@h JOIN #ops"]);
    assert_eq!(rows(&chat).len(), 2);
}

#[test]
fn a_person_in_no_channel_gets_the_defaults_once() {
    let listed = [
        ":s 322 me #general 4 :",
        ":s 322 me #api 3 :",
        ":s 323 me :End",
    ];
    let mut chat = bare();
    chat.connected(link(true));
    assert_eq!(feed(&mut chat, &listed), ["JOIN #general"]);
    // Leaving everything doesn't bring them back while connected.
    feed(&mut chat, &[":me!u@h JOIN #general"]);
    chat.part("#general", None);
    feed(&mut chat, &[":me!u@h PART #general"]);
    assert!(feed(&mut chat, &listed).is_empty());

    // An account the server already holds channels for joins nothing by itself.
    let mut existing = bare();
    existing.connected(link(true));
    feed(&mut existing, &[":me!u@h JOIN #api"]);
    assert!(feed(&mut existing, &listed).is_empty());
}

#[test]
fn browse_lists_what_we_are_not_in() {
    let mut chat = chat();
    chat.connected(link(true));
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #general",
            ":s 322 me #general 4 :",
            ":s 322 me #zeta 2 :",
            ":s 322 me #alpha 2 :",
            ":s 322 me #busy 9 :",
            ":s 322 me #api/login 1 :",
            ":s 322 me #dm-abc 2 :",
            ":s 323 me :End",
        ],
    );
    let names = |channels: Vec<(String, usize)>| -> Vec<String> {
        channels.into_iter().map(|(name, _)| name).collect()
    };
    // Live ones first, the most people first, then by name; none we're in.
    let browse = names(chat.browse());
    assert_eq!(browse[..4], ["#busy", "#alpha", "#zeta", "#api/login"]);
    assert!(!browse.contains(&"#general".to_owned()), "{browse:?}");
    let listed = names(chat.listed());
    assert!(listed.contains(&"#general".to_owned()), "{listed:?}");
}

#[test]
fn a_channel_left_with_history_stays_in_browse() {
    let mut chat = chat();
    chat.connected(link(true));
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #talk",
            "@time=2026-10-30T11:00:00.000Z;msgid=t1 :bob!u@h PRIVMSG #talk :done",
        ],
    );
    chat.part("#talk", None);
    feed(&mut chat, &[":me!u@h PART #talk"]);
    // We were the last in it: the server no longer lists it.
    feed(&mut chat, &[":s 322 me #other 2 :", ":s 323 me :End"]);
    let browse: Vec<String> = chat.browse().into_iter().map(|(name, _)| name).collect();
    assert!(browse.contains(&"#talk".to_owned()), "{browse:?}");
    // Without history, it would be gone.
}

#[test]
fn the_leave_prompt_says_when_we_are_the_last_one() {
    let mut chat = chat();
    chat.connected(link(true));
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #busy",
            ":bob!u@h JOIN #busy",
            ":me!u@h JOIN #alone",
        ],
    );
    assert_eq!(
        chat.leave_prompt("#busy"),
        "Leave #busy? You can join it again from Browse channels."
    );
    let alone = chat.leave_prompt("#alone");
    assert!(
        alone.starts_with("Leave #alone? You're the last one here"),
        "{alone}"
    );
    assert!(alone.contains("/join"), "{alone}");
    // One we aren't in has no one to count: the ordinary question.
    assert_eq!(
        chat.leave_prompt("#other"),
        "Leave #other? You can join it again from Browse channels."
    );
}

#[test]
fn a_channel_we_left_leaves_the_lists_and_comes_back_when_joined() {
    let mut chat = chat();
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #general", ":me!u@h JOIN #ops"]);
    let rows = |chat: &Chat| -> Vec<String> {
        chat.list(SidebarSection::Channels)
            .into_iter()
            .map(|at| chat.conversations()[at].name.clone())
            .collect()
    };
    assert_eq!(rows(&chat).len(), 2);
    chat.part("#ops", None);
    feed(&mut chat, &[":me!u@h PART #ops"]);
    assert_eq!(rows(&chat), ["#general"]);
    // Shown when left, another conversation is shown instead.
    let ops = chat.position("#ops").unwrap();
    let _ = chat.select(ops);
    chat.part("#ops", None);
    feed(&mut chat, &[":me!u@h PART #ops"]);
    assert_ne!(chat.selected().name, "#ops");
    // Kicked out is the same.
    feed(&mut chat, &[":op!u@h KICK #general me :bye"]);
    assert!(rows(&chat).is_empty(), "{:?}", rows(&chat));
    feed(&mut chat, &[":me!u@h JOIN #ops"]);
    assert_eq!(rows(&chat), ["#ops"]);
}

fn marking() -> Chat {
    let mut chat = chat_with(Options {
        show_first_channel: false,
        ..Options::default()
    });
    chat.connected(Link {
        read_marker: true,
        ..link(true)
    });
    chat
}

#[test]
fn counts_what_came_after_the_read_marker_history_included() {
    let mut chat = marking();
    let sent = feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s MARKREAD #api timestamp=2026-10-30T11:00:00.000Z",
            ":s BATCH +h1 chathistory #api",
            "@batch=h1;time=2026-10-30T10:00:00.000Z;msgid=1 :bob!u@h PRIVMSG #api :read already",
            "@batch=h1;time=2026-10-30T11:30:00.000Z;msgid=2 :alice!u@h PRIVMSG #api :me: missed this",
            "@batch=h1;time=2026-10-30T11:45:00.000Z;msgid=3 :me!u@h PRIVMSG #api :from another app",
            ":s BATCH -h1",
        ],
    );
    assert!(
        !sent.iter().any(|line| line.starts_with("MARKREAD")),
        "{sent:?}"
    );
    assert_eq!((api(&chat).unread, api(&chat).mentions), (1, 1));
    assert!(chat.take_fresh().is_empty(), "history is never fresh");

    // Reading it here tells the server, for the person's other apps.
    let at = chat.position("#api").unwrap();
    assert_eq!(
        to_server(&texts(&chat.select(at))),
        ["MARKREAD #api timestamp=2026-10-30T11:45:00.000Z"]
    );
    assert_eq!(
        api(&chat).read,
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 11, 45, 0).unwrap())
    );
    assert!(chat.select(at).is_empty(), "nothing new to mark");
}

#[test]
fn reading_in_another_app_clears_what_is_unread_here() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s MARKREAD #api *",
            "@time=2026-10-30T11:58:00.000Z;msgid=7 :alice!u@h PRIVMSG #api :one",
            "@time=2026-10-30T11:59:00.000Z;msgid=8 :alice!u@h PRIVMSG #api :two",
        ],
    );
    assert_eq!(api(&chat).unread, 2);
    feed(
        &mut chat,
        &[":s MARKREAD #api timestamp=2026-10-30T11:58:00.000Z"],
    );
    assert_eq!(api(&chat).unread, 1);
    // An older marker, as the server never sends but a slow app might cause, changes nothing.
    feed(
        &mut chat,
        &[":s MARKREAD #api timestamp=2026-10-30T10:00:00.000Z"],
    );
    assert_eq!(api(&chat).unread, 1);
    feed(
        &mut chat,
        &[":s MARKREAD #api timestamp=2026-10-30T11:59:00.000Z"],
    );
    assert_eq!(api(&chat).unread, 0);
}

#[test]
fn marks_the_conversation_shown_as_messages_arrive_and_history_once() {
    let mut chat = marking();
    feed(&mut chat, &[":me!u@h JOIN #api", ":s MARKREAD #api *"]);
    let at = chat.position("#api").unwrap();
    assert!(chat.select(at).is_empty(), "nothing to mark yet");
    let sent = feed(
        &mut chat,
        &[
            ":s BATCH +h1 chathistory #api",
            "@batch=h1;time=2026-10-30T10:00:00.000Z;msgid=1 :bob!u@h PRIVMSG #api :one",
            "@batch=h1;time=2026-10-30T10:01:00.000Z;msgid=2 :bob!u@h PRIVMSG #api :two",
            ":s BATCH -h1",
            "@time=2026-10-30T11:59:00.000Z;msgid=3 :alice!u@h PRIVMSG #api :live",
        ],
    );
    assert_eq!(
        to_server(&sent),
        [
            "MARKREAD #api timestamp=2026-10-30T10:01:00.000Z",
            "MARKREAD #api timestamp=2026-10-30T11:59:00.000Z",
        ]
    );
    assert_eq!(api(&chat).unread, 0);
}

#[test]
fn asks_how_far_private_conversations_are_read() {
    let mut chat = marking();
    let sent = feed(
        &mut chat,
        &["@time=2026-10-30T11:59:00.000Z;msgid=9 :bob!u@h PRIVMSG me :psst"],
    );
    assert_eq!(sent, ["MARKREAD bob"]);
    assert_eq!(chat.conversation("bob").unwrap().unread, 1);
    feed(
        &mut chat,
        &[":s MARKREAD bob timestamp=2026-10-30T11:59:00.000Z"],
    );
    assert_eq!(chat.conversation("bob").unwrap().unread, 0);

    // And again for each one when connected anew.
    chat.disconnected("gone");
    let sent = texts(&chat.connected(Link {
        read_marker: true,
        ..link(true)
    }));
    assert!(sent.contains(&"MARKREAD bob".to_owned()), "{sent:?}");
}

#[test]
fn sends_no_read_markers_to_a_server_without_them() {
    let mut chat = chat_with(Options {
        show_first_channel: false,
        ..Options::default()
    });
    chat.connected(link(true));
    let sent = feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            "@time=2026-10-30T11:59:00.000Z;msgid=3 :alice!u@h PRIVMSG #api :live",
            "@time=2026-10-30T11:59:00.000Z;msgid=4 :bob!u@h PRIVMSG me :psst",
        ],
    );
    assert!(
        !sent.iter().any(|line| line.starts_with("MARKREAD")),
        "{sent:?}"
    );
    let at = chat.position("#api").unwrap();
    assert!(chat.select(at).is_empty());
}

const AWAY_NOTIFIED: &str = ":s 305 me :You are no longer marked as being away";
const AWAY_ASKED: &str = ":s 306 me :You have been marked as being away";

#[test]
fn who_at_joining_says_who_is_away() {
    let scene = Scene::api_with_away();

    assert_eq!(
        scene.away_in_api(),
        [
            ("alice".into(), Some(String::new())),
            ("bob".into(), None),
            ("carol".into(), Some(String::new())),
            ("me".into(), None),
        ]
    );
}

#[test]
fn away_notify_says_who_goes_with_what_reason_and_who_returns() {
    let mut scene = Scene::api_with_away();

    scene.hears(&[":bob!u@h AWAY :lunch", ":alice!u@h AWAY"]);

    assert_eq!(scene.away("bob"), Some("lunch"));
    assert_eq!(scene.away("alice"), None);
}

#[test]
fn a_rename_keeps_away_and_quitting_ends_it() {
    let mut scene = Scene::api_with_away();

    scene.hears(&[":carol!u@h NICK caz"]);
    assert_eq!(scene.away("caz"), Some(""));

    scene.hears(&[":caz!u@h QUIT :bye"]);
    assert_eq!(scene.away("caz"), None);
}

#[test]
fn messaging_someone_away_says_why() {
    let mut scene = Scene::in_api();

    scene.hears(&[":s 301 me dave :on holiday"]);

    assert_eq!(scene.away("dave"), Some("on holiday"));
}

#[test]
fn our_own_away_counts_once_the_server_says_so() {
    let mut scene = Scene::in_api();

    let sent = texts(&scene.set_away(Some("in a meeting")));
    // Asked, not yet away: only the server says the person is.
    assert_eq!(sent, ["AWAY :in a meeting"]);
    assert_eq!(scene.status().away_here.as_deref(), Some("in a meeting"));
    assert_eq!(scene.away("me"), None);

    scene.hears(&[AWAY_ASKED]);
    assert_eq!(scene.away("me"), Some("in a meeting"));

    assert_eq!(texts(&scene.set_away(None)), ["AWAY"]);
    scene.hears(&[AWAY_NOTIFIED]);
    assert_eq!(scene.away("me"), None);
    assert_eq!(scene.status().away_here, None);
    let notes = scene.shown();
    assert_eq!(
        notes[notes.len() - 2..],
        ["You're away: in a meeting", "You're back."]
    );
}

#[test]
fn a_bare_away_gives_no_reason_and_says_while_another_app_shows_us_here() {
    let mut scene = Scene::in_api();
    let sent = texts(&scene.set_away(Some("")));
    // IRC needs a reason: the bare one, which every app says as `away` alone.
    assert_eq!(sent, [format!("AWAY {}", crate::chat::BARE_AWAY)]);
    assert_eq!(crate::chat::away_reason(crate::chat::BARE_AWAY), "");
    // Asked, not yet away: another of our apps keeps us shown here, which You says.
    assert_eq!(scene.away_note(), crate::chat::AWAY_ELSEWHERE_NOTE);
    scene.hears(&[AWAY_ASKED]);
    assert_eq!(scene.away_note(), crate::chat::AWAY_NOTE);
    let away = scene.away("me").unwrap();
    assert_eq!(crate::chat::away_label(away), "away");
    assert_eq!(crate::chat::away_title(away), "Away");
}

#[test]
fn away_is_a_persons_not_an_apps() {
    // While another of their apps is here, the server says they're not away, and so do we,
    // as every app does; this app is away here.
    let mut scene = Scene::in_api();
    scene.set_away(Some(""));

    scene.hears(&[AWAY_NOTIFIED]);

    assert_eq!(scene.away("me"), None);
    assert_eq!(scene.status().away_here.as_deref(), Some("away"));
    assert_eq!(
        scene.shown().last().unwrap(),
        "You're away here, but shown as here while another of your apps is connected and \
         not away."
    );
    // What the server says unasked isn't news.
    let before = scene.shown().len();
    scene.hears(&[AWAY_NOTIFIED]);
    assert_eq!(scene.shown().len(), before);
    // Once the other app goes away too, the server says so to every app: the person is.
    scene.hears(&[":me!u@h AWAY :away"]);
    assert_eq!(scene.away("me"), Some("away"));
}

#[test]
fn an_idle_or_locked_computer_goes_away_by_itself_and_comes_back_with_input() {
    let mut scene = Scene::in_api();
    // Not idle long enough, nothing to say.
    assert!(scene.set_idle(IDLE_AFTER_SECS - 1, false).is_empty());
    assert_eq!(
        texts(&scene.set_idle(IDLE_AFTER_SECS, false)),
        ["AWAY idle"]
    );
    // Still idle: said once.
    assert!(scene.set_idle(IDLE_AFTER_SECS + 60, false).is_empty());
    assert_eq!(texts(&scene.set_idle(0, false)), ["AWAY"]);
    // Locked at once, whatever the idle time.
    assert_eq!(texts(&scene.set_idle(0, true)), ["AWAY locked"]);
    assert_eq!(texts(&scene.set_idle(0, false)), ["AWAY"]);
}

#[test]
fn someone_going_away_says_when_they_were_last_active() {
    let mut scene = Scene::api_with_away();
    let at = |time: &str| {
        Utc.with_ymd_and_hms(2026, 11, 9, 10, time.parse().unwrap(), 0)
            .unwrap()
    };
    // Idle is said after ten minutes without input: active ten minutes before.
    scene.hears(&["@time=2026-11-09T10:30:00.000Z :bob!u@h AWAY :idle"]);
    assert_eq!(scene.active_at("bob"), Some(at("20")));
    // Told again, the first time stays.
    scene.hears(&["@time=2026-11-09T10:40:00.000Z :bob!u@h AWAY :idle"]);
    assert_eq!(scene.active_at("bob"), Some(at("20")));
    // Back, and gone: no longer away, so no time; away by hand is active then.
    scene.hears(&["@time=2026-11-09T10:45:00.000Z :bob!u@h AWAY"]);
    assert_eq!(scene.active_at("bob"), None);
    scene.hears(&["@time=2026-11-09T10:50:00.000Z :bob!u@h AWAY :lunch"]);
    assert_eq!(scene.active_at("bob"), Some(at("50")));
}

#[test]
fn going_away_by_hand_is_not_undone_by_input() {
    let mut scene = Scene::in_api();
    scene.set_away(Some("lunch"));
    assert!(scene.set_idle(IDLE_AFTER_SECS, false).is_empty());
    assert!(scene.set_idle(0, false).is_empty());
    assert_eq!(scene.status().away_here.as_deref(), Some("lunch"));
    // Asking by hand while away by idleness makes it the person's.
    let mut scene = Scene::in_api();
    scene.set_idle(IDLE_AFTER_SECS, false);
    scene.set_away(Some("lunch"));
    assert!(scene.set_idle(0, false).is_empty());
    assert_eq!(scene.status().away_here.as_deref(), Some("lunch"));
}

#[test]
fn who_is_away_stays_as_last_known_offline_and_is_told_afresh_on_connecting() {
    let mut scene = Scene::api_with_away();
    scene.hears(&[":bob!u@h AWAY :lunch"]);
    scene.set_away(Some(""));
    scene.hears(&[":me!u@h AWAY :away"]);

    scene.disconnected("gone");
    assert_eq!(scene.away("bob"), Some("lunch"));

    // This app asks again, which the server forgets it on.
    let sent = texts(&scene.connected(link(true)));
    assert_eq!(scene.away("bob"), None);
    assert_eq!(scene.away("me"), None);
    assert!(sent.contains(&"AWAY away".to_owned()), "{sent:?}");
    scene.hears(&[AWAY_ASKED]);
    assert_eq!(scene.away("me"), Some("away"));
}

#[test]
fn every_agent_event_is_a_full_message() {
    let chat = alice_and_her_machine();
    let shown = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let mut icons = Vec::new();
    for item in &shown {
        if let Item::Event { parts, .. } = item {
            assert!(!parts.compact, "{parts:?}");
            icons.push(parts.icon);
        }
    }
    assert!(
        icons.contains(&Icon::Intent) && icons.contains(&Icon::Taking),
        "{icons:?}"
    );
}

/// Alice, away, and her machine, working, about to change and taking on something, with
/// what a WHOIS said of her.
fn alice_and_her_machine() -> Chat {
    let mut chat = Scene::in_api();
    let intent = Message::new(
        "cc-1",
        Event::Intent {
            summary: "refactor login".into(),
            paths: vec!["src/auth".into()],
            expires: None,
            lasts: Some(1800),
        },
    );
    let taking = Message::new(
        "cc-1",
        Event::Text {
            body: "taking: the login bug".into(),
        },
    );
    feed(
        &mut chat,
        &[
            ":s 353 me = #api :me alice alice-mbp bob",
            ":s 366 me #api :End",
            ":s 354 me 42 machine/alice alice-mbp H alice-mbp :alice",
            ":s 354 me 42 h alice G alice :Alice",
            "@time=2026-10-30T11:50:00.000Z;msgid=1 :alice!u@h PRIVMSG #api :morning",
            &format!(
                "@time=2026-10-30T11:51:00.000Z {}",
                zirc("alice-mbp", &working("cc-1", &["src/auth.rs"]))
            ),
            &format!(
                "@time=2026-10-30T11:52:00.000Z {}",
                zirc("alice-mbp", &intent)
            ),
            &format!(
                "@time=2026-10-30T11:53:00.000Z {}",
                zirc("alice-mbp", &taking)
            ),
            // WHOIS: account, idle a minute, connected at 10:00.
            ":s 311 me alice u h * :Alice",
            ":s 330 me alice alice :is logged in as",
            ":s 317 me alice 60 1793354400 :seconds idle, signon time",
            ":s 318 me alice :End of WHOIS",
        ],
    );
    chat.into_chat()
}

#[test]
fn profiles_a_person_and_their_machines() {
    let mut chat = alice_and_her_machine();
    let alice = chat.profile("alice");
    assert_eq!(alice.machine, None);
    assert_eq!(alice.machines, ["alice-mbp"]);
    assert_eq!(alice.away.as_deref(), Some(""));
    assert_eq!(alice.channels, ["#api"]);
    assert_eq!(
        alice
            .working
            .iter()
            .map(|(channel, member)| (channel.as_str(), member.nick.as_str(), member.paths.clone()))
            .collect::<Vec<_>>(),
        [("#api", "alice-mbp", vec!["src/auth.rs".to_owned()])]
    );
    assert_eq!(alice.intents.len(), 1);
    assert_eq!(alice.intents[0].1.summary, "refactor login");
    assert_eq!(alice.claims.len(), 1);
    assert_eq!(alice.claims[0].1.task, "the login bug");
    assert_eq!(
        alice.last_said,
        Some((None, Utc.with_ymd_and_hms(2026, 10, 30, 11, 50, 0).unwrap()))
    );
    assert_eq!(alice.account.as_deref(), Some("alice"));
    assert_eq!(
        alice.idle_since,
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 11, 59, 0).unwrap())
    );
    assert_eq!(
        alice.signed_on,
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 10, 0, 0).unwrap())
    );

    // Her machine is hers.
    let mbp = chat.profile("alice-mbp");
    assert_eq!(mbp.machine, Some(("alice".into(), "alice-mbp".into())));
    assert!(mbp.machines.is_empty());

    // Asking needs a connection.
    assert_eq!(texts(&chat.whois("bob")), ["WHOIS bob"]);
    chat.disconnected("gone");
    assert!(chat.whois("bob").is_empty());
}

#[test]
fn a_card_names_them_once_and_says_where_they_last_spoke_by_its_title() {
    let mut chat = chat();
    chat.connected(link(true));
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s 354 me 42 h carol H carol :Carol Danvers",
            "@time=2026-10-30T11:50:00.000Z;msgid=c1 :carol!u@h PRIVMSG me :lunch?",
        ],
    );
    let about = |chat: &Chat| {
        chat.profile("carol")
            .card(noon(), |at| at.naive_utc())
            .about
    };
    // Her real name is what she shows as, so it's the card's heading, not a line under it;
    // her nick is the line under it.
    assert_eq!(chat.profile("carol").name, "Carol Danvers");
    let card = chat.profile("carol").card(noon(), |at| at.naive_utc());
    assert_eq!(card.title, "Carol Danvers");
    assert_eq!(card.handle.as_deref(), Some("carol"));
    assert_eq!(about(&chat), ["last said 11:50 in Carol Danvers"]);
    // In the conversation where she said it, it's here.
    chat.open("carol");
    assert_eq!(about(&chat), ["last said 11:50 here"]);
}

#[test]
fn says_the_same_on_every_apps_card() {
    let chat = alice_and_her_machine();
    let alice = chat.profile("alice");
    let card = alice.card(noon(), |at| at.naive_utc());
    assert_eq!(card.title, "alice");
    // Her name is her nick: no handle under it.
    assert_eq!(card.handle, None);
    // Her machine is its own section, with its sessions, as her row in People has it.
    let machines: Vec<String> = chat
        .card_machines("alice")
        .into_iter()
        .map(|machine| machine.name)
        .collect();
    assert_eq!(machines, ["alice-mbp"]);
    assert_eq!(card.standing, "away");
    assert!(card.away);
    assert_eq!(card.presence, Presence::Away);
    assert_eq!(
        card.about,
        [
            "account alice · idle 1m · on since 10:00",
            "in #api",
            // #api is the conversation shown.
            "last said 11:50 here",
        ]
    );
    let work: Vec<(&str, &[String])> = card
        .work
        .iter()
        .map(|work| (work.heading.as_str(), work.lines.as_slice()))
        .collect();
    assert_eq!(
        work,
        [
            (
                "changing",
                ["#api alice-mbp cc-1: 1 file".to_owned()].as_slice()
            ),
            (
                "about to change",
                ["#api alice-mbp/cc-1: refactor login".to_owned()].as_slice()
            ),
            (
                "taken on",
                ["#api alice-mbp/cc-1: the login bug".to_owned()].as_slice()
            ),
        ]
    );
    assert_eq!(card.agent, None);
    let card = chat.profile("alice-mbp").card(noon(), |at| at.naive_utc());
    assert_eq!(card.title, "alice · alice-mbp");
    assert_eq!(card.standing, "here");
    assert!(!card.away);
    assert_eq!(card.presence, Presence::Here);
    assert_eq!(card.agent.as_deref(), Some("Agent of alice, on alice-mbp"));
    assert_eq!(
        card.with_session("cc-1").agent.as_deref(),
        Some("Agent of alice, on alice-mbp · cc-1")
    );
    let profile = chat.profile("alice-mbp");
    let mine = profile.session_card("cc-1", noon(), |at| at.naive_utc());
    assert_eq!(
        mine.agent.as_deref(),
        Some("Agent of alice, on alice-mbp · cc-1")
    );
    assert!(!mine.work.is_empty());
    let other = profile.session_card("cc-2", noon(), |at| at.naive_utc());
    assert!(other.work.is_empty());
}

#[test]
fn names_agents_first_on_every_kind_of_row() {
    let mut chat = alice_and_her_machine();
    let mut post = Message::new(
        "cc-1",
        Event::Text {
            body: "done".into(),
        },
    );
    post.name = Some("analytics".into());
    post.tool = Some(zirc_proto::CLAUDE_CODE.into());
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:58:00.000Z;msgid=9 :alice-mbp!u@machine/alice PRIVMSG #api :"
                .to_owned()
                + &zirc_proto::encode(&post).unwrap(),
        ]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>(),
    );
    let profiles: zirc_proto::profile::Profiles =
        serde_json::from_str(r#"{"alice": {"name": "Alice", "avatar": null}}"#).unwrap();
    chat.take_profiles(&profiles, "https://zirc.dev/");

    // The one place an agent's name line is made: agent first.
    assert_eq!(
        agent_name(Some("analytics"), "Alice"),
        ("analytics".to_owned(), Some("Alice's agent".to_owned()))
    );
    assert_eq!(
        agent_name(None, "Alice"),
        ("Alice's agent".to_owned(), None)
    );
    assert_eq!(
        agent_label(Some("analytics"), "Alice"),
        "analytics, agent of Alice"
    );

    let conversation = chat.conversation("#api").unwrap();
    let posted = conversation
        .entries
        .iter()
        .find(|entry| entry.id.as_deref() == Some("9"))
        .unwrap();
    // A message row: the session's name, then its owner's agent by their profile's name.
    let speaker = chat.speaker(posted);
    assert_eq!(
        (speaker.name.as_str(), speaker.detail.as_deref()),
        ("analytics", Some("Alice's agent"))
    );
    assert_eq!(speaker.label, "analytics, agent of Alice");

    // An activity row says whose agent too, and its plain text never the machine's nick.
    let rows = chat.items(conversation, |time| time.date_naive());
    let intent = rows
        .iter()
        .find_map(|item| match item {
            Item::Event {
                parts, text, entry, ..
            } if parts.icon == Icon::Intent => Some((parts, text, entry)),
            _ => None,
        })
        .unwrap();
    // Said before the session had a name: the row has the name it's given since, as its
    // speaker does, never its id.
    assert_eq!(intent.0.actor, "analytics");
    // Its owner is named as the speaker names them, by their name.
    assert_eq!(intent.0.actor_detail.as_deref(), Some("Alice's agent"));
    assert!(!intent.1.contains("alice-mbp"), "{}", intent.1);
    let speaker = chat.speaker(intent.2);
    assert_eq!(
        (speaker.name.as_str(), speaker.detail.as_deref()),
        ("analytics", Some("Alice's agent"))
    );

    // The card of an agent row describes the session: what it's called (its task, with no
    // title card) and its owner's, its handle, machine and id, what it's doing, whose agent
    // it is and its tool.
    let card = chat.session_card("alice-mbp", "cc-1", noon(), |at| at.naive_utc());
    assert_eq!(card.title, "the login bug · Alice");
    assert_eq!(card.ids.as_deref(), Some("analytics · alice-mbp · cc-1"));
    assert!(
        card.doing
            .as_deref()
            .is_some_and(|doing| doing.starts_with("the login bug · ")),
        "{:?}",
        card.doing
    );
    assert_eq!(card.agent.as_deref(), Some("Alice's agent · Claude Code"));
    assert!(!card.work.is_empty());
    // Away is its owner's, and says so.
    assert_eq!(card.standing, "here");
    feed(&mut chat, &[":alice-mbp!u@machine/alice AWAY :lunch"]);
    let card = chat.session_card("alice-mbp", "cc-1", noon(), |at| at.naive_utc());
    assert_eq!(card.standing, "Alice is away: lunch");

    // In the people list, the session sits under its owner with its own avatar.
    let rows = chat.people_rows("#api");
    let alice = rows.iter().find(|row| row.person.nick == "alice").unwrap();
    let session = &alice.machines[0].sessions[0];
    assert_eq!(session.label, "the login bug");
    assert!(matches!(
        &session.avatar,
        Avatar::Agent { owner, tool, .. } if owner == "alice" && tool.as_deref() == Some("claude-code")
    ));
}

#[test]
fn both_changing_is_the_wire_record_of_the_phones() {
    // The phones read these as UniFFI records of the same fields, mobile/core/src/view.rs.
    let both = BothChanging {
        path: "client.ts".into(),
        who: vec![BothChanger {
            nick: "alice-mbp".into(),
            session: Some("cc-1".into()),
            name: "analytics".into(),
        }],
        same_lines: false,
        mine: true,
    };
    assert_eq!(
        format!("{both:?}"),
        r#"BothChanging { path: "client.ts", who: [BothChanger { nick: "alice-mbp", session: Some("cc-1"), name: "analytics" }], same_lines: false, mine: true }"#
    );
}

#[test]
fn the_rows_word_the_machine_the_dot_the_pill_and_a_sessions_caption() {
    let chat = alice_and_her_named_session();
    let rows = chat.people_rows("#api");
    let alice = rows.iter().find(|row| row.person.nick == "alice").unwrap();
    let session = &alice.machines[0].sessions[0];
    assert_eq!(alice.machines[0].name(), "alice-mbp");
    assert_eq!(alice.machines[0].away_label(), None);
    assert_eq!(
        (alice.pill(), alice.away_label()),
        (None, Some("away".to_owned()))
    );
    assert_eq!(chat.session_both(session), None);
}

#[test]
fn a_people_row_is_the_wire_record_every_app_draws() {
    let chat = alice_and_her_named_session();
    let rows = chat.member_rows("#api");
    let alice = rows.iter().find(|row| row.nick == "alice").unwrap();
    // Worded as her row is: its line, her dot, her away and her face.
    let row = chat
        .people_rows("#api")
        .into_iter()
        .find(|row| row.person.nick == "alice")
        .unwrap();
    assert_eq!(alice.label, row.label());
    assert_eq!(alice.presence, row.standing().presence());
    assert_eq!(alice.away_label.as_deref(), Some("away"));
    // What every app draws after her name, muted, and at her row's end; ours says `you`.
    assert_eq!(
        (alice.detail.as_deref(), alice.aside.as_deref()),
        (Some("· 1 agent"), Some("away"))
    );
    let me = rows.iter().find(|row| row.me).unwrap();
    assert_eq!(
        (me.detail.as_deref(), me.aside.as_deref()),
        (None, Some("you"))
    );
    assert_eq!(
        (alice.pill.as_deref(), alice.op, alice.me),
        (None, false, false)
    );
    assert!(matches!(&alice.face, Some(Face::Person { nick, .. }) if nick == "alice"));
    // Her machine, by its own name, and its session ready to open, its face its row's.
    let machine = &alice.machines[0];
    assert_eq!(
        (machine.nick.as_str(), machine.name.as_str()),
        ("alice-mbp", "alice-mbp")
    );
    assert_eq!(machine.away_label, None);
    let session = &machine.sessions[0];
    assert_eq!(
        (
            session.conversation.as_str(),
            session.short.as_str(),
            session.label.as_str(),
            session.machine.as_str()
        ),
        ("alice-mbp/cc-1", "cc-1", "the login bug", "alice-mbp")
    );
    assert!(matches!(
        &session.face,
        Some(Face::Agent { owner, session: Some(id), .. }) if owner == "alice" && id == "cc-1"
    ));
    assert_eq!(session.both, None);
}

#[test]
fn a_machine_without_a_name_goes_by_its_nick_and_away_is_worded_once() {
    let chat = alice_and_her_machine();
    let mut rows = chat.people_rows("#api");
    let alice = rows
        .iter_mut()
        .find(|row| row.person.nick == "alice")
        .expect("alice is listed");
    alice.machines[0].person.machine = None;
    assert_eq!(alice.machines[0].name(), "alice-mbp");
    alice.person.away = Some("lunch".to_owned());
    assert_eq!(alice.away_label().as_deref(), Some("away: lunch"));
    assert_eq!(alice.standing().presence(), crate::chat::Presence::Away);
    assert_eq!(alice.pill(), None);
    alice.person.app = Some("ci".to_owned());
    assert_eq!(alice.pill().as_deref(), Some("APP"));
}

/// What alice-mbp's session cc-1 says at `time`, as a line.
fn alice_session_says(time: &str, event: Event) -> String {
    let mut message = Message::new("cc-1", event);
    message.name = Some("analytics".into());
    message.tool = Some(zirc_proto::CLAUDE_CODE.into());
    format!(
        "@time=2026-10-30T{time}.000Z {}",
        old_zirc("alice-mbp", &message)
    )
}

/// alice's machine, her session cc-1 named `analytics` and heard at 11:54, and her
/// profile's name.
fn alice_and_her_named_session() -> Chat {
    let mut chat = alice_and_her_machine();
    let profiles: zirc_proto::profile::Profiles =
        serde_json::from_str(r#"{"alice": {"name": "Alice", "avatar": null}}"#).unwrap();
    chat.take_profiles(&profiles, "https://zirc.dev/");
    let said = alice_session_says(
        "11:54:00",
        Event::Text {
            body: "on it".into(),
        },
    );
    feed(&mut chat, &[&said]);
    chat
}

#[test]
fn an_agent_card_with_no_title_card_leads_with_its_task_its_handle_under_it() {
    let chat = alice_and_her_named_session();
    // Before its card says a title: the task it's on leads, its handle under it (the user's
    // choice, 25 Nov 2026), with where it runs, what it's doing and when it was heard.
    let card = chat.agent_card("alice-mbp", "cc-1", noon());
    assert_eq!(
        card,
        AgentCard {
            session: "cc-1".into(),
            title: "the login bug".into(),
            titled: true,
            handle: "analytics".into(),
            whose: Some("Alice's agent".into()),
            after_handle: Some(" · Alice's agent".into()),
            tool: Some("claude-code".into()),
            place: Some("Claude Code · alice-mbp".into()),
            standing: None,
            now: Some("✎ about to change src/auth · 8m".into()),
            now_lead: Some("✎ about to change".into()),
            now_paths: Some(" src/auth".into()),
            now_ago: Some(" · 8m".into()),
            task: Some("refactor login".into()),
            heard: Some("Active now".into()),
            active: true,
            mine: false,
            message_label: "Message".into(),
        }
    );
}

#[test]
fn an_agent_card_has_the_full_title_where_it_runs_and_what_its_doing() {
    let mut chat = alice_and_her_named_session();
    // `zirc name --title` sends its card; an older one from history never replaces it.
    let title = |title: &str| zirc_proto::session::SessionCard {
        session: "cc-1".into(),
        name: Some("analytics".into()),
        title: Some(title.into()),
        tool: Some(zirc_proto::CLAUDE_CODE.into()),
    };
    // Sent as zircd sends it: the card alone, which names the session and its tool.
    let sent = |time: &str, card: zirc_proto::session::SessionCard| {
        let message = Message::new("cc-1", card.event());
        format!(
            "@time=2026-10-30T{time}.000Z {}",
            old_zirc("alice-mbp", &message)
        )
    };
    let new = sent("11:55:00", title("Fix the login bug on phones"));
    let old = sent("11:00:00", title("Look into login"));
    let unread = chat.conversation("#api").unwrap().unread;
    feed(&mut chat, &[&new, &old]);
    // A title card is nothing said: nothing unread, and never the list's preview.
    assert_eq!(chat.conversation("#api").unwrap().unread, unread);
    let preview = chat.preview("#api").unwrap();
    assert!(!preview.text.contains("login bug on phones"), "{preview:?}");
    // Nor something search finds.
    let hits = chat.search("#api", "bug on phones").hits;
    assert!(hits.is_empty(), "{hits:?}");
    let card = chat.agent_card("alice-mbp", "cc-1", noon());
    assert_eq!(card.title, "Fix the login bug on phones");
    assert!(card.titled);
    assert_eq!(card.handle, "analytics");
    assert_eq!(card.tool.as_deref(), Some("claude-code"));
    assert_eq!(card.place.as_deref(), Some("Claude Code · alice-mbp"));
    // Its live intent, said at 11:52, with its task under it.
    assert_eq!(card.now.as_deref(), Some("✎ about to change src/auth · 8m"));
    // In parts, for apps that set the paths in mono and the age muted, drawn as they come.
    assert_eq!(
        (
            card.now_lead.as_deref(),
            card.now_paths.as_deref(),
            card.now_ago.as_deref()
        ),
        (Some("✎ about to change"), Some(" src/auth"), Some(" · 8m"))
    );
    assert_eq!(card.after_handle.as_deref(), Some(" · Alice's agent"));
    assert_eq!(card.task.as_deref(), Some("refactor login"));
    assert_eq!(card.heard.as_deref(), Some("Active now"));
    assert!(card.active);

    // Once the intent's half hour is up, what it's taking on.
    let later = noon() + chrono::Duration::minutes(40);
    let card = chat.agent_card("alice-mbp", "cc-1", later);
    assert_eq!(card.now.as_deref(), Some("⟶ the login bug · 47m"));
    assert_eq!(card.task, None);

    // Done with it: nothing in progress, and when it was last heard.
    let done = alice_session_says(
        "12:10:00",
        Event::Text {
            body: "done: the login bug".into(),
        },
    );
    feed(&mut chat, &[&done]);
    let card = chat.agent_card("alice-mbp", "cc-1", later);
    assert_eq!(card.now.as_deref(), Some("Nothing in progress"));
    assert_eq!(card.heard.as_deref(), Some("Last heard 30m ago"));
    assert!(!card.active);
    // Every word on it is the model's, buttons included.
    assert_eq!((MESSAGE, OPEN_SESSION), ("Message", "Open session"));
    // The title cards themselves are never rows of the conversation.
    let conversation = chat.conversation("#api").unwrap();
    let items = chat.items(conversation, |time| time.date_naive());
    assert!(
        !items
            .iter()
            .map(shown)
            .any(|row| row.contains("Fix the login bug on phones")
                || row.contains("Look into login")),
        "{:?}",
        items.iter().map(shown).collect::<Vec<_>>()
    );
}

#[test]
fn browse_channels_filters_by_what_is_typed_names_that_start_with_it_first() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":s 322 me #design 9 :",
            ":s 322 me #api/login 2 :",
            ":s 322 me #login 1 :",
            ":s 322 me #web 4 :",
            ":s 323 me :End of /LIST",
        ],
    );
    let names = |query: &str| -> Vec<String> {
        chat.browse_matching(query)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    };
    assert_eq!(names(""), ["#design", "#web", "#api/login", "#login"]);
    // A room's own name counts as its start; a leading # and the case don't matter.
    assert_eq!(names("#LOG"), ["#api/login", "#login"]);
    assert_eq!(names("sig"), ["#design"]);
    assert!(names("nothing").is_empty());
}

#[test]
fn jump_to_offers_to_create_a_channel_nothing_lists_or_join_one_the_server_does() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[":s 322 me #design 9 :", ":s 323 me :End of /LIST"],
    );
    // Nothing lists #launch: joining it makes it.
    let offer = chat.join_offer(" launch ").unwrap();
    assert_eq!(
        (offer.name.as_str(), offer.label.as_str(), offer.create),
        ("#launch", "Create #launch", true)
    );
    assert_eq!(chat.join_offer("#Launch").unwrap().name, "#Launch");
    // One the server lists: Join, last, after Browse's own row for it.
    let offer = chat.join_offer("#DESIGN").unwrap();
    assert_eq!(
        (offer.label.as_str(), offer.create),
        ("Join #DESIGN", false)
    );
    // We're in #api: nothing to offer.
    assert!(chat.join_offer("api").is_none());
    // Not a channel's name.
    assert!(chat.join_offer("").is_none());
    assert!(chat.join_offer("two words").is_none());
    assert!(chat.join_offer("#a,b").is_none());
}

#[test]
fn jump_to_says_nothing_is_called_that_only_when_it_offers_nothing() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[":s 322 me #design 9 :", ":s 323 me :End of /LIST"],
    );
    let nothing = |query: &str| chat.jump_to(query).nothing;
    // Typed nothing yet, or a conversation of ours, a channel to join or one to make.
    assert_eq!(nothing(""), None);
    assert_eq!(nothing("api"), None);
    assert_eq!(nothing("desi"), None);
    assert_eq!(nothing("launch"), None);
    // No channel's name, so no Create row: nothing at all.
    assert_eq!(
        nothing(" two words "),
        Some(crate::chat::nothing_called("two words"))
    );
}

#[test]
fn a_title_opens_its_people_or_someones_card_in_every_app() {
    use crate::chat::TitleOpens;
    let chat = Scene::in_api();
    assert_eq!(chat.title_opens("#api"), TitleOpens::People);
    assert_eq!(
        chat.title_opens("bob"),
        TitleOpens::Card {
            nick: "bob".into(),
            session: None
        }
    );
    assert_eq!(
        chat.title_opens("henrik-mbp/cc-1"),
        TitleOpens::Card {
            nick: "henrik-mbp".into(),
            session: Some("cc-1".into())
        }
    );
}

#[test]
fn jump_to_rows_are_a_title_and_the_sidebars_mention_badge() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            "@time=2026-10-30T11:02:00.000Z;msgid=w2 :bob!u@h PRIVMSG #web :me: look",
        ],
    );
    let _ = chat.select(chat.position("#api").unwrap());
    let rows = chat.jump_to("").conversations;
    let web = rows.iter().find(|row| row.name == "#web").unwrap();
    assert_eq!(web.title, chat.title("#web"));
    assert_eq!((web.mentions, web.badge.as_deref()), (1, Some("1")));
    let api = rows.iter().find(|row| row.name == "#api").unwrap();
    assert_eq!((api.mentions, api.badge.as_deref()), (0, None));
}

#[test]
fn jump_to_heads_browse_channels_only_over_channels_to_join() {
    let mut chat = Scene::in_api();
    // Nothing to join: no heading.
    let jump = chat.jump_to("");
    let names: Vec<&str> = jump
        .conversations
        .iter()
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(names, ["#api"]);
    assert!(jump.browse.is_empty() && jump.browse_heading.is_none());
    feed(
        &mut chat,
        &[":s 322 me #design 9 :", ":s 323 me :End of /LIST"],
    );
    let jump = chat.jump_to("des");
    assert_eq!(jump.browse_heading.as_deref(), Some(BROWSE_CHANNELS));
    assert_eq!(jump.browse[0].name, "#design");
    // What it offers to make is marked so, the same in every app.
    let make = chat.jump_to("launch").offer.unwrap();
    assert_eq!(
        (make.label.as_str(), make.mark.as_str()),
        ("Create #launch", "+")
    );
}

#[test]
fn a_badge_says_its_count_up_to_ninety_nine() {
    assert_eq!(badge_label(7), "7");
    assert_eq!(badge_label(99), "99");
    assert_eq!(badge_label(100), "99+");
}

#[test]
fn keeps_zircs_apps_apart_from_people() {
    let mut chat = alice_and_her_machine();
    feed(
        &mut chat,
        &[
            ":zirc-alerts!u@h JOIN #api zirc-alerts :zirc",
            ":s 353 me = #api :me alice alice-mbp bob zirc-alerts",
            ":s 366 me #api :End",
        ],
    );
    let people: Vec<String> = chat
        .people_rows("#api")
        .into_iter()
        .map(|row| row.person.nick)
        .collect();
    assert!(
        !people.iter().any(|nick| nick == "zirc-alerts"),
        "{people:?}"
    );
    assert!(people.iter().any(|nick| nick == "alice"));
    let apps = chat.app_rows("#api");
    assert_eq!(apps.len(), 1);
    assert_eq!(
        (apps[0].nick.as_str(), apps[0].label.as_str()),
        ("zirc-alerts", "Alerts")
    );
    assert!(matches!(&apps[0].avatar, Avatar::App { glyph, .. } if glyph == "AL"));
    // Ready to draw, as every app's Apps part does: its glyph on a square.
    assert_eq!(
        chat.app_members("#api"),
        [AppMember {
            nick: "zirc-alerts".to_owned(),
            label: "Alerts".to_owned(),
            face: Face::App {
                glyph: "AL".to_owned()
            },
        }]
    );

    // Once it has posted as CI, it's CI here as in its rows: one app, one name.
    let post = ci_post(LANDING_KIND, serde_json::json!({}), "landed");
    feed_all(&mut chat, &[zirc("zirc-alerts", &post)]);
    let apps = chat.app_rows("#api");
    assert_eq!(apps[0].label, "CI");
    assert!(matches!(&apps[0].avatar, Avatar::App { glyph, .. } if glyph == "CI"));
    let entries = &chat.conversation("#api").unwrap().entries;
    assert_eq!(chat.speaker(entries.last().unwrap()).name, "CI");
}

#[test]
fn names_an_app_as_letters_when_short_and_a_word_when_not() {
    assert_eq!(app_name("ci"), "CI");
    assert_eq!(app_name("alerts"), "Alerts");
    assert_eq!(app_name(""), "");
}

#[test]
fn zircs_system_account_is_zirc_everywhere_and_its_messages_take_no_replies() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":zirc-system!u@h PRIVMSG me :Your trial ends in 3 days",
            ":zirc-system!u@h NOTICE #api :#api is now public",
        ],
    );
    let system = zirc_proto::names::SYSTEM;
    assert_eq!(app_label(system, None).as_deref(), Some(ZIRC_LABEL));
    assert_eq!(app_name(ZIRC_LABEL), zirc_proto::NAME);
    // Its messages' heads, and its notices', name zirc, never its nick.
    assert_eq!(chat.sender_name(system, None), "Zirc");
    let dm = &chat.conversation(system).unwrap().entries;
    let head = chat.speaker(dm.last().unwrap());
    assert_eq!(head.name, "Zirc");
    assert!(matches!(&head.avatar, Avatar::App { glyph, .. } if glyph == "Z"));
    let notice = chat.conversation("#api").unwrap().entries.last().unwrap();
    assert!(matches!(notice.kind, EntryKind::Notice(_)));
    assert_eq!(chat.speaker(notice).name, "Zirc");
    // Its direct conversation is titled zirc, in the list and the composer alike.
    assert_eq!(chat.title(system), "Zirc");
    assert_eq!(chat.short_title(system), "Zirc");
    // It reads no replies: no composer, a line saying so, and no one to go to instead.
    assert!(!chat.can_message(system));
    assert!(!chat.can_message("Zirc-System"));
    assert!(chat.can_message("#api"));
    assert_eq!(
        chat.read_only_note(system),
        Some(ReadOnlyNote {
            text: "Zirc's notices can't be answered.".into(),
            button: String::new(),
            dm: String::new(),
            remove: String::new(),
        })
    );
    assert_eq!(chat.read_only_note("#api"), None);
    // zirc's other apps' conversations are titled as their messages are headed too.
    assert_eq!(chat.title("zirc-ci"), "CI");
}

#[test]
fn lists_each_person_once_with_their_machines_and_sessions_under_them() {
    let mut chat = alice_and_her_machine();
    feed(
        &mut chat,
        &[
            ":carol-laptop!u@machine/carol JOIN #api",
            ":s 354 me 42 machine/carol carol-laptop H carol-laptop :carol",
        ],
    );
    let rows = chat.people_rows("#api");
    let shown: Vec<(String, bool, Vec<String>)> = rows
        .iter()
        .map(|row| {
            let machines = row
                .machines
                .iter()
                .map(|machine| {
                    let sessions: Vec<String> = machine
                        .sessions
                        .iter()
                        .map(|s| format!("{} {}", s.short, s.task.clone().unwrap_or_default()))
                        .collect();
                    format!("{}: {}", machine.person.label(), sessions.join(", "))
                })
                .collect();
            (row.label(), row.here, machines)
        })
        .collect();
    assert_eq!(
        shown,
        // Who's here first, then alice, who's away, then carol, gone.
        [
            ("bob".to_owned(), true, vec![]),
            ("me".to_owned(), true, vec![]),
            (
                "alice · 1 agent".to_owned(),
                true,
                vec!["alice · alice-mbp: cc-1 the login bug".to_owned()]
            ),
            // carol herself isn't here, only her laptop.
            (
                "carol".to_owned(),
                false,
                vec!["carol · carol-laptop: ".to_owned()]
            ),
        ]
    );
    assert_eq!(rows[2].agents(), 1);
}

#[test]
fn a_motd_that_says_messages_are_logged_shows_until_the_connection_ends() {
    let mut chat = chat();
    let channels = format!(":s 372 me :- {}", Logged::Channels.notice());
    feed(
        &mut chat,
        &[
            ":s 375 me :- s Message of the day - ",
            ":s 372 me :- zirc · acme's private server · zirc.dev",
            &channels,
            ":s 376 me :End of /MOTD command",
        ],
    );
    connected(&mut chat);
    assert_eq!(chat.status().logged, Some(Logged::Channels));
    chat.disconnected("gone");
    assert_eq!(chat.status().logged, None);
    // Turned off, the server says nothing of it on connecting again.
    feed(
        &mut chat,
        &[":s 372 me :- zirc · acme's private server · zirc.dev"],
    );
    connected(&mut chat);
    assert_eq!(chat.status().logged, None);
}

/// What agent `session` on `machine` answers us with `zirc dm`, at 12:`minute`.
fn dm(machine: &str, session: &str, text: &str, minute: u32) -> String {
    let message = Message::new(
        session,
        Event::Text {
            body: text.to_owned(),
        },
    );
    format!(
        "@time=2026-10-30T12:{minute:02}:00.000Z;msgid={machine}{minute} :{} PRIVMSG me :{}",
        source(machine),
        zirc_proto::encode(&message).unwrap()
    )
}

#[test]
fn files_what_agents_say_by_session_ours_and_a_teammates() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            &dm("me-mbp", "cc-1531f8", "done with the tests", 1),
            &dm("me-mbp", "cc-77aa01", "which branch?", 2),
            // A teammate's agent's is its own conversation too; a machine's own word isn't
            // an agent's.
            &dm("bob-mbp", "cc-0000ff", "hello", 3),
            &dm("me-mbp", zirc_proto::NO_SESSION, "from the machine", 4),
        ],
    );
    assert_eq!(
        names(&chat),
        [
            "irc.zirc.test",
            "#api",
            "bob-mbp/cc-0000ff",
            "me-mbp",
            "me-mbp/cc-1531f8",
            "me-mbp/cc-77aa01"
        ]
    );
    let first = chat.conversation("me-mbp/cc-1531f8").unwrap();
    assert_eq!(said(first).len(), 1);
    assert_eq!(first.unread, 1);
    assert_eq!(first.mentions, 1, "a private message counts as a mention");
    assert_eq!(chat.title("me-mbp/cc-1531f8"), "cc-1531f8");
    assert_eq!(chat.title("me-mbp"), "me · me-mbp");
    assert_eq!(chat.title("bob-mbp/cc-0000ff"), "cc-0000ff · bob");
    assert_eq!(chat.title("#api"), "#api");
}

#[test]
fn says_to_an_agent_what_its_machine_hands_it() {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":me-mbp!u@machine/me JOIN #api"]);
    let sent = chat.open("me-mbp/cc-1531f8");
    assert_eq!(texts(&sent), ["CHATHISTORY LATEST me-mbp * 200"]);
    assert_eq!(chat.selected().name, "me-mbp/cc-1531f8");
    assert_eq!(
        texts(&chat.say("me-mbp/cc-1531f8", "run the tests", false)),
        ["PRIVMSG me-mbp :cc-1531f8 run the tests"]
    );
    // The server's echo comes back to the session's conversation, as it was typed there.
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T12:05:00.000Z;msgid=e1 :me!u@h PRIVMSG me-mbp :/agent cc-1531f8 run the tests",
            "@time=2026-10-30T12:06:00.000Z;msgid=e2 :me!u@h PRIVMSG me-mbp :/agent everyone stop",
        ],
    );
    let session = chat.conversation("me-mbp/cc-1531f8").unwrap();
    assert_eq!(said(session), ["<me> run the tests"]);
    assert_eq!(
        said(chat.conversation("me-mbp").unwrap()),
        ["<me> /agent everyone stop"]
    );
    // Reactions go to the machine.
    assert_eq!(
        texts(&chat.react("me-mbp/cc-1531f8", "e1", "👍")),
        ["@+draft/react=👍;+draft/reply=e1 TAGMSG me-mbp"]
    );
}

#[test]
fn moves_what_we_said_to_an_agent_once_it_answers() {
    let mut chat = Scene::in_api();
    // History, oldest first: what we said comes before the session has a conversation.
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
    assert_eq!(
        names(&chat),
        ["irc.zirc.test", "#api"],
        "made as its history arrives"
    );
    feed(
        &mut chat,
        &[
            ":s BATCH +h chathistory me-mbp",
            "@batch=h;time=2026-10-30T12:00:00.000Z;msgid=a :me!u@h PRIVMSG me-mbp :/agent cc-1531f8 status?",
            &format!(
                "@batch=h;{}",
                &dm("me-mbp", "cc-1531f8", "all green", 1)[1..]
            ),
            ":s BATCH -h",
        ],
    );
    assert_eq!(
        said(chat.conversation("me-mbp/cc-1531f8").unwrap()),
        ["<me> status?", "[post] cc-1531f8: all green"]
    );
    assert!(said(chat.conversation("me-mbp").unwrap()).is_empty());
    // Catching up again asks for the machine's history once, from its newest message.
    chat.disconnected("gone");
    let sent = texts(&chat.connected(link(true)));
    assert_eq!(
        sent,
        [
            "LIST",
            "CHATHISTORY TARGETS timestamp=2026-09-30T12:00:00.000Z timestamp=2026-10-30T12:00:00.000Z 50",
            "CHATHISTORY LATEST me-mbp timestamp=2026-10-30T12:00:59.000Z 200"
        ]
    );
}

#[test]
fn keeps_read_markers_to_the_machine_not_its_agents() {
    let mut chat = marking();
    let sent = chat.open("me-mbp/cc-1531f8");
    assert_eq!(texts(&sent), ["CHATHISTORY LATEST me-mbp * 200"]);
    let sent = feed(&mut chat, &[&dm("me-mbp", "cc-1531f8", "done", 1)]);
    assert!(
        !sent.iter().any(|line| line.starts_with("MARKREAD")),
        "{sent:?}"
    );
}

#[test]
fn lists_the_agents_on_our_machines() {
    let mut chat = Scene::in_api();
    let busy = working("cc-1531f8", &["src/lib.rs"]);
    let taking = Message::new(
        "cc-77aa01",
        Event::Text {
            body: "taking: the docs".into(),
        },
    );
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            &zirc("me-mbp", &busy),
            &zirc("me-mbp", &taking),
            &zirc("bob-mbp", &Message::new("cc-0000ff", taking.event.clone())),
            &dm("me-mini", "cc-3", "hi", 1),
        ],
    );
    assert_eq!(
        chat.agents(),
        ["me-mbp/cc-1531f8", "me-mbp/cc-77aa01", "me-mini/cc-3"]
    );
}

#[test]
fn makes_a_group_only_its_members_can_find_or_join() {
    let mut chat = Scene::in_api();
    let sent = texts(&chat.create_group(&["bob".into(), "carol".into(), "me".into()]));
    assert_eq!(sent.len(), 1);
    let name = sent[0].strip_prefix("JOIN ").unwrap().to_owned();
    assert!(is_group(&name), "{name}");
    assert_eq!(chat.title(&name), "bob, carol");
    let sent = feed(&mut chat, &[&format!(":me!u@h JOIN {name}")]);
    assert_eq!(
        sent[..6],
        [
            format!("MODE {name} +si"),
            format!("PRIVMSG ChanServ :REGISTER {name}"),
            format!("PRIVMSG ChanServ :AMODE {name} +v bob"),
            format!("INVITE bob {name}"),
            format!("PRIVMSG ChanServ :AMODE {name} +v carol"),
            format!("INVITE carol {name}"),
        ]
    );
    assert_eq!(chat.selected().name, name, "shown once joined");
    feed(&mut chat, &[&format!(":bob!u@h JOIN {name}")]);
    assert_eq!(
        chat.title(&name),
        "bob, carol",
        "carol's invited, if not here yet"
    );
    assert_eq!(
        texts(&chat.add_to_group(&name, &["dave".into()])),
        [
            format!("PRIVMSG ChanServ :AMODE {name} +v dave"),
            format!("INVITE dave {name}"),
        ]
    );
    assert_eq!(chat.title(&name), "bob, carol, dave");
}

#[test]
fn says_who_a_private_conversation_is_with_for_its_avatar() {
    let mut chat = Scene::in_api();
    let sent = texts(&chat.create_group(&["bob".into(), "carol".into(), "me".into()]));
    let group = sent[0].strip_prefix("JOIN ").unwrap().to_owned();
    feed(
        &mut chat,
        &[
            &format!(":me!u@h JOIN {group}"),
            &format!(":bob!u@h JOIN {group}"),
            &format!(":carol!u@h JOIN {group}"),
        ],
    );
    // A group's others, never us; a person; an agent session's machine; no one for a channel.
    let mut others = chat.others(&group);
    others.sort();
    assert_eq!(others, ["bob", "carol"]);
    assert_eq!(chat.others("bob"), ["bob"]);
    assert_eq!(chat.others("bob-laptop/cc-1"), ["bob-laptop"]);
    assert!(chat.others("#api").is_empty());
}

#[test]
fn new_message_offers_people_and_starts_a_direct_message_or_a_group() {
    use crate::chat::in_label;
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":bob!u@h PRIVMSG me :hi",
            ":me!u@h JOIN #web",
            ":s 353 me = #web :me carol bob",
        ],
    );
    // Those we have direct messages with first, then the channels' people, saying where.
    let people = chat.pickable_people();
    let nicks: Vec<&str> = people.iter().map(|p| p.nick.as_str()).collect();
    assert_eq!(nicks, ["bob", "carol"]);
    assert_eq!(people[1].note, Some(in_label("#web")));
    // Nobody until something's typed, and never someone chosen already.
    assert!(chat.pick_people("", &[]).is_empty());
    assert_eq!(chat.pick_people("car", &[])[0].nick, "carol");
    assert!(chat.pick_people("car", &["carol".into()]).is_empty());
    // One person opens their direct message; several make a group.
    let one = chat.start_message(&["bob".into()]);
    assert_eq!((one.open.as_deref(), one.lines.len()), (Some("bob"), 0));
    let group = chat.start_message(&["bob".into(), "carol".into()]);
    assert_eq!(group.open, None);
    assert!(texts(&group.lines)[0].starts_with("JOIN #dm-"));
}

#[test]
fn a_group_shows_two_faces_whoever_spoke_last_first() {
    let mut chat = Scene::in_api();
    let members = ["bob".into(), "carol".into(), "dave".into(), "me".into()];
    let sent = texts(&chat.create_group(&members));
    let group = sent[0].strip_prefix("JOIN ").unwrap().to_owned();
    feed(
        &mut chat,
        &[
            &format!(":me!u@h JOIN {group}"),
            &format!(":bob!u@h JOIN {group}"),
            &format!(":carol!u@h JOIN {group}"),
            &format!(":dave!u@h JOIN {group}"),
        ],
    );
    // Nobody has spoken: the first two by nick.
    assert_eq!(chat.faces_of(&group), ["bob", "carol"]);
    feed(
        &mut chat,
        &[
            &format!(":carol!u@h PRIVMSG {group} :hi"),
            &format!(":dave!u@h PRIVMSG {group} :hello"),
            &format!(":me!u@h PRIVMSG {group} :hey"),
        ],
    );
    // Dave spoke last of the others, then Carol; never us.
    assert_eq!(chat.faces_of(&group), ["dave", "carol"]);
    assert_eq!(crate::chat::GROUP_FACES, 2);
    assert_eq!(chat.faces_of("bob"), ["bob"]);
}

#[test]
fn joins_groups_by_invitation_never_from_the_list() {
    let mut chat = marking();
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    // Listed, as it is to its members: joined already, or left.
    let sent = feed(
        &mut chat,
        &[
            ":s 322 me #dm-abc 2 :",
            ":s 322 me #web 3 :",
            ":s 323 me :End",
        ],
    );
    assert_eq!(sent, ["JOIN #extra"]);
    // Invited live: joined, without taking the person away from what they're looking at.
    let sent = feed(&mut chat, &[":bob!u@h INVITE me #dm-abc"]);
    assert_eq!(sent, ["JOIN #dm-abc"]);
    let shown = chat.selected().name.clone();
    feed(&mut chat, &[":me!u@h JOIN #dm-abc"]);
    assert_eq!(chat.selected().name, shown);
    // Someone else invited, as the server may say: nothing to do.
    assert!(feed(&mut chat, &[":bob!u@h INVITE carol #dm-abc"]).is_empty());
    // An invitation to a channel that isn't a group is the person's to take up.
    assert!(feed(&mut chat, &[":bob!u@h INVITE me #secret"]).is_empty());
    let note = said(chat.selected()).pop().unwrap();
    assert!(note.contains("bob invites you to #secret"), "{note}");
}

#[test]
fn joins_the_rooms_its_own_machine_invites_it_to_and_no_one_elses() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #general",
            // Who's here: our machine, and someone else's machine named as if it were ours.
            ":s 354 me 42 machine/me me-mbp H me-mbp :me",
            ":s 354 me 42 machine/bob me-evil H bob-evil :bob",
        ],
    );
    // Our machine started watching a repository: its channel and agents' room are joined,
    // without a word and without taking the person away from what they're looking at.
    let shown = chat.selected().name.clone();
    let sent = feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me INVITE me #api",
            ":me-mbp!u@machine/me INVITE me #api/agents",
        ],
    );
    assert_eq!(sent, ["JOIN #api", "JOIN #api/agents"]);
    assert_eq!(chat.selected().name, shown);
    assert!(
        said(chat.selected())
            .iter()
            .all(|line| !line.contains("invites"))
    );
    // Joined already: nothing more.
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    assert!(feed(&mut chat, &[":me-mbp!u@machine/me INVITE me #api"]).is_empty());
    // Anyone else's, a machine named as ours whose host says it's bob's among them, is the person's
    // to take up.
    for from in ["bob-mbp", "me-evil"] {
        let line = format!(":{from}!u@machine/bob INVITE me #secret");
        assert!(feed(&mut chat, &[line.as_str()]).is_empty(), "{from}");
    }
    let note = said(chat.selected()).pop().unwrap();
    assert!(note.contains("invites you to #secret"), "{note}");
}

#[test]
fn joins_a_group_from_history_only_when_its_invitation_is_unread() {
    let mut chat = marking();
    // How far the conversation with bob was read comes first.
    let sent = feed(
        &mut chat,
        &[
            ":s BATCH +t draft/chathistory-targets",
            ":s CHATHISTORY TARGETS bob 2026-10-30T11:00:00.000Z",
            ":s BATCH -t",
        ],
    );
    assert_eq!(sent, ["MARKREAD bob", "CHATHISTORY LATEST bob * 200"]);
    let invites = |chat: &mut Chat| {
        feed(
            chat,
            &[
                ":s MARKREAD bob timestamp=2026-10-30T11:30:00.000Z",
                ":s BATCH +h chathistory bob",
                "@batch=h;time=2026-10-30T11:00:00.000Z;msgid=i1 :bob!u@h INVITE me #dm-old",
                "@batch=h;time=2026-10-30T11:45:00.000Z;msgid=i2 :bob!u@h INVITE me #dm-new",
                ":s BATCH -h",
            ],
        )
    };
    let sent = invites(&mut chat);
    assert!(sent.contains(&"JOIN #dm-new".to_owned()), "{sent:?}");
    assert!(
        !sent.iter().any(|line| line.contains("#dm-old")),
        "{sent:?}"
    );
    // Left: not joined again by the invitation it came from.
    feed(&mut chat, &[":me!u@h JOIN #dm-new"]);
    let _ = chat.part("#dm-new", None);
    feed(&mut chat, &[":me!u@h PART #dm-new"]);
    let sent = invites(&mut chat);
    assert!(
        !sent.iter().any(|line| line.starts_with("JOIN")),
        "{sent:?}"
    );
}

#[test]
fn leaves_machines_out_of_groups_and_says_who_isnt_around() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":s 353 me = #api :me bob bob-mbp alice-laptop",
            ":s 366 me #api :End",
            // Their hosts say whose machines they are.
            ":s 354 me 42 machine/bob bob-mbp H bob-mbp :bob",
            ":s 354 me 42 machine/alice alice-laptop H alice-laptop :alice",
        ],
    );
    let notes = |chat: &Chat| -> Vec<String> {
        said(chat.selected())
            .into_iter()
            .filter(|line| !line.starts_with('<'))
            .collect()
    };
    let sent = texts(&chat.create_group(&[
        "bob".into(),
        "bob-mbp".into(),
        "alice-laptop".into(),
        "zed".into(),
    ]));
    let name = sent[0].strip_prefix("JOIN ").unwrap().to_owned();
    let said = notes(&chat);
    assert!(
        said.iter().any(|l| l.contains("bob-mbp is a machine")),
        "{said:?}"
    );
    assert!(
        said.iter().any(|l| l.contains("alice-laptop is a machine")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|l| l.contains("zed isn't anywhere you are")),
        "{said:?}"
    );
    assert!(!said.iter().any(|l| l.contains("bob isn't")), "{said:?}");
    assert_eq!(chat.title(&name), "bob, zed");
    let sent = feed(&mut chat, &[&format!(":me!u@h JOIN {name}")]);
    assert!(
        !sent
            .iter()
            .any(|line| line.contains("bob-mbp") || line.contains("alice-laptop")),
        "{sent:?}"
    );

    // Only machines: no group.
    assert!(chat.create_group(&["bob-mbp".into()]).is_empty());
    assert!(
        notes(&chat)
            .last()
            .unwrap()
            .contains("a group needs someone else")
    );
    assert!(
        chat.add_to_group(&name, &["alice-laptop".into()])
            .is_empty()
    );
}

#[test]
fn lists_conversations_starred_first_then_by_what_was_said_last() {
    let mut chat = marking();
    // The server says when each conversation last heard something, before any history.
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            ":me!u@h JOIN #ops",
            ":s BATCH +t draft/chathistory-targets",
            ":s CHATHISTORY TARGETS #web 2026-10-30T11:50:00.000Z",
            ":s CHATHISTORY TARGETS #api 2026-10-30T09:00:00.000Z",
            ":s CHATHISTORY TARGETS bob 2026-10-30T11:00:00.000Z",
            ":s CHATHISTORY TARGETS carol 2026-10-30T11:30:00.000Z",
            ":s BATCH -t",
        ],
    );
    let listed = |chat: &Chat, section| -> Vec<String> {
        chat.list(section)
            .into_iter()
            .map(|at| chat.conversations()[at].name.clone())
            .collect()
    };
    // #ops has said nothing, so it's last.
    assert_eq!(
        listed(&chat, SidebarSection::Channels),
        ["#web", "#api", "#ops"]
    );
    assert_eq!(listed(&chat, SidebarSection::Direct), ["carol", "bob"]);
    assert_eq!(
        chat.last_said("#web"),
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 11, 50, 0).unwrap())
    );

    // Something said moves it up.
    feed(
        &mut chat,
        &["@time=2026-10-30T11:55:00.000Z;msgid=m1 :alice!u@h PRIVMSG #api :deploying"],
    );
    assert_eq!(
        listed(&chat, SidebarSection::Channels),
        ["#api", "#web", "#ops"]
    );

    // Starred ones come first, in the order starred, whatever was said.
    chat.star("#ops", true);
    chat.star("bob", true);
    assert_eq!(
        listed(&chat, SidebarSection::Channels),
        ["#ops", "#api", "#web"]
    );
    assert_eq!(listed(&chat, SidebarSection::Direct), ["bob", "carol"]);
    assert!(chat.is_starred("#OPS"));
    assert_eq!(chat.stars(), ["#ops", "bob"]);
    chat.star("#ops", false);
    assert_eq!(
        listed(&chat, SidebarSection::Channels),
        ["#api", "#web", "#ops"]
    );
    // The channel list every app shows goes by name, not by what was said: what's starred
    // first, in the order starred, then A–Z.
    let groups = |chat: &Chat| -> Vec<String> {
        chat.listed_channel_groups()
            .into_iter()
            .map(|group| group.name)
            .collect()
    };
    assert_eq!(groups(&chat), ["#api", "#ops", "#web"]);
    chat.star("#web", true);
    chat.star("#ops", true);
    assert_eq!(groups(&chat), ["#web", "#ops", "#api"]);
    chat.star("#web", false);
    chat.star("#ops", false);
    // As an app kept them.
    chat.set_stars(["#web".to_owned(), "#web".to_owned()]);
    assert_eq!(chat.stars(), ["#web"]);
    assert_eq!(SidebarSection::of("#dm-abc"), SidebarSection::Direct);
    assert_eq!(SidebarSection::of("me-mbp/cc-1"), SidebarSection::Direct);
}

#[test]
fn active_now_is_the_agents_taking_something_on_ours_first_then_newest() {
    let mut chat = Scene::in_api();
    let taking = |session: &str, task: &str| {
        Message::new(
            session,
            Event::Text {
                body: format!("taking: {task}"),
            },
        )
    };
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            ":me-mini!u@machine/me JOIN #api",
            ":bob-laptop!u@machine/bob JOIN #api",
            ":bob!u@h JOIN #api bob :Bob Lee",
            &format!(
                "@time=2026-10-30T10:00:00.000Z {}",
                zirc("me-mini", &taking("cc-2", "the docs"))
            ),
            &format!(
                "@time=2026-10-30T11:00:00.000Z {}",
                zirc("me-mbp", &taking("cc-1", "the login page"))
            ),
            &format!(
                "@time=2026-10-30T11:30:00.000Z {}",
                zirc("bob-laptop", &taking("cc-9", "billing"))
            ),
        ],
    );
    let cards: Vec<_> = chat
        .active_now()
        .into_iter()
        .map(|card| (card.agent.name, card.owner, card.mine, card.task))
        .collect();
    assert_eq!(
        cards,
        [
            (
                "me-mbp/cc-1".into(),
                "you".into(),
                true,
                "the login page".into()
            ),
            ("me-mini/cc-2".into(), "you".into(), true, "the docs".into()),
            (
                "bob-laptop/cc-9".into(),
                "Bob Lee".into(),
                false,
                "billing".into()
            ),
        ]
    );
    assert_eq!(
        chat.active_now()[0].since,
        Utc.with_ymd_and_hms(2026, 10, 30, 11, 0, 0).unwrap()
    );
}

#[test]
fn a_channels_header_says_its_people_threads_and_topic() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web",
            ":s 353 me = #web :me carol dana",
            ":s 332 me #web :The web app",
            ":me!u@h JOIN #web/checkout",
            ":me!u@h JOIN #web/coupon",
            ":me!u@h JOIN #quiet",
        ],
    );
    assert_eq!(
        chat.channel_about("#web").as_deref(),
        Some("3 members · 2 threads · The web app")
    );
    assert_eq!(chat.channel_about("#quiet").as_deref(), Some("1 member"));
    assert_eq!(chat.channel_about("bob"), None);
    assert_eq!(threads_label(1), "1 thread");
}

#[test]
fn a_header_says_one_line_the_first_there_is() {
    use crate::chat::{HeaderLine, HeaderLineKind};
    let mut scene = Scene::api_with_away();
    scene.hears(&[
        ":me!u@h JOIN #web",
        ":s 353 me = #web :me carol",
        ":s 332 me #web :The web app",
        ":bob!u@h PRIVMSG me :hi",
        ":bob!u@h AWAY :lunch",
        ":dana!u@h PRIVMSG me :hello",
    ]);
    let line = |text: &str, kind| {
        Some(HeaderLine {
            text: text.to_owned(),
            kind,
        })
    };
    // A channel's people, threads and topic.
    assert_eq!(
        scene.header_line("#web"),
        line("2 members · The web app", HeaderLineKind::About)
    );
    // A person away, in a direct message; none for one who's here.
    assert_eq!(
        scene.header_line("bob"),
        line("Away: lunch", HeaderLineKind::Away)
    );
    assert_eq!(scene.header_line("dana"), None);
}

#[test]
fn counts_people_never_an_app_whether_or_not_it_has_posted() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s 353 me = #api :me @carol carl dana zirc-alerts",
            // An app by its account alone, before it posts.
            ":me!u@h JOIN #web",
            ":alerts!u@h JOIN #web zirc-ci :CI",
        ],
    );
    assert_eq!(chat.org_header().members, 4, "{:?}", chat.org_header());
    assert_eq!(chat.member_count("#api"), 4);
    assert_eq!(chat.member_count("#web"), 1);
    assert!(chat.is_app("zirc-alerts") && chat.is_app("alerts"));
    assert!(!chat.is_app("carol"));
    assert!(
        chat.people_rows("#web")
            .iter()
            .all(|row| row.person.nick != "alerts")
    );
}

#[test]
fn heads_the_channel_list_with_the_organisation_and_its_people() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":s 005 me NETWORK=acme CHANTYPES=# :are supported by this server",
            ":me!u@h JOIN #api",
            ":s 353 me = #api :me bob me-mbp zirc-ci",
            ":s 354 me 42 machine/me me-mbp H me-mbp :me",
            ":me!u@h JOIN #web",
            ":s 353 me = #web :me BOB carol",
        ],
    );
    // People only, once each: not a machine, nor an app.
    assert_eq!(
        chat.org_header(),
        OrgHeader {
            name: "acme".into(),
            initials: "A".into(),
            members: 3,
            members_label: "3 members".into(),
            stale: false,
            mark_all_read: None,
        }
    );
    // Mark all read, while anything's unread, and gone once it's all read.
    feed(&mut chat, &[":bob!u@h PRIVMSG #web :hello"]);
    assert_eq!(
        chat.org_header().mark_all_read.as_deref(),
        Some(crate::chat::conversation_menu::MARK_ALL_READ)
    );
    let _ = chat.mark_all_read();
    assert_eq!(chat.org_header().mark_all_read, None);
    chat.set_org_name(Some("Acme Inc"));
    assert_eq!(chat.org_header().name, "Acme Inc");
    assert_eq!(chat.org_header().initials, "AI");
    // Offline, the people stay as last known, flagged stale, until NAMES says them afresh.
    chat.disconnected("lost");
    let header = chat.org_header();
    assert_eq!((header.members, header.stale), (3, true));
    assert_eq!(chat.member_count("#api"), 2);
    connected(&mut chat);
    feed(&mut chat, &[":s 353 me = #api :me", ":s 366 me #api :End"]);
    assert_eq!(chat.member_count("#api"), 1);
    assert!(!chat.org_header().stale);
}

#[test]
fn names_an_agent_by_its_session_and_what_it_took_on() {
    let mut chat = Scene::in_api();
    let taking = Message::new(
        "cc-1531f8",
        Event::Text {
            body: "taking: platform: CI, alerts".into(),
        },
    );
    feed(
        &mut chat,
        &[
            ":me-mbp!u@machine/me JOIN #api",
            &format!("@time=2026-10-30T11:00:00.000Z {}", zirc("me-mbp", &taking)),
            &dm("me-mbp", "cc-1531f8", "on it", 1),
        ],
    );
    let agent = chat.agent("me-mbp/cc-1531f8").unwrap();
    assert_eq!(agent.machine, "me-mbp");
    assert_eq!(agent.machine_name, "me-mbp");
    assert_eq!(agent.short, "cc-1531f8");
    assert_eq!(agent.task.as_deref(), Some("platform: CI, alerts"));
    assert_eq!(
        agent.latest,
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 12, 1, 0).unwrap())
    );
    // Done: no task.
    let done = Message::new(
        "cc-1531f8",
        Event::Text {
            body: "done: platform: CI, alerts".into(),
        },
    );
    feed(&mut chat, &[&zirc("me-mbp", &done)]);
    assert_eq!(chat.agent("me-mbp/cc-1531f8").unwrap().task, None);
    assert!(chat.agent("bob").is_none());
}

#[test]
fn says_who_is_typing_until_they_stop_or_what_they_typed_arrives() {
    let mut chat = Scene::in_api();
    let at = |second: u32| format!("@time=2026-10-30T11:59:{second:02}.000Z;+typing=active");
    feed(
        &mut chat,
        &[
            &format!("{} :bob!u@h TAGMSG me", at(58)),
            &format!("{} :alice!u@h TAGMSG #api", at(57)),
            &format!("{} :carol!u@h TAGMSG #api", at(57)),
        ],
    );
    assert_eq!(chat.typing_text("bob").as_deref(), Some("bob is typing…"));
    assert_eq!(
        chat.typing_text("#api").as_deref(),
        Some("alice and carol are typing…")
    );
    // What they typed arrives, or they say they're done: they've stopped.
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T12:00:00.000Z;msgid=b1 :bob!u@h PRIVMSG me :here it is",
            "@+typing=done :carol!u@h TAGMSG #api",
        ],
    );
    assert_eq!(chat.typing_text("bob"), None);
    assert_eq!(
        chat.typing_text("#api").as_deref(),
        Some("alice is typing…")
    );
    // Six seconds on, with no word since, alice isn't typing either.
    chat.clock = || noon() + chrono::Duration::seconds(4);
    assert!(chat.typing_text("#api").is_none());

    // One of our machines' agents is thinking, in its own conversation.
    chat.clock = noon;
    feed(
        &mut chat,
        &["@+typing=active;+zirc.dev/session=cc-1531f8 :me-mbp!u@machine/me TAGMSG me"],
    );
    assert_eq!(
        chat.typing_text("me-mbp/cc-1531f8").as_deref(),
        Some("cc-1531f8 is thinking…")
    );
    assert!(chat.typing_text("me-mbp").is_none());

    // A machine at work for itself, as a host answering what we said to it, thinks too.
    feed(&mut chat, &["@+typing=active :ci-1!u@machine/me TAGMSG me"]);
    assert_eq!(
        chat.typing_text("ci-1").as_deref(),
        Some("ci-1 is thinking…")
    );
}

#[test]
fn says_the_person_is_typing_every_few_seconds_and_when_they_clear_it() {
    let mut chat = Scene::in_api();
    let _ = chat.open("bob");
    assert_eq!(
        texts(&chat.typed("bob", false)),
        ["@+typing=active TAGMSG bob"]
    );
    // Not again within three seconds.
    assert!(chat.typed("bob", false).is_empty());
    chat.clock = || noon() + chrono::Duration::seconds(3);
    assert_eq!(
        texts(&chat.typed("bob", false)),
        ["@+typing=active TAGMSG bob"]
    );
    // Cleared: done, once.
    assert_eq!(
        texts(&chat.typed("bob", true)),
        ["@+typing=done TAGMSG bob"]
    );
    assert!(chat.typed("bob", true).is_empty());
    // Sent: the next keystroke says it again at once.
    let _ = chat.typed("bob", false);
    let _ = chat.say("bob", "hi", false);
    assert_eq!(
        texts(&chat.typed("bob", false)),
        ["@+typing=active TAGMSG bob"]
    );
    // Not at all, when the person turned it off; nor where there's no conversation.
    chat.set_send_typing(false);
    chat.clock = || noon() + chrono::Duration::seconds(30);
    assert!(chat.typed("bob", false).is_empty());
    chat.set_send_typing(true);
    assert!(chat.typed("nobody", false).is_empty());
}

#[test]
fn leaves_a_thread_off_the_sidebar_after_a_day_with_nothing_unread_or_shown() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web",
            ":me!u@h JOIN #web/fresh",
            ":me!u@h JOIN #web/old",
            ":me!u@h JOIN #web/unread",
            ":me!u@h JOIN #web/silent",
            ":me!u@h JOIN #web/shown",
            "@time=2026-10-30T11:00:00.000Z :me!u@h PRIVMSG #web/fresh :today",
            "@time=2026-10-27T11:00:00.000Z :me!u@h PRIVMSG #web/old :days ago",
            "@time=2026-10-27T11:00:00.000Z :bob!u@h PRIVMSG #web/unread :days ago",
            "@time=2026-10-27T11:00:00.000Z :me!u@h PRIVMSG #web/shown :days ago",
        ],
    );
    let at = chat.position("#web/shown").unwrap();
    let _ = chat.select(at);
    let listed: Vec<String> = chat
        .sidebar_list(&ListOptions::default())
        .into_iter()
        .filter(|row| row.kind == ListKind::Room)
        .map(|row| row.name)
        .collect();
    // An active thread is listed under its channel; a quiet one only in its channel's
    // threads, as Discord lists them (the user's choice, 14 Nov 2026).
    assert_eq!(listed, ["#web/fresh", "#web/shown", "#web/unread"]);
    let threads: Vec<String> = chat
        .threads("#web")
        .into_iter()
        .map(|thread| thread.name)
        .collect();
    for quiet in ["#web/old", "#web/silent"] {
        assert!(threads.iter().any(|name| name == quiet), "{threads:?}");
    }
}

#[test]
fn lists_every_thread_plainly_quiet_ones_too() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web",
            ":me!u@h JOIN #web/fresh",
            ":me!u@h JOIN #web/old",
            ":me!u@h JOIN #web/shown",
            ":me!u@h JOIN #web/unread",
            ":me!u@h JOIN #web/silent",
            ":me!u@h JOIN #web/pinned",
            "@time=2026-10-30T11:00:00.000Z :me!u@h PRIVMSG #web/fresh :today",
            "@time=2026-10-27T11:00:00.000Z :me!u@h PRIVMSG #web/old :days ago",
            "@time=2026-10-27T11:00:00.000Z :me!u@h PRIVMSG #web/shown :days ago",
            "@time=2026-10-27T11:00:00.000Z :bob!u@h PRIVMSG #web/unread :days ago",
        ],
    );
    chat.star("#web/pinned", true);
    let at = chat.position("#web/shown").unwrap();
    let _ = chat.select(at);
    let group = chat
        .channel_groups()
        .into_iter()
        .find(|g| g.name == "#web")
        .unwrap();
    // Every thread is listed, quiet or not: none folds away.
    assert_eq!(group.rooms.len(), 6);
}

#[test]
fn nests_rooms_under_their_repository_with_what_is_unread() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web",
            ":me!u@h JOIN #api/login",
            ":me!u@h JOIN #api/auth",
            ":me!u@h JOIN #ops",
            "@time=2026-10-30T11:00:00.000Z;msgid=1 :bob!u@h PRIVMSG #api :morning",
            "@time=2026-10-30T11:10:00.000Z;msgid=2 :bob!u@h PRIVMSG #api/login :me: a look?",
            "@time=2026-10-30T11:05:00.000Z;msgid=3 :bob!u@h PRIVMSG #web :deploying",
        ],
    );
    let at = chat.position("#ops").unwrap();
    let _ = chat.select(at);
    let groups = chat.channel_groups();
    let rows: Vec<(&str, Vec<&str>, usize, usize)> = groups
        .iter()
        .map(|g| {
            let rooms = g.rooms.iter().map(String::as_str).collect();
            (g.name.as_str(), rooms, g.unread, g.mentions)
        })
        .collect();
    // #api's own message was read as it was shown; its room's wasn't.
    assert_eq!(
        rows,
        [
            ("#api", vec!["#api/login", "#api/auth"], 1, 1),
            ("#web", vec![], 1, 0),
            ("#ops", vec![], 0, 0),
        ]
    );
    assert_eq!(
        groups[0].latest,
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 11, 10, 0).unwrap())
    );
}

#[test]
fn lists_each_direct_conversation_on_its_own_newest_first() {
    let mut chat = Scene::in_api();
    let old =
        dm("me-mini", "cc-2", "done long ago", 0).replace("2026-10-30T12:00", "2026-10-28T09:00");
    feed(
        &mut chat,
        &[
            &old,
            "@time=2026-10-30T11:00:00.000Z;msgid=b1 :bob!u@h PRIVMSG me :hi",
            &dm("me-mbp", "cc-1", "done", 5),
            "@time=2026-10-30T11:30:00.000Z;msgid=l1 :bob-laptop!u@machine/bob PRIVMSG me :a machine of bob's",
            ":me!u@h JOIN #dm-abc",
            ":s 353 me @ #dm-abc :me carol",
        ],
    );
    // Read, the old session is quiet: unread, it wouldn't fold.
    let _ = chat.mark_read("me-mini/cc-2");
    let rows: Vec<(String, DirectKind, bool)> = chat
        .direct_rows()
        .into_iter()
        .map(|row| (row.name, row.kind, row.folded))
        .collect();
    // Newest first, each on its own: our agent, bob's machine itself, whose plain word has
    // no session to file it under, bob, the old agent session (folded, as it's been quiet a
    // day), and the group nothing's been said in.
    assert_eq!(
        rows,
        [
            ("me-mbp/cc-1".to_owned(), DirectKind::Agent, false),
            ("bob-laptop".to_owned(), DirectKind::Machine, false),
            ("bob".to_owned(), DirectKind::Person, false),
            ("me-mini/cc-2".to_owned(), DirectKind::Agent, true),
            ("#dm-abc".to_owned(), DirectKind::Group, false),
        ]
    );
    let agent = &chat.direct_rows()[0];
    assert_eq!(agent.title, "cc-1");
    assert_eq!(agent.owner.as_deref(), Some("you"));
    assert_eq!(agent.label(), "cc-1");
    assert!(agent.mine && agent.presence.is_none());
    assert!(matches!(agent.avatars[..], [Avatar::Agent { .. }]));
    let bob = chat
        .direct_rows()
        .into_iter()
        .find(|row| row.name == "bob")
        .unwrap();
    assert_eq!((bob.unread, bob.badge), (1, 1));
    assert_eq!(bob.presence, Some(Presence::Gone));
    // A star still pins it first.
    chat.star("bob", true);
    assert_eq!(chat.direct_rows()[0].name, "bob");
}

#[test]
fn the_core_says_which_rows_are_channels_and_which_a_person() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            &dm("me-mbp", "cc-1", "done", 1),
            ":bob!u@h PRIVMSG me :hi",
            ":me!u@h JOIN &local",
            ":me!u@h JOIN #dm-abc",
            ":s 353 me @ #dm-abc :me carol",
        ],
    );
    let rows = chat.sidebar_list(&super::ListOptions::default());
    let row = |name: &str| {
        rows.iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no row {name}: {rows:#?}"))
    };
    // "#" and "&" are both channels, the sigil part of the name every app draws whole (the
    // user's choice, 16 Nov 2026).
    assert!(row("#api").channel && row("#api").title == "#api");
    assert!(row("&local").channel && row("&local").title == "&local");
    // A group's name is a channel's, but it's a direct message.
    let group = row("#dm-abc");
    assert!(!group.channel && group.person.is_none());
    // Only a direct message with one person has a person.
    assert!(matches!(row("bob").person, Some(Face::Person { .. })));
    assert!(!row("bob").channel);
    assert!(row("me-mbp/cc-1").person.is_none());
    assert_eq!(chat.direct_kind("#api"), None);
    assert_eq!(chat.direct_kind("#dm-abc"), Some(DirectKind::Group));
}

#[test]
fn a_persons_direct_row_has_their_dot_and_a_groups_none() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            &dm("me-mbp", "cc-1", "done", 1),
            ":bob!u@h PRIVMSG me :hi",
            ":me!u@h JOIN #dm-abc",
            ":s 353 me @ #dm-abc :me carol",
        ],
    );
    let dot = |chat: &Chat, name: &str| {
        chat.direct_rows()
            .into_iter()
            .find(|row| row.name == name)
            .unwrap()
            .presence
    };
    assert_eq!(
        dot(&chat, "bob"),
        Some(Presence::Gone),
        "in no channel of ours"
    );
    assert_eq!(dot(&chat, "me-mbp/cc-1"), None, "an agent session");
    assert_eq!(dot(&chat, "#dm-abc"), None, "a group");
    feed(
        &mut chat,
        &[":me!u@h JOIN #web", ":s 353 me @ #web :me bob"],
    );
    assert_eq!(dot(&chat, "bob"), Some(Presence::Here));
    feed(&mut chat, &[":bob!u@h AWAY :lunch"]);
    assert_eq!(dot(&chat, "bob"), Some(Presence::Away));
}

#[test]
fn without_listing_joins_only_what_it_was_given() {
    let mut chat = chat_with(Options {
        list_channels: false,
        ..Options::default()
    });
    let sent = texts(&chat.connected(link(true)));
    assert!(
        !sent.iter().any(|line| line.starts_with("LIST")),
        "{sent:?}"
    );
    assert!(sent.contains(&"JOIN #extra".to_owned()), "{sent:?}");
    feed(&mut chat, &[":me!u@h JOIN #extra"]);
    assert_eq!(chat.selected().name, "#extra");
    assert!(chat.tick().is_empty());
    // Again, after a reconnect.
    chat.disconnected("gone");
    let again = texts(&chat.connected(link(true)));
    assert!(again.contains(&"JOIN #extra".to_owned()), "{again:?}");
}

#[test]
fn a_group_never_opens_first() {
    let mut chat = chat();
    chat.connected(link(true));
    // Groups sort before the organisation's channels, and the server may join them first.
    feed(
        &mut chat,
        &[":me!u@h JOIN #dm-qowqnj7ryj", ":me!u@h JOIN #api"],
    );
    assert_eq!(chat.selected().name, "#api");
}

#[test]
fn marks_one_conversation_or_all_read_as_escape_does() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            "@time=2026-10-30T11:30:00.000Z;msgid=1 :alice!u@h PRIVMSG #api :me: look",
            "@time=2026-10-30T11:40:00.000Z;msgid=2 :bob!u@h PRIVMSG #web :new design",
        ],
    );
    assert_eq!((api(&chat).unread, api(&chat).mentions), (1, 1));
    // One, up to its newest message, here and for the person's other apps.
    let marked = texts(&chat.mark_read("#api"));
    assert_eq!(
        to_server(&marked),
        ["MARKREAD #api timestamp=2026-10-30T11:30:00.000Z"]
    );
    // And the tenant's pusher, so the person's phone isn't pushed what's read.
    let told = marked.last().unwrap();
    let tag = told
        .strip_prefix("@+zirc/push=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    assert_eq!(
        zirc_proto::push::Said::of(tag),
        Some(zirc_proto::push::Said::Read {
            target: "#api".into(),
            at: "2026-10-30T11:30:00.000Z".into(),
        })
    );
    assert_eq!((api(&chat).unread, api(&chat).mentions), (0, 0));
    assert!(chat.mark_read("#api").is_empty(), "nothing new to mark");
    assert!(chat.mark_read("#nowhere").is_empty());
    // Every one.
    assert_eq!(
        to_server(&texts(&chat.mark_all_read())),
        ["MARKREAD #web timestamp=2026-10-30T11:40:00.000Z"]
    );
    let web = chat.conversation("#web").unwrap();
    assert_eq!(web.unread, 0);
}

#[test]
fn says_a_short_message_on_several_lines_as_one_or_a_line_each() {
    // Where the server takes multiline messages: one, its lines as they were written.
    let mut multi = chat();
    multi.connected(Link {
        multiline: Some(Limits {
            max_bytes: 4096,
            max_lines: 100,
        }),
        ..link(false)
    });
    feed(&mut multi, &[":me!u@h JOIN #api"]);
    let sent = texts(&multi.say("#api", "retries are in:\n- three tries", false));
    assert!(sent[0].contains("BATCH +"), "{sent:?}");
    assert!(
        sent.iter().any(|line| line.ends_with(":retries are in:")),
        "{sent:?}"
    );
    assert!(
        sent.iter().any(|line| line.ends_with(":- three tries")),
        "{sent:?}"
    );
    assert!(sent.iter().all(|line| !line.contains('\n')));
    // Elsewhere, a message a line, blank lines left out.
    let mut plain = chat();
    plain.connected(link(false));
    feed(&mut plain, &[":me!u@h JOIN #api"]);
    assert_eq!(
        texts(&plain.say("#api", "first\n\nsecond", false)),
        ["PRIVMSG #api first", "PRIVMSG #api second"]
    );
}

#[test]
fn searches_what_is_loaded_then_asks_the_server_for_older() {
    use zirc_proto::login::{HistoryMessage, HistoryPage, HistorySender};
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            "@time=2026-10-30T11:00:00.000Z;msgid=m1 :bob!u@h PRIVMSG #api :the Deploy failed",
            "@time=2026-10-30T11:05:00.000Z;msgid=m2 :ada!u@h PRIVMSG #api :lunch?",
            "@time=2026-10-30T11:10:00.000Z;msgid=m3 :ada!u@h PRIVMSG #api :deploying again",
        ],
    );
    let mut search = chat.search("#api", "deploy");
    let found: Vec<(Option<&str>, bool)> = search
        .hits
        .iter()
        .map(|hit| (hit.id.as_deref(), hit.loaded))
        .collect();
    assert_eq!(found, [(Some("m3"), true), (Some("m1"), true)]);

    // Older, from the server: before what's loaded, without repeating it.
    let query = search.server_query().unwrap();
    assert_eq!(query.conversation, "#api");
    assert_eq!(query.text, "deploy");
    assert_eq!(
        query.until,
        Some(Utc.with_ymd_and_hms(2026, 10, 30, 11, 0, 0).unwrap())
    );
    let said = |id: &str, time: &str, session: Option<&str>| HistoryMessage {
        msgid: id.into(),
        time: time.into(),
        conversation: "#api".into(),
        sender: HistorySender {
            account: Some("bob".into()),
            nick: "bob".into(),
            kind: "person".into(),
        },
        text: "deploy notes".into(),
        zirc: session.map(|s| serde_json::json!({ "session": s })),
    };
    search.take_page(&HistoryPage {
        policy: "channels".into(),
        messages: vec![
            said("m1", "2026-10-30T11:00:00.000Z", None),
            said("m0", "2026-10-29T09:00:00.000Z", None),
        ],
        next: Some("c1".into()),
        indexed: false,
        counts: None,
    });
    assert_eq!(search.hits.len(), 3);
    assert_eq!(search.hits[2].id.as_deref(), Some("m0"));
    assert!(!search.hits[2].loaded);
    assert_eq!(search.server_query().unwrap().cursor.as_deref(), Some("c1"));
    search.take_page(&HistoryPage {
        policy: "channels".into(),
        messages: Vec::new(),
        next: None,
        indexed: false,
        counts: None,
    });
    assert!(search.done && search.server_query().is_none());
    assert_eq!(
        search.server_query().map(|q| q.params(50)),
        None,
        "nothing more to ask"
    );
}

#[test]
fn a_search_finds_a_loaded_zirc_message_by_its_data() {
    let mut chat = Scene::in_api();
    let mut post = zirc_proto::Message::new(
        "cc-1",
        zirc_proto::Event::Text {
            body: "taking: the tests".into(),
        },
    );
    post.task = Some("t-abc123".into());
    let body = zirc_proto::encode(&post).unwrap();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            &format!(
                "@time=2026-10-30T11:00:00.000Z;msgid=m1 :ada-mbp!u@machine/ada PRIVMSG #api :{body}"
            ),
        ],
    );
    let ids = |query: &str| -> Vec<Option<String>> {
        chat.search("#api", query)
            .hits
            .into_iter()
            .map(|hit| hit.id)
            .collect()
    };
    assert_eq!(ids("t-abc123"), [Some("m1".to_owned())]);
    assert_eq!(ids("the tests"), [Some("m1".to_owned())]);
    assert!(ids("task").is_empty(), "a field's name finds nothing");
}

#[test]
fn a_message_s_details_list_what_s_attached_and_its_data_as_json() {
    let mut chat = Scene::in_api();
    let mut post = zirc_proto::Message::new(
        "cc-1",
        zirc_proto::Event::Text {
            body: "taking: the tests".into(),
        },
    );
    post.task = Some("t-abc123".into());
    post.name = Some("login".into());
    let body = zirc_proto::encode(&post).unwrap();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            &format!(
                "@time=2026-10-30T11:00:00.000Z;msgid=m1 :ada-mbp!u@machine/ada PRIVMSG #api :{body}"
            ),
        ],
    );
    let details = chat.details("#api", "m1").unwrap();
    assert_eq!(details.title, crate::chat::MESSAGE_DETAILS);
    let rows: Vec<(&str, &str)> = details
        .rows
        .iter()
        .map(|row| (row.label.as_str(), row.value.as_str()))
        .filter(|(label, _)| *label != "Level")
        .collect();
    assert_eq!(
        rows,
        [
            ("Id", "m1"),
            ("Time", "2026-10-30T11:00:00.000Z"),
            ("Kind", "text"),
            ("From", "ada-mbp"),
            ("Machine of", "ada"),
            ("Session", "cc-1"),
            ("Session name", "login"),
            ("Task", "t-abc123"),
            ("Format", "1"),
        ]
    );
    let data: serde_json::Value = serde_json::from_str(details.data.as_deref().unwrap()).unwrap();
    assert_eq!(
        (&data["task"], &data["event"]["body"]),
        (&"t-abc123".into(), &"taking: the tests".into())
    );
}

#[test]
fn a_plain_message_s_details_have_no_data() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            "@time=2026-10-30T11:01:00.000Z;msgid=m2 :bob!u@h PRIVMSG #api :hello",
        ],
    );
    let plain = chat.details("#api", "m2").unwrap();
    assert_eq!(plain.data, None);
    assert!(
        plain
            .rows
            .iter()
            .any(|row| row.label == "Kind" && row.value == "message")
    );
    assert_eq!(chat.details("#api", "gone"), None);
}

#[test]
fn a_search_reads_from_and_quotes_as_every_app_and_the_cli_do() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            "@time=2026-10-30T11:00:00.000Z;msgid=m1 :bob!u@h PRIVMSG #api :the Deploy failed",
            "@time=2026-10-30T11:10:00.000Z;msgid=m3 :ada!u@h PRIVMSG #api :deploying again",
        ],
    );
    // Only Ada's, found loaded, and asked of the server by sender with the words alone.
    let search = chat.search("#api", "from:Ada deploy");
    let ids: Vec<_> = search.hits.iter().map(|hit| hit.id.as_deref()).collect();
    assert_eq!(ids, [Some("m3")]);
    let query = search.server_query().unwrap();
    assert_eq!(
        (query.text.as_str(), query.sender.as_deref()),
        ("deploy", Some("ada"))
    );
    assert!(query.params(50).contains(&("sender", "ada".to_owned())));
    // Everything Bob said here, with no words.
    let bob = chat.search("#api", "from:bob");
    assert_eq!(bob.hits.len(), 1);
    // A quoted phrase is one, found as typed; text that isn't a search is looked for as it is.
    assert_eq!(chat.search("#api", "\"deploy failed\"").hits.len(), 1);
    let view = chat.search_view("from:ada deploy", Some("#api"), None, |t| t.naive_utc());
    assert_eq!(view.hits.len(), 1);
    assert!(chat.search("#api", "after:yesterday").hits.is_empty());
}

#[test]
fn a_search_view_is_everywhere_or_one_conversation_with_the_history_merged() {
    use zirc_proto::login::{HistoryMessage, HistoryPage, HistorySender};
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            "@time=2026-10-30T11:00:00.000Z;msgid=m1 :bob!u@h PRIVMSG #api :the Deploy failed",
            "@time=2026-10-30T11:10:00.000Z;msgid=m2 :ada!u@h PRIVMSG #web :deploying web",
            &dm("me-mbp", "cc-1531f8", "deploy is green", 1),
        ],
    );
    let wall = |at: DateTime<Utc>| at.naive_utc();
    // Everywhere: every conversation's matches, newest first, cut for highlighting, each
    // named as a message's head names them: an agent by its session, never the machine.
    let view = chat.search_view("deploy", None, None, wall);
    let found: Vec<(&str, &str, &str)> = view
        .hits
        .iter()
        .map(|hit| {
            (
                hit.conversation.as_str(),
                hit.when.as_str(),
                hit.name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("me-mbp/cc-1531f8", "12:01", "cc-1531f8"),
            ("#web", "11:10", "ada"),
            ("#api", "11:00", "bob"),
        ]
    );
    // Each with the face its rows have: a person's own, an agent session's owner's.
    assert!(matches!(&view.hits[1].face, Face::Person { nick, .. } if nick == "ada"));
    assert!(matches!(&view.hits[0].face, Face::Person { nick, .. } if nick == "me"));
    // What a screen reader says of one, as every app says it.
    assert_eq!(
        view.hits[1].a11y,
        format!(
            "{} in #web, {}: {}",
            view.hits[1].name,
            view.hits[1].when,
            view.hits[1]
                .parts
                .iter()
                .map(|part| part.text.as_str())
                .collect::<String>()
        )
    );
    assert_eq!(
        view.hits[2].parts,
        [
            HitPart {
                text: "the ".into(),
                hit: false
            },
            HitPart {
                text: "Deploy".into(),
                hit: true
            },
            HitPart {
                text: " failed".into(),
                hit: false
            },
        ]
    );
    assert_eq!((view.scope, view.more), (None, false));

    // In one conversation, with what the history gave before it: not repeated, and more to
    // ask for until the history says it's done.
    let mut older = chat.search("#api", "deploy");
    let said = |id: &str, time: &str| HistoryMessage {
        msgid: id.into(),
        time: time.into(),
        conversation: "#api".into(),
        sender: HistorySender {
            account: Some("bob".into()),
            nick: "bob".into(),
            kind: "person".into(),
        },
        text: "deploy notes".into(),
        zirc: None,
    };
    older.take_page(&HistoryPage {
        policy: "channels".into(),
        messages: vec![
            said("m1", "2026-10-30T11:00:00.000Z"),
            said("m0", "2026-10-29T09:00:00.000Z"),
        ],
        next: Some("c1".into()),
        indexed: false,
        counts: None,
    });
    let view = chat.search_view("deploy", Some("#api"), Some(&older), wall);
    let found: Vec<(Option<&str>, bool)> = view
        .hits
        .iter()
        .map(|hit| (hit.id.as_deref(), hit.loaded))
        .collect();
    assert_eq!(found, [(Some("m1"), true), (Some("m0"), false)]);
    assert_eq!(view.scope_title.as_deref(), Some("#api"));
    assert!(view.more);
}

#[test]
fn searches_direct_messages_and_an_agents_own_lines_on_the_server() {
    use zirc_proto::login::{HistoryMessage, HistoryPage, HistorySender};
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[":bob!u@h PRIVMSG me :hi", &dm("me-mbp", "cc-1", "done", 1)],
    );
    assert_eq!(
        chat.search("bob", "x").server_query().unwrap().conversation,
        "@bob"
    );
    let mut agent = chat.search("me-mbp/cc-1", "tests");
    assert_eq!(agent.server_query().unwrap().conversation, "@me-mbp");
    let said = |id: &str, session: &str| HistoryMessage {
        msgid: id.into(),
        time: "2026-10-29T09:00:00.000Z".into(),
        conversation: "@me-mbp".into(),
        sender: HistorySender {
            account: Some("me-mbp".into()),
            nick: "me-mbp".into(),
            kind: "agent".into(),
        },
        text: "tests pass".into(),
        zirc: Some(serde_json::json!({ "session": session })),
    };
    agent.take_page(&HistoryPage {
        policy: "participant".into(),
        messages: vec![said("a1", "cc-1"), said("a2", "cc-2")],
        next: None,
        indexed: false,
        counts: None,
    });
    let ids: Vec<_> = agent
        .hits
        .iter()
        .filter_map(|hit| hit.id.as_deref())
        .collect();
    assert_eq!(ids, ["a1"], "only that session's");
    // Nothing to find: nothing asked.
    assert!(chat.search("bob", "  ").server_query().is_none());
}

#[test]
fn highlights_matches_ignoring_case() {
    assert_eq!(highlights("Deploy, then deploy", "DEPLOY"), [0..6, 13..19]);
    assert_eq!(highlights("Ärger über ÄRGER", "ärger"), [0..6, 13..19]);
    assert!(highlights("anything", " ").is_empty());
    assert!(highlights("short", "longer").is_empty());
}

/// `#api` with talk, machines' commits, a join and CI's posts between.
fn talk_among_system_rows() -> Chat {
    let mut chat = Scene::in_api();
    chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
    let at = |time: &str, line: &str| format!("@time=2026-10-30T{time}.000Z {line}");
    let commit = |sha: &str| {
        Message::new(
            "cc-1",
            Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: sha.into(),
                subject: "Fix".into(),
                repo: None,
            },
        )
    };
    let landed = ci_post(
        LANDING_KIND,
        serde_json::json!({ "build": "263", "url": "https://ci/263" }),
        "cc-598aa1: landed decisions as 2ba614c (build 263)",
    );
    let deployed = ci_post(
        zirc_proto::ci::DEPLOY_KIND,
        serde_json::json!({}),
        "cc-598aa1: deployed zircd 2ba614c",
    );
    feed(
        &mut chat,
        &[
            ":s 353 me = #api :me alice carol-mbp",
            ":s 366 me #api :End",
            &at("11:00:00", ":alice!u@h PRIVMSG #api :first"),
            &at("11:01:00", &zirc("carol-mbp", &commit("aaa1111"))),
            &at("11:01:30", ":dave!u@h JOIN #api"),
            &at("11:02:00", ":alice!u@h PRIVMSG #api :still me"),
            &at("11:03:00", &zirc("carol-mbp", &commit("bbb2222"))),
            &at("11:03:30", &zirc("carol-mbp", &commit("ccc3333"))),
            &at("11:04:00", &zirc("zirc-alerts", &landed)),
            &at("11:04:30", &zirc("zirc-alerts", &deployed)),
            &at("11:10:00", ":alice!u@h PRIVMSG #api :later"),
        ],
    );
    chat.into_chat()
}

/// A system row in words: its label, actor, verb and details.
fn header(parts: &Parts) -> (&str, &str, &str, &str) {
    (
        parts.icon.label(),
        &parts.actor,
        &parts.verb,
        &parts.details,
    )
}

#[test]
fn an_events_header_never_says_its_kind_and_its_lead_says_it_once() {
    let chat = alice_and_her_machine();
    let conversation = chat.conversation("#api").unwrap();
    let items = chat.items(conversation, |time| time.date_naive());
    let events: Vec<&Parts> = items
        .iter()
        .filter_map(|item| match item {
            Item::Event { parts, .. } => Some(parts),
            _ => None,
        })
        .collect();
    let icons: Vec<Icon> = events.iter().map(|parts| parts.icon).collect();
    assert!(
        icons.contains(&Icon::Intent) && icons.contains(&Icon::Taking),
        "{icons:?}"
    );
    for parts in events {
        // The header is who, whose agent and the time: no kind word, no verb.
        let header = [
            parts.actor.as_str(),
            parts.actor_detail.as_deref().unwrap_or(""),
        ];
        for said in header {
            assert!(
                !said.contains(parts.icon.label()),
                "{said:?} says {:?}",
                parts.icon
            );
            assert!(
                parts.verb.is_empty() || !said.contains(&parts.verb),
                "{said:?}"
            );
        }
        // The body starts with the marker and the verb, once.
        let lead = parts.lead();
        assert!(lead.starts_with(parts.icon.glyph()), "{lead:?}");
        assert_eq!(lead.matches(parts.icon.glyph()).count(), 1, "{lead:?}");
        assert!(lead.ends_with(&parts.verb), "{lead:?}");
    }
}

#[test]
fn runs_carry_on_past_system_rows_and_events_are_talk_in_their_runs() {
    let chat = talk_among_system_rows();
    let conversation = chat.conversation("#api").unwrap();
    let items = chat.items(conversation, |time| time.date_naive());
    let shown: Vec<String> = items.iter().map(shown).collect();
    assert_eq!(
        shown,
        [
            "--- 2026-10-30",
            "alice: first",
            "◆ cc-1 committed on main: Fix",
            "· dave joined",
            // An event between ends alice's run: it's carol's agent's talk.
            "alice: still me",
            // Never folded: each event is a message, the second in the first's run.
            "◆ cc-1 committed on main: Fix",
            "◆ cc-1 committed on main: Fix",
            "= Landed cc-598aa1: landed decisions as 2ba614c (build 263)",
            // An integration's posts are messages, never folded, and run on.
            "= Other cc-598aa1: deployed zircd 2ba614c",
            // Over five minutes on: a new run.
            "alice: later",
        ]
    );
    let Item::Landing(landing) = &items[7] else {
        panic!("not a landing")
    };
    assert_eq!(
        (landing.session, landing.commit, landing.build, landing.url),
        (
            Some("cc-598aa1"),
            Some("2ba614c"),
            Some("263"),
            Some("https://ci/263")
        )
    );
    let (
        Item::Event {
            continues: first, ..
        },
        Item::Event {
            continues: second, ..
        },
    ) = (&items[5], &items[6])
    else {
        panic!("not two events: {shown:?}")
    };
    assert!(!*first && *second);
    let Item::Landing(deploy) = &items[8] else {
        panic!("not an integration's post")
    };
    assert!(!landing.continues);
    assert!(deploy.continues);
    assert_eq!(
        (deploy.kind, deploy.session),
        (zirc_proto::ci::DEPLOY_KIND, Some("cc-598aa1"))
    );
    assert_eq!(deploy.said(), "deployed zircd 2ba614c");
}

#[test]
fn an_agents_post_and_its_intent_are_one_run() {
    let mut chat = Scene::in_api();
    let post = Message::new(
        "cc-1",
        Event::Text {
            body: "the form posts twice".into(),
        },
    );
    let intent = Message::new(
        "cc-1",
        Event::Intent {
            summary: "Fix the login form".into(),
            paths: vec!["web/src/login.tsx".into()],
            expires: None,
            lasts: None,
        },
    );
    feed(
        &mut chat,
        &[
            &format!("@time=2026-10-30T11:00:00.000Z {}", zirc("dana-mbp", &post)),
            &format!(
                "@time=2026-10-30T11:00:01.000Z {}",
                zirc("dana-mbp", &intent)
            ),
        ],
    );
    let items = chat.items(api(&chat), |time| time.date_naive());
    let runs: Vec<(Icon, bool)> = items
        .iter()
        .filter_map(|item| match item {
            Item::Event {
                parts, continues, ..
            } => Some((parts.icon, *continues)),
            _ => None,
        })
        .collect();
    assert_eq!(runs, [(Icon::Post, false), (Icon::Intent, true)]);
}

#[test]
fn ranks_the_quick_switchers_matches_as_every_app_does() {
    // A title starting with it, then a word of it, then every word anywhere.
    assert_eq!(switch_rank("api", "#api", None), Some(0));
    assert_eq!(switch_rank("#API", "#api-docs", None), Some(0));
    assert_eq!(switch_rank("docs", "#api-docs", None), Some(1));
    assert_eq!(switch_rank("pi do", "#api-docs", None), Some(2));
    assert_eq!(
        switch_rank(
            "coupon",
            "checkout · Henrik Sandberg",
            Some("the coupon field")
        ),
        Some(2)
    );
    assert_eq!(switch_rank("web", "#api", None), None);
    assert_eq!(switch_rank("", "#api", None), Some(0));

    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api-docs",
            ":me!u@h JOIN #docs",
            ":bob!u@h PRIVMSG #api-docs :new page",
        ],
    );
    // #docs starts with it; #api-docs has a word that does: rank first, then unread.
    assert_eq!(chat.switcher("docs"), ["#docs", "#api-docs"]);
    // Both start with it: what's unread first.
    assert_eq!(chat.switcher("ap"), ["#api-docs", "#api"]);
}

#[test]
fn comings_and_goings_never_end_a_run_but_events_do() {
    let mut chat = Scene::in_api();
    chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
    let at = |time: &str, line: &str| format!("@time=2026-11-03T{time}.000Z {line}");
    let landed = |build: &str| {
        ci_post(
            LANDING_KIND,
            serde_json::json!({ "build": build }),
            &format!("cc-ad4f7c: landed a branch (build {build})"),
        )
    };
    let push = Message::new(
        "cc-1",
        Event::Push {
            branch: "login-page".into(),
            head: "abc1234".into(),
            forced: false,
            subjects: Vec::new(),
            paths: Vec::new(),
            base: None,
            lines: std::collections::BTreeMap::new(),
            repo: None,
            on: None,
        },
    );
    let commit = Message::new(
        "cc-1",
        Event::Commit {
            paths: Vec::new(),
            branch: "main".into(),
            sha: "def5678".into(),
            subject: "Fix".into(),
            repo: None,
        },
    );
    feed(
        &mut chat,
        &[
            // As in #monorepo: CI's landing, an agent's push, then CI's next landing.
            &at("11:43:00", &zirc("zirc-alerts", &landed("606"))),
            &at("11:44:32", &zirc("mattias-matt", &push)),
            &at("11:44:38", &zirc("zirc-alerts", &landed("608"))),
            // Talk, an event, then talk from the same person.
            &at("11:45:00", ":alice!u@h PRIVMSG #api :one"),
            &at("11:45:10", &zirc("carol-mbp", &commit)),
            &at("11:45:20", ":alice!u@h PRIVMSG #api :two"),
            // Talk, a join, then talk: one run.
            &at("11:45:30", ":dave!u@h JOIN #api"),
            &at("11:45:40", ":alice!u@h PRIVMSG #api :three"),
        ],
    );
    let items = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let runs: Vec<(String, bool)> = items
        .iter()
        .filter_map(|item| match item {
            Item::Landing(landing) => {
                Some((format!("build {:?}", landing.build), landing.continues))
            }
            Item::Message {
                text, continues, ..
            } => Some(((*text).to_owned(), *continues)),
            _ => None,
        })
        .collect();
    assert_eq!(
        runs,
        [
            ("build Some(\"606\")".to_owned(), false),
            // After the push, CI's landing has its own header, never the agent's.
            ("build Some(\"608\")".to_owned(), false),
            ("one".to_owned(), false),
            ("two".to_owned(), false),
            ("three".to_owned(), true),
        ]
    );
}

#[test]
fn names_sessions_and_says_who_a_post_is_for() {
    let mut chat = Scene::in_api();
    let mut named = Message::new(
        "cc-598aa1",
        Event::Text {
            body: "hello".into(),
        },
    );
    named.name = Some("Zirc Chat".into());
    let to = Message::new(
        "cc-ad4f7c",
        Event::Text {
            body: "cc-598aa1: thanks, landed".into(),
        },
    );
    feed(
        &mut chat,
        &[&zirc("mattias-matt", &named), &zirc("mattias-matt", &to)],
    );
    assert_eq!(chat.session_label("cc-598aa1"), "zirc-chat");
    assert_eq!(chat.session_label("cc-ad4f7c"), "cc-ad4f7c");
    let shown = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let posts: Vec<(Option<String>, String)> = shown
        .iter()
        .filter_map(|item| match item {
            Item::Event { parts, .. } => Some((parts.to.clone(), parts.details.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        posts,
        [
            (None, "hello".to_owned()),
            (Some("cc-598aa1".to_owned()), "thanks, landed".to_owned()),
        ]
    );
    // Its name is its own machine's to say: someone else sending its id doesn't rename it.
    let mut renamed = named.clone();
    renamed.name = Some("deploy-prod".into());
    feed(&mut chat, &[&zirc("bob-mbp", &renamed)]);
    assert_eq!(chat.session_label("cc-598aa1"), "zirc-chat");
}

#[test]
fn an_agent_s_later_lines_go_by_the_name_it_gave() {
    let mut chat = Scene::in_api();
    let mut named = Message::new(
        "cc-598aa1",
        Event::Text {
            body: "hello".into(),
        },
    );
    named.name = Some("zirc-chat".into());
    feed(&mut chat, &[&zirc("mattias-matt", &named)]);
    // Later lines needn't say it again.
    let unnamed = Message::new(
        "cc-598aa1",
        Event::Text {
            body: "pushed".into(),
        },
    );
    feed(&mut chat, &[&zirc("mattias-matt", &unnamed)]);
    let shown = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let Some(Item::Event { parts, text, .. }) = shown.last() else {
        panic!("not an event: {:?}", shown.last())
    };
    assert_eq!(
        (parts.actor.as_str(), text.as_str()),
        ("zirc-chat", "zirc-chat: pushed")
    );
}

#[test]
fn a_commit_and_its_push_are_one_row() {
    let mut chat = Scene::in_api();
    chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
    let at = |time: &str, line: &str| format!("@time=2026-11-03T{time}.000Z {line}");
    let commit = |session: &str, sha: &str| {
        Message::new(
            session,
            Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: sha.into(),
                subject: "Fix".into(),
                repo: None,
            },
        )
    };
    let push = |session: &str, head: &str| {
        Message::new(
            session,
            Event::Push {
                branch: "main".into(),
                head: head.into(),
                forced: false,
                subjects: Vec::new(),
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: None,
                on: None,
            },
        )
    };
    feed(
        &mut chat,
        &[
            &at(
                "11:00:00",
                &zirc("mattias-matt", &commit("cc-1", "abc1234")),
            ),
            &at(
                "11:00:05",
                &zirc("mattias-matt", &push("cc-1", "abc1234def")),
            ),
            &at("11:00:30", ":alice!u@h PRIVMSG #api :between"),
            // Another session's push of the same commit isn't this commit's.
            &at(
                "11:01:00",
                &zirc("mattias-matt", &commit("cc-1", "fff0000")),
            ),
            &at("11:01:05", &zirc("carol-mbp", &push("cc-2", "fff0000"))),
        ],
    );
    let shown = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let Item::Event { parts, .. } = &shown[1] else {
        panic!("not an event: {:?}", shown[1])
    };
    assert_eq!(parts.verb, "committed and pushed to main");
    // Another session's push stays a row of its own, after the commit's: neither folds.
    let verbs: Vec<&str> = shown
        .iter()
        .filter_map(|item| match item {
            Item::Event { parts, .. } => Some(parts.verb.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        verbs,
        [
            "committed and pushed to main",
            "committed",
            "pushed main to fff0000"
        ]
    );
}

#[test]
fn heads_each_row_with_an_avatar_a_name_and_a_label() {
    let mut chat = chat();
    feed(&mut chat, &[":mattias-matt!u@machine/mattias JOIN #api"]);
    let entry = |from: &str, kind: EntryKind| Entry {
        from: from.into(),
        owner: zirc_proto::names::owner_from_host(&host(from)).map(str::to_owned),
        kind,
        ..Entry::info(noon(), String::new())
    };
    let mut post = Message::new("cc-598aa1", Event::Text { body: "hi".into() });
    post.name = Some("zirc-chat".into());
    post.tool = Some(zirc_proto::CLAUDE_CODE.into());
    post.avatar = Some("Rocket".into());
    let mut unknown = post.clone();
    unknown.avatar = Some("unicorn".into());
    let agent = chat.speaker(&entry("mattias-matt", EntryKind::Zirc(post)));
    assert_eq!(agent.name, "zirc-chat");
    assert_eq!(agent.detail.as_deref(), Some("mattias's agent"));
    assert_eq!(agent.label, "zirc-chat, agent of mattias");
    let Avatar::Agent {
        tool,
        owner,
        session,
        hue,
        picture,
    } = &agent.avatar
    else {
        panic!("not an agent: {agent:?}")
    };
    assert_eq!(tool.as_deref(), Some("claude-code"));
    // A picture from the gallery, by its id; one the gallery hasn't is none.
    assert_eq!(picture.as_deref(), Some("rocket"));
    let unknown = chat.speaker(&entry("mattias-matt", EntryKind::Zirc(unknown)));
    assert!(matches!(
        unknown.avatar,
        Avatar::Agent { picture: None, .. }
    ));
    assert_eq!(owner, "mattias");
    assert_eq!(session.as_deref(), Some("cc-598aa1"));
    assert!(*hue < 360);

    // The machine itself, with no session: its own name and its owner's machine.
    let machine = chat.speaker(&entry(
        "mattias-matt",
        EntryKind::Zirc(Message::new("watcher", Event::Text { body: "x".into() })),
    ));
    assert_eq!(machine.name, "mattias-matt");
    assert_eq!(machine.detail.as_deref(), Some("mattias's machine"));

    let person = chat.speaker(&entry("alice", EntryKind::Message("hi".into())));
    assert_eq!(
        (person.name.as_str(), person.label.as_str()),
        ("alice", "alice")
    );
    assert_eq!(
        person.avatar,
        Avatar::Person {
            nick: "alice".into()
        }
    );

    let ci = chat.speaker(&entry(
        "zirc-alerts",
        EntryKind::Zirc(ci_post(LANDING_KIND, serde_json::json!({}), "landed")),
    ));
    assert_eq!(ci.name, "CI");
    assert_eq!(ci.label, "CI, app");
    assert!(matches!(ci.avatar, Avatar::App { ref glyph, .. } if glyph == "CI"));
}

#[test]
fn each_agent_session_speaks_as_itself() {
    let mut chat = Scene::in_api();
    let at = |time: &str, line: &str| format!("@time=2026-11-03T{time}.000Z {line}");
    let post = |session: &str, name: Option<&str>, text: &str| {
        let mut message = Message::new(session, Event::Text { body: text.into() });
        message.name = name.map(str::to_owned);
        message
    };
    feed(
        &mut chat,
        &[
            &at(
                "11:40:00",
                &zirc("mattias-matt", &post("cc-598aa1", Some("Zirc Chat"), "one")),
            ),
            &at(
                "11:40:10",
                &zirc("mattias-matt", &post("cc-598aa1", Some("Zirc Chat"), "two")),
            ),
            // Another session on the same machine is another speaker.
            &at(
                "11:40:20",
                &zirc("mattias-matt", &post("cc-ad4f7c", None, "three")),
            ),
            // The machine itself is its owner's agent.
            &at(
                "11:40:30",
                &zirc("mattias-matt", &post("watcher", None, "four")),
            ),
        ],
    );
    let items = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let rows: Vec<(String, Option<String>, Option<String>, bool)> = items
        .iter()
        .filter_map(|item| match item {
            Item::Event {
                parts, continues, ..
            } => Some((
                parts.actor.clone(),
                parts.session.clone(),
                parts.owner.clone(),
                *continues,
            )),
            _ => None,
        })
        .collect();
    let row = |actor: &str, session: Option<&str>, continues| {
        (
            actor.to_owned(),
            session.map(str::to_owned),
            Some("mattias".to_owned()),
            continues,
        )
    };
    assert_eq!(
        rows,
        [
            row("zirc-chat", Some("cc-598aa1"), false),
            row("zirc-chat", Some("cc-598aa1"), true),
            row("cc-ad4f7c", Some("cc-ad4f7c"), false),
            row("mattias's agent", None, false),
        ]
    );
    // Files shared by two sessions on one machine are two runs.
    let file = |session: &str, id: &str| {
        Message::new(
            session,
            Event::File(zirc_proto::File {
                url: format!("https://zirc.dev/api/media/{id}"),
                name: "a.png".into(),
                mime: "image/png".into(),
                size: 1,
                width: None,
                height: None,
                duration_ms: None,
            }),
        )
    };
    feed(
        &mut chat,
        &[
            &at("11:41:00", &zirc("mattias-matt", &file("cc-598aa1", "f1"))),
            &at("11:41:10", &zirc("mattias-matt", &file("cc-598aa1", "f2"))),
            &at("11:41:20", &zirc("mattias-matt", &file("cc-ad4f7c", "f3"))),
        ],
    );
    let shown = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let files: Vec<bool> = shown
        .iter()
        .filter_map(|item| match item {
            Item::File { continues, .. } => Some(*continues),
            _ => None,
        })
        .collect();
    assert_eq!(files, [false, true, false]);
}
#[test]
fn actions_carry_on_talk_and_events_carry_on_their_senders() {
    let mut chat = talk_among_system_rows();
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:10:30.000Z :alice!u@h PRIVMSG #api :\u{1}ACTION waves\u{1}",
            "@time=2026-10-30T11:11:00.000Z :alice!u@h PRIVMSG #api :and talks",
        ],
    );
    let items = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let runs: Vec<bool> = items[items.len() - 2..]
        .iter()
        .map(|item| match item {
            Item::Action { continues, .. } | Item::Message { continues, .. } => *continues,
            _ => panic!("not talk"),
        })
        .collect();
    assert_eq!(runs, [true, true]);
    // The same machine's events carry on from each other, as its talk does.
    let runs: Vec<bool> = items[5..7]
        .iter()
        .map(|item| {
            matches!(
                item,
                Item::Event {
                    continues: true,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(runs, [false, true]);
}

#[test]
fn an_events_body_starts_with_its_marker_and_what_was_done() {
    let chat = talk_among_system_rows();
    let items = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let Item::Event { parts, .. } = &items[2] else {
        panic!("not an event")
    };
    // Never a kind word: the marker and the verb say it once.
    assert_eq!(parts.lead(), "● committed");
    assert_eq!(Icon::Taking.glyph(), "⟶");
    assert_eq!(Icon::Join.glyph(), "+");
    assert_eq!(Icon::Leave.glyph(), "−");
}

#[test]
fn system_rows_come_in_parts() {
    let chat = talk_among_system_rows();
    let items = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    // A header, then details: the agent session, by its name or else its id, never the
    // device.
    let Item::Event { parts, .. } = &items[2] else {
        panic!("not an event")
    };
    assert_eq!(
        (parts.session.as_deref(), parts.owner.as_deref()),
        (Some("cc-1"), Some("carol"))
    );
    assert_eq!(
        header(parts),
        ("commit", "cc-1", "committed", "Fix  (main aaa1111)")
    );
    let Item::Landing(landing) = &items[7] else {
        panic!("not landed")
    };
    let landed = landing.parts();
    assert_eq!(
        header(&landed),
        (
            "landed",
            "zirc-alerts",
            "",
            "landed decisions as 2ba614c (build 263)"
        )
    );
    let Item::Landing(deploy) = &items[8] else {
        panic!("not an integration's post")
    };
    assert_eq!(header(&deploy.parts()).0, "post");
}

/// What CI posts: an event of `kind`, from its `ci` session.
fn ci_post(kind: &str, data: serde_json::Value, text: &str) -> Message {
    Message::new(
        "ci",
        Event::Custom {
            kind: kind.into(),
            data,
            text: text.into(),
        },
    )
}

#[test]
fn tells_people_agents_and_integrations_apart() {
    assert_eq!(sender_kind("zirc-alerts", None), SenderKind::Integration);
    assert_eq!(sender_kind("ZIRC", None), SenderKind::Integration);
    // A machine by its host, which says whose it is, never by its name.
    assert_eq!(sender_kind("m3-mac", Some("carol")), SenderKind::Agent);
    assert_eq!(sender_kind("carol-mbp", None), SenderKind::Person);
    assert_eq!(sender_kind("dana", None), SenderKind::Person);
    // An integration's short name: its own, its session's, or its nick's.
    let mut post = ci_post(
        zirc_proto::ci::DEPLOY_KIND,
        serde_json::json!({}),
        "deployed",
    );
    assert_eq!(app_label("zirc-alerts", Some(&post)).as_deref(), Some("ci"));
    post.app = Some("Deploy".into());
    assert_eq!(
        app_label("zirc-alerts", Some(&post)).as_deref(),
        Some("deploy")
    );
    assert_eq!(app_label("zirc-alerts", None).as_deref(), Some("alerts"));
    assert_eq!(app_label("zirc", None).as_deref(), Some("zirc"));
    assert_eq!(app_label("carol-mbp", Some(&post)), None);
    // A system row's who: apps by their name, never "zirc's agent"; a machine as its
    // owner's agent, the owner as named.
    let named = |owner: &str| {
        if owner == "carol" {
            "Carol".to_owned()
        } else {
            owner.to_owned()
        }
    };
    assert_eq!(actor("zirc-alerts", None, &named), "Alerts");
    assert_eq!(actor("zirc-ci", None, &named), "CI");
    assert_eq!(actor("m3-mac", Some("carol"), &named), "Carol's agent");
    assert_eq!(actor("dana", None, &named), "dana");
}

#[test]
fn lists_the_feeds_among_the_channels_never_bold() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #ops",
            ":me!u@h JOIN #decisions",
            ":me!u@h JOIN #web",
            ":me!u@h JOIN #web/landings",
            "@time=2026-10-30T11:00:00.000Z :zirc-ci!u@h PRIVMSG #ops :build 5 failed",
        ],
    );
    let rows = chat.sidebar_list(&ListOptions::default());
    let channels: Vec<(&str, bool)> = rows
        .iter()
        .filter(|row| row.shelf == Shelf::Channels && row.kind == ListKind::Conversation)
        .map(|row| (row.name.as_str(), row.unread))
        .collect();
    // Joined, every one is listed under Channels, nothing folded away, and what systems
    // post there, #ops' build and #web's standing room, never shows bold.
    assert_eq!(
        channels,
        [
            ("#api", false),
            ("#decisions", false),
            ("#ops", false),
            ("#web", false),
        ]
    );
    assert!(chat.collapsed().is_empty());
}

#[test]
fn lays_out_the_sidebar_starred_channels_then_direct_messages() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web",
            ":me!u@h JOIN #api/login",
            ":me!u@h JOIN #api",
            // Said in today, so an active thread, which the sidebar lists.
            "@time=2026-10-30T10:59:00.000Z :me!u@h PRIVMSG #api/login :looking",
            "@time=2026-10-30T11:00:00.000Z :bob!u@h PRIVMSG me :hi",
            "@time=2026-10-30T11:05:00.000Z :carol!u@h PRIVMSG me :hey",
            "@time=2026-10-30T11:06:00.000Z :dave!u@h PRIVMSG me :yo",
            &dm("me-mbp", "cc-1", "done", 1),
        ],
    );
    chat.star("carol", true);
    let shelves = |chat: &Chat| -> Vec<(Shelf, Vec<String>)> {
        let mut shelves: Vec<(Shelf, Vec<String>)> = Vec::new();
        for row in chat.sidebar_list(&ListOptions::default()) {
            match row.kind {
                ListKind::Heading => shelves.push((row.shelf, Vec::new())),
                ListKind::Conversation | ListKind::Room => {
                    let indent = "  ".repeat(usize::from(row.depth));
                    shelves
                        .last_mut()
                        .unwrap()
                        .1
                        .push(format!("{indent}{}", row.name));
                }
            }
        }
        shelves
    };
    assert_eq!(
        shelves(&chat),
        [
            (Shelf::Starred, vec!["carol".to_owned()]),
            (
                Shelf::Channels,
                vec![
                    "#api".to_owned(),
                    "  #api/login".to_owned(),
                    "#web".to_owned()
                ]
            ),
            // People's and our agent's, newest first.
            (
                Shelf::Direct,
                vec![
                    "me-mbp/cc-1".to_owned(),
                    "dave".to_owned(),
                    "bob".to_owned()
                ]
            ),
        ]
    );
    // What's new in each row: dave's and bob's direct messages count, as everything unread
    // in one does.
    let rows = chat.sidebar_list(&ListOptions::default());
    assert!(
        rows.iter()
            .filter(|row| row.shelf == Shelf::Direct && row.kind == ListKind::Conversation)
            .all(|row| row.unread && row.badge == 1)
    );
    chat.set_collapsed(Shelf::Direct, true);
    // Nothing else is folded until it's folded.
    assert_eq!(chat.collapsed(), [Shelf::Direct]);
    let rows = chat.sidebar_list(&ListOptions::default());
    let heading = rows
        .iter()
        .find(|row| row.kind == ListKind::Heading && row.shelf == Shelf::Direct)
        .unwrap();
    assert_eq!(heading.open, Some(false));
}

#[test]
fn says_where_unread_starts_and_counts_mentions_and_direct_messages() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            ":s MARKREAD #web timestamp=2026-10-30T11:00:00.000Z",
            "@time=2026-10-30T10:59:00.000Z;msgid=w0 :bob!u@h PRIVMSG #web :old",
            "@time=2026-10-30T11:01:00.000Z;msgid=w1 :bob!u@h PRIVMSG #web :new",
            "@time=2026-10-30T11:02:00.000Z;msgid=w2 :bob!u@h PRIVMSG #web :me: look",
            "@time=2026-10-30T11:03:00.000Z;msgid=d1 :bob!u@h PRIVMSG me :psst",
        ],
    );
    let _ = chat.select(chat.position("#api").unwrap());
    assert_eq!(chat.first_unread("#web").as_deref(), Some("w1"));
    // A channel counts what mentions us; a direct message everything unread.
    assert_eq!(chat.badge("#web"), 1);
    assert_eq!(chat.badge("bob"), 1);
    assert_eq!(chat.badge_total(), 2);
    let _ = chat.mark_read("#web");
    assert_eq!(chat.first_unread("#web"), None);
    assert_eq!(chat.badge_total(), 1);
}

#[test]
fn keeps_the_new_divider_where_the_shown_conversation_was_unread() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            ":s MARKREAD #web timestamp=2026-10-30T11:00:00.000Z",
            "@time=2026-10-30T10:59:00.000Z;msgid=w0 :bob!u@h PRIVMSG #web :old",
            "@time=2026-10-30T11:01:00.000Z;msgid=w1 :bob!u@h PRIVMSG #web :new",
        ],
    );
    let _ = chat.select(chat.position("#api").unwrap());
    assert_eq!(chat.divider(), None);
    let _ = chat.select(chat.position("#web").unwrap());
    // Shown, it's read, but the divider stays where the unread began.
    assert_eq!(chat.first_unread("#web"), None);
    assert_eq!(chat.divider(), Some("w1"));
    let _ = chat.mark_read("#web");
    assert_eq!(chat.divider(), None);
    let _ = chat.select(chat.position("#web").unwrap());
    assert_eq!(chat.divider(), None);
}

#[test]
fn nothing_is_read_while_the_app_is_behind_or_the_screen_locked() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #web",
            ":s MARKREAD #web timestamp=2026-10-30T11:00:00.000Z",
        ],
    );
    let _ = chat.select(chat.position("#web").unwrap());
    assert!(chat.reads());
    // The window behind another, or the phone's app in the background: unread, not read.
    assert!(chat.set_in_front(false).is_empty());
    feed(
        &mut chat,
        &["@time=2026-10-30T11:04:00.000Z;msgid=w4 :bob!u@h PRIVMSG #web :me: there?"],
    );
    assert_eq!(chat.conversation("#web").unwrap().unread, 1);
    let sent = chat.set_in_front(true);
    assert!(
        sent.iter()
            .any(|line| line.to_string().starts_with("MARKREAD #web")),
        "{sent:?}"
    );
    assert_eq!(chat.conversation("#web").unwrap().unread, 0);
    // A locked screen, the same, and unlocking reads it.
    let _ = chat.set_idle(0, true);
    assert!(!chat.reads());
    feed(
        &mut chat,
        &["@time=2026-10-30T11:06:00.000Z;msgid=w6 :bob!u@h PRIVMSG #web :still?"],
    );
    assert_eq!(chat.conversation("#web").unwrap().unread, 1);
    let sent = chat.set_idle(0, false);
    assert!(
        sent.iter()
            .any(|line| line.to_string().starts_with("MARKREAD #web")),
        "{sent:?}"
    );
    assert_eq!(chat.conversation("#web").unwrap().unread, 0);
}

#[test]
fn what_comes_to_the_conversation_shown_while_nobody_looks_is_unread_until_they_look_again() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            ":s MARKREAD #web timestamp=2026-10-30T11:00:00.000Z",
        ],
    );
    let _ = chat.select(chat.position("#web").unwrap());
    // Another organisation is shown: #web is out of sight, though still the one selected.
    assert!(chat.set_looking(false).is_empty());
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:04:00.000Z;msgid=w4 :bob!u@h PRIVMSG #web :me: are you there?",
            "@time=2026-10-30T11:05:00.000Z;msgid=w5 :bob!u@h PRIVMSG #web :anyone",
        ],
    );
    let web = chat.conversation("#web").unwrap();
    assert_eq!((web.unread, web.mentions), (2, 1));
    // The rows' badges: a mention in a channel.
    assert_eq!(chat.org_badge().mentions, 1);
    assert_eq!(chat.divider(), Some("w4"));
    // Looking again opens it as if just shown: read, from its New divider, and said so.
    let sent = chat.set_looking(true);
    assert!(
        sent.iter()
            .any(|line| line.to_string().starts_with("MARKREAD #web")),
        "{sent:?}"
    );
    let web = chat.conversation("#web").unwrap();
    assert_eq!((web.unread, web.mentions), (0, 0));
    assert_eq!(chat.divider(), Some("w4"));
    // Looking already, saying so again sends nothing.
    assert!(chat.set_looking(true).is_empty());
}

#[test]
fn counts_whats_new_from_the_divider_and_what_comes_while_reading_further_up() {
    let mut chat = marking();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            ":s MARKREAD #web timestamp=2026-10-30T11:00:00.000Z",
            "@time=2026-10-30T10:59:00.000Z;msgid=w0 :bob!u@h PRIVMSG #web :old",
            "@time=2026-10-30T11:01:00.000Z;msgid=w1 :bob!u@h PRIVMSG #web :new",
            "@time=2026-10-30T11:02:00.000Z;msgid=w2 :bob!u@h PRIVMSG #web :newer",
        ],
    );
    let _ = chat.select(chat.position("#web").unwrap());
    let wall = |at: DateTime<Utc>| at.naive_utc();
    let since = Utc.with_ymd_and_hms(2026, 10, 30, 11, 1, 0).unwrap();
    let unseen = chat.unseen(wall).unwrap();
    assert_eq!(
        (unseen.count, unseen.since_ms),
        (2, since.timestamp_millis())
    );
    // Read to the end, what comes is seen.
    let _ = chat.mark_read("#web");
    feed(
        &mut chat,
        &["@time=2026-10-30T11:03:00.000Z;msgid=w3 :bob!u@h PRIVMSG #web :seen"],
    );
    assert_eq!(chat.unseen(wall), None);
    // Reading further up, what comes is new, from the first of it.
    chat.scrolled(false);
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:04:00.000Z;msgid=w4 :bob!u@h PRIVMSG #web :one",
            "@time=2026-10-30T11:05:00.000Z;msgid=w5 :bob!u@h PRIVMSG #web :two",
        ],
    );
    let since = Utc.with_ymd_and_hms(2026, 10, 30, 11, 4, 0).unwrap();
    assert_eq!(chat.divider(), Some("w4"));
    // Ready to draw, on the app's clock, ten minutes on.
    chat.clock = || Utc.with_ymd_and_hms(2026, 10, 30, 11, 14, 0).unwrap();
    assert_eq!(
        chat.unseen(wall),
        Some(Unseen {
            count: 2,
            since_ms: since.timestamp_millis(),
            label: "2 new messages since 11:04".to_owned(),
            above: "↑ 2 new".to_owned(),
            short: "↓ 2 new".to_owned(),
        })
    );
    // The New line on screen, the pill's said its piece: it goes, the rest stays.
    chat.saw_divider();
    assert_eq!(chat.unseen(wall).unwrap().above, "");
    assert_eq!(chat.unseen(wall).unwrap().count, 2);
    // On a clock an hour ahead, the time is its own.
    let ahead = |at: DateTime<Utc>| (at + chrono::Duration::hours(1)).naive_utc();
    assert_eq!(
        chat.unseen(ahead).unwrap().label,
        "2 new messages since 12:04"
    );
    // One, a day later: the day before the time.
    let _ = chat.mark_read("#web");
    chat.scrolled(false);
    feed(
        &mut chat,
        &["@time=2026-10-30T11:20:00.000Z;msgid=w6 :bob!u@h PRIVMSG #web :lone"],
    );
    chat.clock = || Utc.with_ymd_and_hms(2026, 10, 31, 11, 30, 0).unwrap();
    let one = chat.unseen(wall).unwrap();
    assert_eq!(
        (one.label.as_str(), one.short.as_str()),
        ("1 new message since Fri 11:20", "↓ 1 new")
    );
    // A newer divider's pill shows again.
    assert_eq!(one.above, "↑ 1 new");
}

#[test]
fn notifies_by_each_conversations_level_and_mute() {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":me!u@h JOIN #api", ":me!u@h JOIN #web"]);
    let at = chat.position("#web").unwrap();
    let _ = chat.select(at);
    feed(
        &mut chat,
        &[
            ":bob!u@h PRIVMSG #api :lunch?",
            ":bob!u@h PRIVMSG #api :me: a look?",
            ":bob!u@h PRIVMSG me :psst",
        ],
    );
    let last = |chat: &Chat, name: &str| {
        chat.conversation(name)
            .unwrap()
            .entries
            .last()
            .cloned()
            .unwrap()
    };
    let api = chat.conversation("#api").unwrap().entries.clone();
    let (chatter, mention) = (&api[api.len() - 2], &api[api.len() - 1]);
    let direct = last(&chat, "bob");
    // Mentions, the default: mentions, and every direct message.
    assert_eq!(chat.level("#api"), Level::Mentions);
    assert!(!chat.notifies("#api", chatter) && chat.notifies("#api", mention));
    assert!(chat.notifies("bob", &direct));
    chat.set_level("#api", Level::All);
    assert!(chat.notifies("#api", chatter));
    // Nothing: only what mentions us, which still counts.
    chat.set_level("#api", Level::Nothing);
    assert!(!chat.notifies("#api", chatter) && chat.notifies("#api", mention));
    assert_eq!(chat.badge("#api"), 1, "a mention counts even at Nothing");
    // A direct message at Nothing is quiet, and counts nothing.
    chat.set_level("bob", Level::Nothing);
    assert!(!chat.notifies("bob", &direct));
    assert_eq!(chat.badge("bob"), 0);
    chat.set_level("bob", Level::Mentions);
    // Muted for an hour: quiet until then but for what mentions us, and again after.
    chat.set_level("#api", Level::Mentions);
    chat.mute_for("#api", 1);
    assert!(chat.is_muted("#api") && !chat.notifies("#api", chatter));
    assert!(
        chat.notifies("#api", mention),
        "a mention gets through a mute"
    );
    // Muted: never bold, but what mentions us still counts.
    assert_eq!(chat.badge("#api"), 1);
    let api_row = chat
        .sidebar_list(&ListOptions::default())
        .into_iter()
        .find(|row| row.name == "#api")
        .unwrap();
    assert!(!api_row.unread && api_row.badge == 1);
    chat.clock = || noon() + chrono::Duration::minutes(61);
    assert!(!chat.is_muted("#api") && chat.notifies("#api", mention));
    chat.set_level("#api", Level::All);
    assert!(chat.notifies("#api", chatter));
    chat.mute("bob", Some(Muted::Off));
    assert!(!chat.notifies("bob", &direct));
    // Kept by the app, and given back.
    let kept = chat.notify_settings().clone();
    assert_eq!(kept.muted.get("bob"), Some(&Muted::Off));
    let mut other = self::chat();
    other.set_notify_settings(kept);
    assert!(other.is_muted("bob"));
}

#[test]
fn pages_back_past_machines_events_until_a_screenful_is_said() {
    let mut chat = chat_with(Options {
        history: 3,
        screenful: 2,
        ..Options::default()
    });
    connected(&mut chat);
    // The newest page is all working sets: nothing to read, so the page before it.
    let events = |reference: &str, ids: std::ops::RangeInclusive<u32>| {
        let mut lines = vec![format!(":s BATCH +{reference} chathistory #api")];
        lines.extend(ids.map(|id| {
            let line = old_zirc("bob", &working("cc-1", &["src/a.rs"]));
            format!("@batch={reference};time=2026-10-30T11:{id:02}:00.000Z;msgid=m{id} {line}")
        }));
        lines.push(format!(":s BATCH -{reference}"));
        lines
    };
    let sent = feed_all(&mut chat, &events("a", 7..=9));
    assert_eq!(sent, ["CHATHISTORY BEFORE #api msgid=m7 3"]);
    // Two messages in the next: a screenful here, so no more.
    let sent = feed_all(&mut chat, &page("b", 4..=6));
    assert!(
        !sent.iter().any(|line| line.starts_with("CHATHISTORY")),
        "{sent:?}"
    );
}

#[test]
fn pages_back_a_page_at_a_time_as_the_person_reaches_the_top() {
    let mut chat = chat_with(Options {
        history: 3,
        screenful: 2,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 7..=9));
    assert!(!api(&chat).beginning);
    let older = chat.older("#api").map(|line| line.to_string());
    assert_eq!(older.as_deref(), Some("CHATHISTORY BEFORE #api msgid=m7 3"));
    // Once, while it's on its way.
    assert!(chat.older("#api").is_none());
    // A full page: one page, no more asked for, and the next is from its oldest.
    let sent = feed_all(&mut chat, &page("b", 4..=6));
    assert!(
        !sent.iter().any(|line| line.starts_with("CHATHISTORY")),
        "{sent:?}"
    );
    assert_eq!(
        said(api(&chat)),
        (4..=9).map(|n| format!("<bob> {n}")).collect::<Vec<_>>()
    );
    let older = chat.older("#api").map(|line| line.to_string());
    assert_eq!(older.as_deref(), Some("CHATHISTORY BEFORE #api msgid=m4 3"));
    // A short page is the start: nothing older to ask for.
    feed_all(&mut chat, &page("c", 3..=3));
    assert!(api(&chat).beginning);
    assert!(chat.older("#api").is_none());
}

#[test]
fn a_short_newest_page_is_the_whole_history() {
    let mut chat = chat_with(Options {
        history: 3,
        screenful: 2,
        ..Options::default()
    });
    connected(&mut chat);
    feed_all(&mut chat, &page("a", 1..=2));
    assert!(api(&chat).beginning);
    assert!(chat.older("#api").is_none());
}

#[test]
fn shows_people_by_their_profiles_live_or_from_zirc_dev() {
    let mut chat = Chat::new("me", "irc.zirc.test", vec![], Options::default());
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s 353 me = #api :me ada bob",
            ":s 366 me #api :End",
        ],
    );
    assert_eq!(chat.shown_name("ada"), "ada");
    assert_eq!(chat.avatar_initials("ada"), "A");
    let mut asked = chat.profiles_to_ask();
    asked.sort();
    assert_eq!(asked, ["ada", "bob", "me"]);
    // Asked once.
    assert!(chat.profiles_to_ask().is_empty());
    let profiles: zirc_proto::profile::Profiles = serde_json::from_str(
        r#"{"ada": {"name": "Ada Lovelace", "avatar": "7"}, "bob": {"name": "Bob", "avatar": null}}"#,
    )
    .unwrap();
    chat.take_profiles(&profiles, "https://zirc.dev/");
    assert_eq!(chat.shown_name("ADA"), "Ada Lovelace");
    // Every app's avatar shows the initials of the name they go by.
    assert_eq!(chat.avatar_initials("ada"), "AL");
    assert_eq!(
        chat.avatar_url("ada"),
        Some("https://zirc.dev/api/media/avatar/ada?v=7")
    );
    assert_eq!(chat.avatar_url("bob"), None);
    // What the server says live wins, and a key gone is gone.
    feed(
        &mut chat,
        &[
            ":s METADATA ada display-name * :Ada L.",
            ":s METADATA bob avatar * :https://zirc.dev/api/media/avatar/bob?v=2",
        ],
    );
    assert_eq!(chat.shown_name("ada"), "Ada L.");
    assert_eq!(
        chat.avatar_url("bob"),
        Some("https://zirc.dev/api/media/avatar/bob?v=2")
    );
    feed(&mut chat, &[":s METADATA ada display-name *"]);
    assert_eq!(chat.shown_name("ada"), "ada");
}

#[test]
fn asks_zirc_dev_for_a_picture_the_server_only_named() {
    let mut chat = Chat::new("me", "irc.zirc.test", vec![], Options::default());
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s 353 me = #api :me ada bob cy",
            ":s 366 me #api :End",
            // ada's name and picture, bob's name alone, and that cy has no picture.
            ":s METADATA ada display-name * :Ada Lovelace",
            ":s METADATA ada avatar * :https://zirc.dev/api/media/avatar/ada?v=1",
            ":s METADATA bob display-name * :Bob",
            ":s METADATA cy avatar *",
        ],
    );
    let mut asked = chat.profiles_to_ask();
    asked.sort();
    // bob's picture isn't known yet; ada's and cy's are, as the server said.
    assert_eq!(asked, ["bob", "me"]);
    let profiles: zirc_proto::profile::Profiles =
        serde_json::from_str(r#"{"bob": {"name": "Robert", "avatar": "7"}}"#).unwrap();
    chat.take_profiles(&profiles, "https://zirc.dev");
    // The server's name wins; the picture comes from zirc.dev.
    assert_eq!(chat.shown_name("bob"), "Bob");
    assert_eq!(
        chat.avatar_url("bob"),
        Some("https://zirc.dev/api/media/avatar/bob?v=7")
    );
    assert!(chat.profiles_to_ask().is_empty());
}

#[test]
fn asks_again_for_profiles_whose_answer_was_never_taken() {
    let mut chat = Chat::new("me", "irc.zirc.test", vec![], Options::default());
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s 353 me = #api :me ada",
            ":s 366 me #api :End",
        ],
    );
    let asked = chat.profiles_to_ask();
    assert!(chat.profiles_to_ask().is_empty());
    chat.ask_profiles_again(&asked);
    let mut again = chat.profiles_to_ask();
    again.sort();
    assert_eq!(again, ["ada", "me"]);
}

#[test]
fn falls_back_to_a_persons_real_name_which_setname_changes() {
    let mut chat = Chat::new("me", "irc.zirc.test", vec![], Options::default());
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":cy!u@h JOIN #api cy :Cyrus Smith",
            ":cy-mbp!u@machine/cy JOIN #api cy-mbp :cy",
            ":dee!u@h JOIN #api dee :dee",
        ],
    );
    // No profile: a person's real name, but never a machine's (its owner) nor a bare nick.
    assert_eq!(chat.shown_name("cy"), "Cyrus Smith");
    assert_eq!(chat.shown_name("cy-mbp"), "cy-mbp");
    assert_eq!(chat.shown_name("dee"), "dee");
    // A person's conversation is titled by it; a machine's, by its owner and own name.
    assert_eq!(chat.title("cy"), "Cyrus Smith");
    assert_eq!(chat.title("cy-mbp"), machine_label("cy", "cy-mbp"));
    feed(&mut chat, &[":cy!u@h SETNAME :Cy Smith"]);
    assert_eq!(chat.shown_name("cy"), "Cy Smith");
}

#[test]
fn offers_people_to_mention_by_nick_or_name() {
    let mut chat = Chat::new("me", "irc.zirc.test", vec![], Options::default());
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":s 353 me = #api :me ada bob adam",
            ":s 366 me #api :End",
        ],
    );
    let profiles: zirc_proto::profile::Profiles =
        serde_json::from_str(r#"{"bob": {"name": "Ada Bobson", "avatar": null}}"#).unwrap();
    chat.take_profiles(&profiles, "https://zirc.dev");
    let nicks: Vec<String> = chat.mentions("ad", 5).into_iter().map(|m| m.nick).collect();
    // Whose nick or name starts with it, by name: bob is Ada Bobson; never us.
    assert_eq!(nicks, ["ada", "bob", "adam"]);
    assert_eq!(chat.mentions("ad", 1).len(), 1);
    assert!(chat.mentions("me", 5).is_empty());
}

#[test]
fn a_landings_words_carry_their_links() {
    let failed = ci_post(
        LANDING_KIND,
        serde_json::json!({ "build": "771" }),
        "cc-598aa1: build 771 failed https://ci/builds/771: //web:e2e",
    );
    let mut chat = chat();
    feed(
        &mut chat,
        &[":me!u@h JOIN #api", &zirc("zirc-alerts", &failed)],
    );
    let conversation = chat.conversation("#api").unwrap();
    let landing = chat
        .items(conversation, |time| time.date_naive())
        .into_iter()
        .find_map(|item| match item {
            Item::Landing(landing) => Some(landing),
            _ => None,
        })
        .expect("a landing row");
    let blocks = landing.rich();
    let [Block::Paragraph(spans)] = blocks.as_slice() else {
        panic!("one paragraph");
    };
    assert!(
        spans
            .iter()
            .any(|span| span.kind == SpanKind::Link("https://ci/builds/771".into())),
        "{spans:?}"
    );
}

/// The spans of `blocks`' first paragraph that are links, as `(text, address)`.
fn links_in(blocks: &[Block]) -> Vec<(String, String)> {
    let Some(Block::Paragraph(spans)) = blocks.first() else {
        panic!("a paragraph first: {blocks:?}");
    };
    spans
        .iter()
        .filter_map(|span| match &span.kind {
            SpanKind::Link(url) => Some((span.text.clone(), url.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_landings_branch_and_commit_link_into_its_repository() {
    let repo = "https://g/a";
    let card = ci_post(
        LANDING_KIND,
        serde_json::json!({
            "landing": "m/771", "build": "771", "branch": "fix-login",
            "repo": repo, "commit": "abc1234def",
        }),
        "cc-1: landed fix-login as abc1234 (build 771)",
    );
    // A row of its own, from before landings had ids, links what its words name.
    let row = ci_post(
        LANDING_KIND,
        serde_json::json!({ "branch": "old", "repo": repo, "commit": "1111111aaa" }),
        "cc-2: landed old as 1111111 (build 9)",
    );
    let mut chat = chat();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            &zirc("zirc-alerts", &card),
            &zirc("zirc-alerts", &row),
        ],
    );
    let landings = landing_cards(&chat);
    assert_eq!(
        links_in(&landings[0].rich()),
        vec![("fix-login".to_owned(), format!("{repo}/tree/fix-login"))]
    );
    assert!(
        landings[0]
            .links()
            .contains(&("abc1234".to_owned(), format!("{repo}/commit/abc1234def"))),
        "{:?}",
        landings[0].links()
    );
    assert_eq!(
        links_in(&landings[1].rich()),
        vec![
            ("old".to_owned(), format!("{repo}/tree/old")),
            ("1111111".to_owned(), format!("{repo}/commit/1111111aaa")),
        ]
    );
}

#[test]
fn a_commits_branch_and_commit_link_into_its_repository() {
    let commit = Message::new(
        "cc-1",
        Event::Commit {
            paths: Vec::new(),
            branch: "fix-login".into(),
            sha: "abc1234def".into(),
            subject: "Fix".into(),
            repo: Some("https://github.com/acme/api".into()),
        },
    );
    let mut chat = chat();
    chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
    feed(
        &mut chat,
        &[":me!u@h JOIN #api", &zirc("carol-mbp", &commit)],
    );
    let parts = chat
        .items(chat.conversation("#api").unwrap(), |time| time.date_naive())
        .into_iter()
        .find_map(|item| match item {
            Item::Event { parts, .. } => Some(parts),
            _ => None,
        })
        .expect("the commit's row");
    let spans = crate::chat::link_words(
        vec![Span {
            text: parts.details.clone(),
            ..Span::default()
        }],
        &parts.links,
    );
    let linked: Vec<(&str, &SpanKind)> = spans
        .iter()
        .filter(|span| matches!(span.kind, SpanKind::Link(_)))
        .map(|span| (span.text.as_str(), &span.kind))
        .collect();
    assert_eq!(
        linked,
        vec![
            (
                "fix-login",
                &SpanKind::Link("https://github.com/acme/api/tree/fix-login".into())
            ),
            (
                "abc1234",
                &SpanKind::Link("https://github.com/acme/api/commit/abc1234def".into())
            ),
        ]
    );
}

/// The landing cards in `#api`.
fn landing_cards(chat: &Chat) -> Vec<Landing<'_>> {
    chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive())
        .into_iter()
        .filter_map(|item| match item {
            Item::Landing(landing) => Some(landing),
            _ => None,
        })
        .collect()
}

/// A span's words, joined.
fn spoken(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

#[test]
fn one_landings_events_are_one_card_that_says_how_it_went() {
    let of =
        serde_json::json!({ "landing": "monorepo/771", "build": "771", "branch": "fix-login" });
    let landed = ci_post(
        LANDING_KIND,
        of.clone(),
        "cc-1: landed fix-login as abc1234 (build 771) https://ci/builds/771",
    );
    let deploying = ci_post(
        zirc_proto::ci::DEPLOY_KIND,
        of.clone(),
        "cc-1: deploying to production: web https://ci/builds/771",
    );
    let deployed = ci_post(
        zirc_proto::ci::DEPLOY_KIND,
        of,
        "cc-1: production runs the web apps as abc1234 has them",
    );
    // Another's, from before landings had ids: a row of its own.
    let old = ci_post(
        LANDING_KIND,
        serde_json::json!({}),
        "cc-2: landed old as 1111111 (build 9)",
    );
    let mut chat = chat();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            &zirc("zirc-alerts", &landed),
            &zirc("zirc-alerts", &old),
            &zirc("zirc-alerts", &deploying),
        ],
    );
    let cards = landing_cards(&chat);
    assert_eq!(cards.len(), 2);
    let line = cards[0].card_line().unwrap();
    assert_eq!(
        spoken(&line),
        "fix-login · build 771 · ✓ landed · deploying"
    );
    // The build is its page's link: no address shows.
    assert!(
        line.iter().any(|span| span.text == "build 771"
            && span.kind == SpanKind::Link("https://ci/builds/771".into())),
        "{line:?}"
    );
    assert_eq!(cards[1].card_line(), None);
    // What apps draw: the state's mark is the row's circle, so the words leave it out.
    let drawn = |card: &Landing<'_>| match &card.rich()[0] {
        Block::Paragraph(line) => spoken(line),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        drawn(&cards[0]),
        "fix-login · build 771 · landed · deploying"
    );
    assert_eq!(cards[0].parts().icon, Icon::Landed);
    // Production updated: the same card, in place, says so.
    feed(&mut chat, &[&zirc("zirc-alerts", &deployed)]);
    let cards = landing_cards(&chat);
    assert_eq!(cards.len(), 2);
    assert_eq!(
        spoken(&cards[0].card_line().unwrap()),
        "fix-login · build 771 · ✓ landed · ✓ production updated"
    );
    // The drawn line keeps the deploy's mark: only the landing's is in the circle.
    assert_eq!(
        drawn(&cards[0]),
        "fix-login · build 771 · landed · ✓ production updated"
    );
}

#[test]
fn a_failed_landings_card_names_its_step_and_carries_cis_details() {
    let failed = ci_post(
        LANDING_KIND,
        serde_json::json!({ "landing": "monorepo/772", "build": "772", "branch": "flat-sizes" }),
        "cc-1: build 772 failed in lint https://ci/builds/772: //web:e2e",
    );
    let mut chat = chat();
    feed(
        &mut chat,
        &[":me!u@h JOIN #api", &zirc("zirc-alerts", &failed)],
    );
    let cards = landing_cards(&chat);
    assert_eq!(
        spoken(&cards[0].card_line().unwrap()),
        "flat-sizes · build 772 · ✗ failed in lint"
    );
    assert_eq!(cards[0].parts().icon, Icon::Failed);
    // Its card, then CI's detail lines under it, as a multiline message brings them.
    let mut card = cards[0].clone();
    card.text = "cc-1: build 772 failed in lint https://ci/builds/772: //web:e2e\n\
                 - failed: `//web:e2e`\n\
                 - run them: `bazel test //web:e2e`";
    let blocks = card.rich();
    assert!(
        matches!(blocks.as_slice(), [Block::Paragraph(_), Block::List { .. }]),
        "{blocks:?}"
    );
}

#[test]
fn a_failed_landings_targets_link_to_their_test_results() {
    let failed = ci_post(
        LANDING_KIND,
        serde_json::json!({
            "landing": "m/7", "failed": [
                { "target": "//web:e2e", "results": "https://r/e2e" },
                { "target": "//oss/apps/desktop:t", "results": "https://r/d" },
            ],
        }),
        "cc-1: build 772 failed: //web:e2e //oss/apps/desk...",
    );
    // CI sends it as a multiline message, longer than one line.
    let line = format!(
        ":zirc-alerts!u@h PRIVMSG #api :{}",
        zirc_proto::encode_up_to(&failed, 2048).unwrap()
    );
    let mut chat = chat();
    feed(&mut chat, &[":me!u@h JOIN #api", &line]);
    let mut card = landing_cards(&chat)[0].clone();
    // Each target its words name, the one cut short too; and in CI's detail lines, as code.
    card.text = "cc-1: build 772 failed: //web:e2e //oss/apps/desk...\n- failed: `//web:e2e`";
    let blocks = card.rich();
    let linked: Vec<(String, String)> = blocks
        .iter()
        .flat_map(|block| match block {
            Block::Paragraph(spans) => spans.clone(),
            Block::List { items, .. } => items
                .iter()
                .flat_map(|item| &item.blocks)
                .flat_map(|block| match block {
                    Block::Paragraph(spans) => spans.clone(),
                    _ => Vec::new(),
                })
                .collect(),
            _ => Vec::new(),
        })
        .filter_map(|span| match span.kind {
            SpanKind::Link(url) => Some((span.text, url)),
            _ => None,
        })
        .collect();
    assert_eq!(
        linked,
        vec![("//web:e2e".to_owned(), "https://r/e2e".to_owned())],
        "the card's line names no target; its details do"
    );
    // A row of its own, from before landings had ids, links its line's targets.
    let mut row = card.clone();
    row.landing = None;
    row.text = "cc-1: build 772 failed: //web:e2e //oss/apps/desk...";
    assert_eq!(
        links_in(&row.rich()),
        vec![
            ("//web:e2e".to_owned(), "https://r/e2e".to_owned()),
            ("//oss/apps/desk...".to_owned(), "https://r/d".to_owned()),
        ]
    );
}

/// An `x-acme/deploy` event carrying `card`.
fn carded(card: &serde_json::Value, text: &str) -> Message {
    ci_post("x-acme/deploy", serde_json::json!({ "card": card }), text)
}

#[test]
fn a_card_is_one_row_its_later_events_update() {
    let first = carded(
        &serde_json::json!({ "id": "d1", "title": "Deploy web", "state": "running" }),
        "deploying web",
    );
    let done = carded(
        &serde_json::json!({
            "id": "d1", "title": "Deploy web", "state": "passed", "about": "cc-1",
            "fields": [{ "label": "commit", "value": "[abc1234](https://g/c)" }],
            "links": [{ "label": "Log", "url": "https://ci/d1" }],
            "actions": [{ "id": "rollback", "label": "Roll back", "who": "admins" }],
            "note": "deployed by mattias",
        }),
        "deployed web",
    );
    // The same id from another sender is a card of its own.
    let theirs = carded(
        &serde_json::json!({ "id": "d1", "title": "Their deploy" }),
        "theirs",
    );
    // A card that breaks the rules is no card: its words show as they are.
    let broken = carded(&serde_json::json!({ "id": "", "title": "no id" }), "broken");
    let mut chat = chat();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            &zirc("deploybot", &first),
            &zirc("carol-mbp", &theirs),
            // As a multiline message, longer than a line, as integrations send cards.
            &format!(
                ":deploybot!u@h PRIVMSG #api :{}",
                zirc_proto::encode_up_to(&done, 2048).unwrap()
            ),
            &zirc("deploybot", &broken),
        ],
    );
    // Two cards, the broken one not among them.
    let cards = landing_cards(&chat);
    assert_eq!(cards.len(), 2, "{cards:?}");
    let card = &cards[0];
    assert_eq!(
        card.card.as_ref().map(|card| card.state),
        Some(zirc_proto::card::State::Passed)
    );
    assert_eq!(card.session, Some("cc-1"));
    assert_eq!(card.parts().icon, Icon::Landed);
    assert_eq!(card.card.as_ref().unwrap().actions[0].id, "rollback");
    let blocks = card.rich();
    let texts: Vec<String> = blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(spans) => spoken(spans),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            "Deploy web passed".to_owned(),
            "commit: abc1234".to_owned(),
            "Log".to_owned(),
            "deployed by mattias".to_owned(),
        ]
    );
    assert_eq!(
        links_in(&blocks[1..]),
        vec![("abc1234".to_owned(), "https://g/c".to_owned())]
    );
    assert_eq!(spoken(&cards[1].card_line().unwrap()), "Their deploy");
}

/// `message` from deploybot in #api: a live one as its TAGMSG, else as a multiline PRIVMSG,
/// as a card with steps is longer than a line.
fn posted(message: &Message) -> String {
    if message.is_live() {
        return zirc("deploybot", message);
    }
    format!(
        ":deploybot!u@h PRIVMSG #api :{}",
        zirc_proto::encode_up_to(message, 4096).unwrap()
    )
}

/// What `rich` says, as its debug form does: enough to find words in.
fn spoken_blocks(rich: &[RichBlock]) -> String {
    format!("{rich:?}")
}

/// A build's card, `state`, with its steps as they stand: rebased, then building `done` of
/// 214 targets, then the push still to come.
fn stepped(state: &str, done: u32, live: bool) -> Message {
    let steps = serde_json::json!([
        { "name": "Rebase onto master", "state": "passed", "detail": "onto 43cdc6e", "secs": 15 },
        { "name": "Build and test", "state": if done == 214 { "passed" } else { "running" },
          "done": done, "of": 214, "secs": 130 },
        { "name": "Fast-forward master", "state": if state == "passed" { "passed" } else { "pending" } },
    ]);
    let mut message = carded(
        &serde_json::json!({ "id": "monorepo/7", "title": "Step cards", "state": state, "steps": steps }),
        "build 7",
    );
    if live {
        message.level = Some(zirc_proto::Level::Debug);
    }
    message
}

#[test]
fn a_cards_steps_show_each_on_its_line_and_fold_once_its_done() {
    let mut chat = chat();
    feed(
        &mut chat,
        &[":me!u@h JOIN #api", &posted(&stepped("running", 81, false))],
    );
    let cards = landing_cards(&chat);
    let blocks = cards[0].rich();
    let Block::Paragraph(steps) = &blocks[1] else {
        panic!("{blocks:?}")
    };
    assert_eq!(
        spoken(steps),
        "✓ Rebase onto master · onto 43cdc6e · 0:15\n\
         ◐ Build and test · ▰▰▰▱▱▱▱▱▱▱ 81 / 214 · 2:10\n\
         ○ Fast-forward master"
    );
    assert!(
        steps
            .iter()
            .any(|span| span.bold && span.text == "Build and test")
    );
    let line = |chat: &Chat| {
        chat.lines("#api", chrono::FixedOffset::east_opt(0).unwrap(), false)
            .into_iter()
            .find(|line| line.system.as_ref().is_some_and(|system| system.card))
            .unwrap()
    };
    assert_eq!(line(&chat).more, None, "a running card shows all its steps");
    // Landed, its steps fold into one line, until opened.
    feed(&mut chat, &[&posted(&stepped("passed", 214, false))]);
    let landed = line(&chat);
    let more = landed.more.clone().unwrap();
    assert_eq!((more.label.as_str(), more.open), ("Show 3 steps", false));
    assert!(
        spoken_blocks(&landed.rich).contains("3 steps · 2:25"),
        "{:?}",
        landed.rich
    );
    chat.toggle_more(&landed.key, false);
    let open = line(&chat);
    assert_eq!(open.more.unwrap().label, "Hide steps");
    assert!(spoken_blocks(&open.rich).contains("Fast-forward master"));
    assert!(!spoken_blocks(&landed.rich).contains("Fast-forward master"));
}

#[test]
fn a_cards_live_updates_fold_into_it_quietly_and_only_the_latest_is_kept() {
    let mut chat = chat();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            &posted(&stepped("running", 0, false)),
            ":alice!u@h PRIVMSG #api :lunch?",
        ],
    );
    let unread = |chat: &Chat| chat.conversation("#api").unwrap().unread;
    let before = unread(&chat);
    let rows = |chat: &Chat| {
        chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive())
            .len()
    };
    let rows_before = rows(&chat);
    // Each goes as a TAGMSG, out of history.
    let live = posted(&stepped("running", 40, true));
    assert!(live.contains(" TAGMSG #api"), "{live}");
    feed(&mut chat, &[&live, &posted(&stepped("running", 81, true))]);
    let cards = landing_cards(&chat);
    assert_eq!(cards[0].card.as_ref().unwrap().steps[1].done, Some(81));
    assert!(cards[0].updates.is_empty(), "no trail line");
    assert_eq!(unread(&chat), before, "nothing unread");
    assert_eq!(
        rows(&chat),
        rows_before,
        "no row, nor a pointer to the card"
    );
    let kept = chat.conversation("#api").unwrap().entries.iter();
    assert_eq!(kept.filter(|entry| entry.live_card().is_some()).count(), 1);
    // A live update of a card that isn't here is nothing.
    let mut other = stepped("running", 5, true);
    if let Event::Custom { data, .. } = &mut other.event {
        data["card"]["id"] = "monorepo/8".into();
    }
    feed(&mut chat, &[&posted(&other)]);
    assert_eq!(landing_cards(&chat).len(), 1);
    // How it ends is said in history, and is in the trail.
    feed(&mut chat, &[&posted(&stepped("passed", 214, false))]);
    let cards = landing_cards(&chat);
    assert_eq!(cards[0].updates.len(), 1);
    assert_eq!(
        cards[0].card.as_ref().unwrap().state,
        zirc_proto::card::State::Passed
    );
}

#[test]
fn a_cards_live_updates_are_never_a_conversations_preview() {
    // A running landing's steps come every few seconds: were each the preview, it would
    // change all the time, and between landings when several run.
    let mut chat = chat();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            &posted(&stepped("running", 0, false)),
            ":alice!u@h PRIVMSG #api :lunch?",
        ],
    );
    let preview = |chat: &Chat| chat.preview("#api").unwrap().text;
    let before = preview(&chat);
    assert_eq!(before, "alice: lunch?");
    feed(&mut chat, &[&posted(&stepped("running", 40, true))]);
    assert_eq!(preview(&chat), before);
}

fn says(session: &str, name: &str, body: &str) -> Message {
    let mut message = Message::new(session, Event::Text { body: body.into() });
    message.name = Some(name.into());
    message
}

/// Two agents talking in #api, then one of them telling a person, then someone saying
/// something and an agent speaking to us.
fn agents_talking() -> Chat {
    let mut chat = Scene::in_api();
    let lines = [
        ":s 353 me = #api :me alice alice-mbp bob-mbp".to_owned(),
        ":s 366 me #api :End".to_owned(),
        format!(
            "@time=2026-10-30T11:40:00.000Z {}",
            zirc(
                "alice-mbp",
                &says(
                    "cc-1",
                    "zirc-chat",
                    "cc-5d2b10: can you move **is_machine** into zirc-proto?\nIt's needed by both."
                )
            )
        ),
        format!(
            "@time=2026-10-30T11:41:00.000Z {}",
            zirc("bob-mbp", &says("cc-2", "controller", "yes, after 606"))
        ),
        format!(
            "@time=2026-10-30T11:42:00.000Z {}",
            zirc("alice-mbp", &says("cc-1", "zirc-chat", "thanks"))
        ),
        format!(
            "@time=2026-10-30T11:43:00.000Z {}",
            zirc("alice-mbp", &says("cc-1", "zirc-chat", "alice: it's moved"))
        ),
        format!(
            "@time=2026-10-30T11:44:00.000Z {}",
            zirc("bob-mbp", &says("cc-2", "controller", "landed"))
        ),
        "@time=2026-10-30T11:45:00.000Z :alice!u@h PRIVMSG #api :lunch?".to_owned(),
        format!(
            "@time=2026-10-30T11:46:00.000Z {}",
            zirc(
                "bob-mbp",
                &says("cc-2", "controller", "me: your build is green")
            )
        ),
    ];
    feed_all(&mut chat, &lines);
    chat.into_chat()
}

#[test]
fn agents_talking_to_each_other_stay_rows_of_their_own() {
    let chat = agents_talking();
    let rows = chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive());
    let posts: Vec<&str> = rows
        .iter()
        .filter_map(|row| match row {
            Item::Event { parts, .. } => Some(parts.actor.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        posts,
        [
            "zirc-chat",
            "controller",
            "zirc-chat",
            "zirc-chat",
            "controller",
            "controller"
        ]
    );
}

#[test]
fn people_only_keeps_people_and_whatever_mentions_me() {
    let chat = agents_talking();
    let rows =
        people_only(chat.items(chat.conversation("#api").unwrap(), |time| time.date_naive()));
    assert!(matches!(rows[0], Item::Day(_)));
    assert!(matches!(&rows[1], Item::Message { text, .. } if *text == "lunch?"));
    assert!(
        matches!(&rows[2], Item::Event { text, .. } if text.ends_with("me: your build is green"))
    );
    assert_eq!(rows.len(), 3, "{rows:#?}");
}

#[test]
fn offers_people_then_agent_sessions_then_apps() {
    let mut chat = Scene::in_api();
    let mut post = Message::new(
        "cc-598aa1",
        Event::Text {
            body: "on it".into(),
        },
    );
    post.name = Some("zirc-chat".into());
    let lines = [
        ":me!u@h JOIN #api".to_owned(),
        ":s 353 me = #api :me ada zed bob alice-mbp zirc-alerts".to_owned(),
        ":s 366 me #api :End".to_owned(),
        zirc("alice-mbp", &post),
    ];
    feed_all(&mut chat, &lines);
    let profiles: zirc_proto::profile::Profiles =
        serde_json::from_str(r#"{"bob": {"name": "Ann Zimmer", "avatar": null}}"#).unwrap();
    chat.take_profiles(&profiles, "https://zirc.dev");
    let all = chat.mentions("", 10);
    let kinds: Vec<(MentionKind, &str)> = all.iter().map(|m| (m.kind, m.token.as_str())).collect();
    // People, then the agent session by its id (never its machine), then the app.
    assert_eq!(
        kinds,
        [
            (MentionKind::Person, "ada"),
            (MentionKind::Person, "bob"),
            (MentionKind::Person, "zed"),
            (MentionKind::Agent, "cc-598aa1"),
            (MentionKind::App, "zirc-alerts"),
        ]
    );
    let agent = &all[3];
    assert_eq!(
        (
            agent.name.as_str(),
            agent.detail.as_deref(),
            agent.nick.as_str()
        ),
        ("zirc-chat", Some("alice's agent"), "alice-mbp")
    );
    assert!(matches!(agent.avatar, Avatar::Agent { .. }));
    assert_eq!(all[4].name, "Alerts");
    assert!(matches!(all[4].avatar, Avatar::App { .. }));
    // A start before a word, within each kind: zed, then Ann Zimmer.
    let tokens: Vec<String> = chat
        .mentions("z", 10)
        .into_iter()
        .map(|m| m.token)
        .collect();
    assert_eq!(tokens, ["zed", "bob", "cc-598aa1", "zirc-alerts"]);
}

#[test]
fn finds_the_mention_being_typed_at_the_cursor() {
    assert_eq!(mention_query("hi @ad", 6), Some((3, "ad")));
    assert_eq!(mention_query("hi @ad there", 6), Some((3, "ad")));
    assert_eq!(mention_query("@adam", 3), Some((0, "ad")));
    assert_eq!(mention_query("@", 1), Some((0, "")));
    assert_eq!(mention_query("hi @ad there", 12), None);
    assert_eq!(mention_query("mail me@ad", 10), None);
    assert_eq!(mention_query("hi ad", 5), None);
    // A cursor past the end, or inside a character, is moved back.
    assert_eq!(mention_query("é @a", 99), Some((3, "a")));
}

#[test]
fn completes_a_mention_and_keeps_what_follows() {
    let ada = Mention {
        token: "ada".into(),
        nick: "ada".into(),
        name: "Ada".into(),
        detail: None,
        kind: MentionKind::Person,
        avatar: Avatar::Person { nick: "ada".into() },
    };
    // At the end: a space after it, and what's typed next follows the space.
    let (text, cursor) = complete_mention("hi @ad", 6, &ada);
    assert_eq!((text.as_str(), cursor), ("hi @ada ", 8));
    let mut typed = text.clone();
    typed.insert(cursor, 'x');
    assert_eq!(typed, "hi @ada x");
    // Mid-text: the space already there is used, and the rest kept.
    let (text, cursor) = complete_mention("hi @ad there", 6, &ada);
    assert_eq!((text.as_str(), cursor), ("hi @ada there", 8));
    // Inside a word: all of it is replaced.
    let (text, cursor) = complete_mention("@adxyz, ok", 3, &ada);
    assert_eq!((text.as_str(), cursor), ("@ada , ok", 5));
    // No mention at the cursor: nothing changes.
    assert_eq!(complete_mention("hi", 2, &ada), ("hi".to_owned(), 2));
}

#[test]
fn says_which_rows_mention_me() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":s 353 me = #api :me ada",
            ":s 366 me #api :End",
            ":ada!u@h PRIVMSG #api :me: can you look?",
            ":ada!u@h PRIVMSG #api :anyone?",
        ],
    );
    let api = chat.conversation("#api").unwrap();
    let rows = chat.items(api, |time| time.date_naive());
    let marked: Vec<bool> = rows.iter().map(|row| row.mentions_me(api)).collect();
    assert_eq!(marked, [false, true, false]);
    // In a direct message every line is for us, so none stands out.
    feed(&mut chat, &[":ada!u@h PRIVMSG me :me: lunch?"]);
    let ada = chat.conversation("ada").unwrap();
    let rows = chat.items(ada, |time| time.date_naive());
    assert!(rows.iter().all(|row| !row.mentions_me(ada)));
}

#[test]
fn shows_a_cards_buttons_to_whom_the_card_says() {
    let mut chat = Scene::in_api();
    // One of our machines speaks as cc-123abc; bob's as cc-999fff.
    let said = |session: &str| Message::new(session, Event::Text { body: "hi".into() });
    feed_all(
        &mut chat,
        &[
            ":s 353 me = #api :me me-mbp bob-mbp".to_owned(),
            ":s 366 me #api :End".to_owned(),
            zirc("me-mbp", &said("cc-123abc")),
            zirc("bob-mbp", &said("cc-999fff")),
        ],
    );
    let card = |about: &str| -> zirc_proto::card::Card {
        serde_json::from_value(serde_json::json!({
            "id": "c1",
            "title": "build 1",
            "about": about,
            "actions": [
                { "id": "anyone", "label": "Anyone", "who": "anyone" },
                { "id": "members", "label": "Members", "who": "members" },
                { "id": "about", "label": "About", "who": "about" },
                { "id": "admins", "label": "Admins", "who": "admins" },
                { "id": "owners", "label": "Owners", "who": "owners" },
            ],
        }))
        .unwrap()
    };
    let shown = |chat: &Chat, card: &zirc_proto::card::Card| -> Vec<String> {
        card.actions
            .iter()
            .filter(|action| chat.shows_action(card, action))
            .map(|action| action.id.clone())
            .collect()
    };
    // A member, about our own session: everything but the admins' and owners'.
    chat.set_role("member");
    assert_eq!(
        shown(&chat, &card("cc-123abc")),
        ["anyone", "members", "about"]
    );
    // About bob's session, or bob: not the about button.
    assert_eq!(shown(&chat, &card("cc-999fff")), ["anyone", "members"]);
    assert_eq!(shown(&chat, &card("bob")), ["anyone", "members"]);
    // About us by nick, or one of our machines.
    assert_eq!(shown(&chat, &card("me")), ["anyone", "members", "about"]);
    assert_eq!(
        shown(&chat, &card("me-mbp")),
        ["anyone", "members", "about"]
    );
    // An admin isn't an owner.
    chat.set_role("admin");
    assert_eq!(shown(&chat, &card("bob")), ["anyone", "members", "admins"]);
    chat.set_role("owner");
    assert_eq!(
        shown(&chat, &card("bob")),
        ["anyone", "members", "admins", "owners"]
    );
}

#[test]
fn says_a_cards_times_with_the_day_only_before_today() {
    let now = noon().naive_utc();
    assert_eq!(card_time(now - chrono::Duration::hours(2), now), "10:00");
    let before = now - chrono::Duration::days(3);
    assert_eq!(
        card_time(before, now),
        before.format("%a 12:00").to_string()
    );
}

#[test]
fn says_spans_and_ago_in_one_way() {
    assert_eq!(span(40), "40s");
    assert_eq!(span(125), "2m");
    assert_eq!(span(3 * 3600 + 5), "3h");
    assert_eq!(span(2 * 86_400), "2d");
    assert_eq!(ago(5), "just now");
    assert_eq!(ago(125), "2m ago");
    assert_eq!(ago(2 * 86_400), "2d ago");
}

#[test]
fn names_a_conversation_shortly_for_the_composer() {
    let mut chat = Scene::in_api();
    let mut named = Message::new("cc-598aa1", Event::Text { body: "hi".into() });
    named.name = Some("zirc-chat".into());
    feed_all(
        &mut chat,
        &[
            ":s 353 me = #api :me ada alice-mbp".to_owned(),
            ":s 366 me #api :End".to_owned(),
            ":s 354 me 42 h ada G ada :Ada Lovelace".to_owned(),
            zirc("alice-mbp", &named),
        ],
    );
    assert_eq!(chat.short_title("#api"), "#api");
    // An agent session by what it goes by, never its machine; one unnamed by its id.
    assert_eq!(chat.short_title("alice-mbp/cc-598aa1"), "zirc-chat");
    assert_eq!(chat.short_title("alice-mbp/cc-ad4f7c"), "cc-ad4f7c");
    // A person by their name.
    assert_eq!(chat.short_title("ada"), "Ada Lovelace");
    // Under an agent session's name, whose agent it is; nothing under anyone else's.
    assert_eq!(
        chat.subtitle("alice-mbp/cc-598aa1").as_deref(),
        Some("alice's agent")
    );
    assert_eq!(chat.subtitle("#api"), None);
    assert_eq!(chat.subtitle("ada"), None);
}

#[test]
fn lists_a_channels_threads_newest_first_with_what_is_new() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #api/fix-login",
            ":me!u@h JOIN #api/login-times-out",
            ":me!u@h JOIN #web",
            "@time=2026-10-30T11:00:00.000Z;msgid=f1 :bob!u@h PRIVMSG #api/fix-login :pushed",
            "@time=2026-10-30T11:30:00.000Z;msgid=t1 :bob!u@h PRIVMSG #api/login-times-out :me: Safari?",
        ],
    );
    let threads: Vec<(String, String, usize)> = chat
        .threads("#API")
        .into_iter()
        .map(|thread| (thread.name, thread.title, thread.badge))
        .collect();
    // Newest first, titled without the "/", each counting what mentions us.
    assert_eq!(
        threads,
        [
            ("#api/login-times-out".into(), "login times out".into(), 1),
            ("#api/fix-login".into(), "fix login".into(), 0),
        ]
    );
    assert_eq!(
        chat.threads("#api")[1]
            .under
            .as_ref()
            .map(|under| under.text.as_str()),
        Some("bob: pushed")
    );
    assert!(chat.threads("#web").is_empty());
    // Bold by every row's rule: something unread, and not muted.
    assert!(chat.threads("#api").iter().all(|thread| thread.bold));
    chat.mute("#api/fix-login", Some(Muted::Off));
    let bold: Vec<bool> = chat.threads("#api").iter().map(|t| t.bold).collect();
    assert_eq!(bold, [true, false]);
}

#[test]
fn says_where_a_conversation_starts_until_something_is_said() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #design",
            ":s 332 me #design :Colours and type",
            ":me!u@h JOIN #web",
        ],
    );
    assert_eq!(
        chat.intro("#design").unwrap(),
        ["This is the start of #design.", "Colours and type"]
    );
    assert_eq!(chat.intro("#web").unwrap(), ["This is the start of #web."]);
    feed(&mut chat, &[":bob!u@h PRIVMSG #web :hi"]);
    assert_eq!(chat.intro("#web"), None);
    // The server's own conversation has none, however quiet.
    let quiet = chat_with(Options::default());
    assert!(
        quiet
            .conversation(quiet.server())
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(quiet.intro(quiet.server()), None);
}

#[test]
fn labels_a_room_by_its_own_name_under_its_channel() {
    assert_eq!(room_label("#zirc/fix-login").as_deref(), Some("fix login"));
    assert_eq!(room_parts("#zirc/fix-login"), Some(("#zirc", "fix-login")));
    assert_eq!(room_label("#zirc"), None);
    assert_eq!(room_label("#zirc/"), None);
}

#[test]
fn draws_a_mention_by_the_name_they_show_as_keeping_the_nick() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":alice!u@h JOIN #api alice :Alice Liddell",
        ],
    );
    let drawn = chat.drawn(rich("thanks @alice and @bob!"));
    let Block::Paragraph(spans) = &drawn[0] else {
        panic!("{drawn:?}");
    };
    let shown: Vec<(&str, &SpanKind)> = spans.iter().map(|s| (s.text.as_str(), &s.kind)).collect();
    // By the name they show as, never with an "@"; the nick stays the span's, for its link.
    assert!(
        shown.contains(&("Alice Liddell", &SpanKind::Mention("alice".into()))),
        "{shown:?}"
    );
    assert!(
        shown.contains(&("bob", &SpanKind::Mention("bob".into()))),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|(text, _)| text.contains('@')),
        "{shown:?}"
    );
}

#[test]
fn says_a_message_plainly_with_mentions_by_their_names() {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":alice!u@h JOIN #api alice :Alice Liddell",
        ],
    );
    // As Activity's excerpt: no "@", no formatting, each mention as they show.
    assert_eq!(
        chat.shown_text("@alice the **plan** is up"),
        "Alice Liddell the plan is up"
    );
    assert_eq!(chat.shown_text("hi @bob"), "hi bob");
}

#[test]
fn draws_sessions_by_name_and_bare_links_by_label() {
    let mut chat = Scene::in_api();
    let mut named = Message::new("cc-598aa1", Event::Text { body: "hi".into() });
    named.name = Some("zirc-chat".into());
    feed_all(
        &mut chat,
        &[
            ":s 353 me = #api :me alice-mbp".to_owned(),
            ":s 366 me #api :End".to_owned(),
            zirc("alice-mbp", &named),
        ],
    );
    let drawn = chat.drawn(rich(
        "ask @cc-598aa1, see https://github.com/zircapp/monorepo/pull/12 or [the PR](https://x.test/1)",
    ));
    let Block::Paragraph(spans) = &drawn[0] else {
        panic!("{drawn:?}");
    };
    let shown: Vec<(&str, &SpanKind)> = spans.iter().map(|s| (s.text.as_str(), &s.kind)).collect();
    // A session by its name, plain, still a session; a bare link short, still its address;
    // a link with words of its own as written.
    assert!(
        shown.contains(&("zirc-chat", &SpanKind::Session("cc-598aa1".into()))),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|(text, kind)| *text
            == link_label("https://github.com/zircapp/monorepo/pull/12")
            && **kind == SpanKind::Link("https://github.com/zircapp/monorepo/pull/12".into())),
        "{shown:?}"
    );
    assert!(shown.iter().any(|(text, _)| *text == "the PR"), "{shown:?}");
}

/// A word naming a session we know is its chip by its name, whatever its id's shape, in
/// a message's words and so in a preview's and in CI's (the user's choice, 26 Nov 2026);
/// a word naming none stays a word.
#[test]
fn draws_a_known_sessions_bare_id_as_its_chip() {
    let mut chat = Scene::in_api();
    let mut named = Message::new("codex-7f2a", Event::Text { body: "hi".into() });
    named.name = Some("rate-limits".into());
    feed_all(
        &mut chat,
        &[
            ":s 353 me = #api :me alice-mbp".to_owned(),
            ":s 366 me #api :End".to_owned(),
            zirc("alice-mbp", &named),
        ],
    );
    let drawn = chat.drawn(rich(
        "CI: codex-7f2a: build 419 failed, (codex-7f2a) not codex-7f2b",
    ));
    let Block::Paragraph(spans) = &drawn[0] else {
        panic!("{drawn:?}");
    };
    let shown: Vec<(&str, &SpanKind)> = spans.iter().map(|s| (s.text.as_str(), &s.kind)).collect();
    let session = SpanKind::Session("codex-7f2a".into());
    assert_eq!(
        shown,
        [
            ("CI: ", &SpanKind::Text),
            ("rate-limits", &session),
            (": build 419 failed, (", &SpanKind::Text),
            ("rate-limits", &session),
            (") not codex-7f2b", &SpanKind::Text),
        ]
    );
    assert_eq!(
        chat.shown_text("CI: codex-7f2a: build 419 failed"),
        "CI: rate-limits: build 419 failed"
    );
}

#[test]
fn links_build_n_in_any_ci_line_to_its_page() {
    let deploying = ci_post(
        zirc_proto::ci::DEPLOY_KIND,
        serde_json::json!({ "build": "977", "url": "https://buildkite.test/builds/977" }),
        "cc-1: deploying to production: web, build 977",
    );
    let mut chat = chat();
    feed(
        &mut chat,
        &[":me!u@h JOIN #api", &zirc("zirc-alerts", &deploying)],
    );
    let conversation = chat.conversation("#api").unwrap();
    let landing = chat
        .items(conversation, |time| time.date_naive())
        .into_iter()
        .find_map(|item| match item {
            Item::Landing(landing) => Some(landing),
            _ => None,
        })
        .expect("a landing row");
    assert!(landing.links().contains(&(
        "build 977".to_owned(),
        "https://buildkite.test/builds/977".to_owned()
    )));
}

#[test]
fn counts_each_tab_as_badge_counts_each_conversation() {
    let mut chat = Scene::in_api();
    feed(&mut chat, &[":me!u@h JOIN #api", ":me!u@h JOIN #web"]);
    let at = chat.position("#web").unwrap();
    let _ = chat.select(at);
    feed(
        &mut chat,
        &[
            ":bob!u@h PRIVMSG #api :lunch?",
            ":bob!u@h PRIVMSG #api :me: a look?",
            ":bob!u@h PRIVMSG me :psst",
            ":bob!u@h PRIVMSG me :and again",
            ":carol!u@h PRIVMSG me :hi",
        ],
    );
    // A channel counts what mentions us; a direct message everything unread.
    let badges = chat.badges();
    assert_eq!((badges.channels, badges.direct), (1, 3));
    assert_eq!(usize::try_from(badges.total), Ok(chat.badge_total()));
    // A direct message set to notify nothing counts nothing, on its tab and its row too.
    chat.set_level("bob", Level::Nothing);
    assert_eq!(chat.badges().direct, 1);
    let carol = chat
        .direct_rows()
        .into_iter()
        .find(|row| row.name == "carol")
        .unwrap();
    assert_eq!(carol.badge, 1);
}

#[test]
fn previews_a_conversations_last_words_as_discord_does() {
    let mut chat = Scene::in_api();
    let photo = |name: &str| {
        Message::new(
            "",
            Event::File(zirc_proto::File {
                url: format!("https://zirc.dev/m/acme/{name}"),
                name: name.into(),
                mime: "image/png".into(),
                size: 10,
                width: None,
                height: None,
                duration_ms: None,
            }),
        )
    };
    let at = |time: &str, line: String| format!("@time=2026-10-30T{time}.000Z {line}");
    feed_all(
        &mut chat,
        &[
            ":me!u@h JOIN #api".to_owned(),
            at(
                "11:00:00",
                ":bob!u@h PRIVMSG me :**yeah** exactly\nand more".to_owned(),
            ),
            at("11:01:00", ":me!u@h PRIVMSG #api :quick sync?".to_owned()),
            at(
                "11:50:00",
                zirc("bob", &photo("a.png")).replace("#api", "me"),
            ),
            at(
                "11:51:00",
                zirc("bob", &photo("b.png")).replace("#api", "me"),
            ),
        ],
    );
    // Our own, as You; a direct message's other person unnamed, as its title names them;
    // files counted.
    assert_eq!(chat.preview("#api").unwrap().text, "You: quick sync?");
    assert_eq!(chat.preview("bob").unwrap().text, "2 images");
    // In a channel, whoever said it is named.
    feed(&mut chat, &[":bob!u@h PRIVMSG #api :sounds good"]);
    assert_eq!(chat.preview("#api").unwrap().text, "bob: sounds good");
    // Words as Activity's excerpt reads them: Markdown's marks gone, a mention by its name.
    feed(
        &mut chat,
        &[":bob!u@h PRIVMSG #api :- the **plan** is up, @carol"],
    );
    assert_eq!(
        chat.preview("#api").unwrap().text,
        "bob: the plan is up, carol"
    );
    // A list's time, from the chat's clock (noon).
    let noon = noon();
    assert_eq!(ago_short(noon, noon), "now");
    assert_eq!(ago_short(noon - chrono::Duration::minutes(5), noon), "5m");
    assert_eq!(ago_short(noon - chrono::Duration::hours(3), noon), "3h");
    assert_eq!(ago_short(noon - chrono::Duration::days(9), noon), "9d");
    assert_eq!(ago_short(noon - chrono::Duration::days(40), noon), "1 mo");
    assert_eq!(ago_short(noon - chrono::Duration::days(400), noon), "1 yr");
    // The direct messages' row previews it, and says when.
    let bob = chat
        .direct_rows()
        .into_iter()
        .find(|row| row.name == "bob")
        .unwrap();
    assert_eq!(
        (bob.preview.as_deref(), bob.when.as_deref()),
        (Some("2 images"), Some("9m"))
    );
}

/// `count` numbered lines, one to a line: `line 1`, `line 2`…
fn numbered(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_short_message_shows_whole_with_no_control() {
    let chat = chat();
    let text = format!("{}\n\n```\n{}\n```", numbered(6), numbered(8));
    let shown = chat.body("m1", rich(&text), false);
    assert_eq!(shown.blocks, rich(&text));
    assert_eq!(shown.more, None);
}

#[test]
fn a_long_code_block_shows_its_first_eight_lines_whole_for_highlighting() {
    let mut chat = chat();
    let text = format!("The diff:\n\n```rust\n{}\n```\n\nAfter it.", numbered(40));
    let shown = chat.body("m1", rich(&text), false);
    // The words before it, then the block whole, of which 8 lines show; what follows is cut.
    assert_eq!(shown.blocks, rich(&text)[..2]);
    let more = shown.more.unwrap();
    assert_eq!(
        more,
        More {
            hidden: 32 + 1,
            open: false,
            code_lines: Some(8),
            updates: false,
            steps: None,
        }
    );
    assert_eq!(more.label(), "Show 33 more lines");
    assert_eq!(
        more.spoken("Henrik Sandberg's agent"),
        "Show 33 more lines of Henrik Sandberg's agent's message"
    );
    assert_eq!(more.terminal_line(), "… 33 more lines · Enter shows them");
    // Opened, it's all there, and says "Show less" at its end.
    chat.toggle_more("m1", more.open);
    let open = chat.body("m1", rich(&text), false);
    assert_eq!(open.blocks, rich(&text));
    let more = open.more.unwrap();
    assert_eq!((more.open, more.code_lines), (true, None));
    assert_eq!(more.label(), "Show less");
    assert_eq!(more.terminal_line(), "· Enter shows less");
    // Closed again, it's cut as before; another message is untouched.
    chat.toggle_more("m1", more.open);
    assert!(!chat.body("m1", rich(&text), false).more.unwrap().open);
}

#[test]
fn long_words_show_their_summary_and_first_bullets_up_to_six_lines() {
    let chat = chat();
    let bullets: Vec<String> = (1..=20).map(|n| format!("- point {n}")).collect();
    let text = format!("Landed the change.\n\n{}", bullets.join("\n"));
    let shown = chat.body("m1", rich(&text), false);
    let Block::List { items, .. } = &shown.blocks[1] else {
        panic!("{:?}", shown.blocks);
    };
    // The summary, then five bullets: six lines. The other fifteen are hidden.
    assert_eq!(shown.blocks.len(), 2);
    assert_eq!(items.len(), 5);
    let more = shown.more.unwrap();
    assert_eq!((more.hidden, more.code_lines), (15, None));
    // A paragraph is cut at a line, never mid-line.
    let shown = chat.body("m2", rich(&numbered(9)), false);
    let Block::Paragraph(spans) = &shown.blocks[0] else {
        panic!("{:?}", shown.blocks);
    };
    let words: String = spans.iter().map(|span| span.text.as_str()).collect();
    assert_eq!(words, numbered(6));
    assert_eq!(shown.more.unwrap().label(), "Show 3 more lines");
}

#[test]
fn a_message_long_both_ways_has_one_control_at_its_first_cut() {
    let chat = chat();
    let text = format!(
        "{}\n\n```\n{}\n```\n\n{}",
        numbered(3),
        numbered(12),
        numbered(10)
    );
    let shown = chat.body("m1", rich(&text), false);
    // Three lines of words, the block's first 8, and the rest counted, code and words.
    assert_eq!(shown.blocks.len(), 2);
    let more = shown.more.unwrap();
    assert_eq!((more.hidden, more.code_lines), (4 + 10, Some(8)));
    // Words before a block that use up the six lines cut ahead of it.
    let text = format!("{}\n\n```\n{}\n```", numbered(7), numbered(3));
    let more = chat.body("m2", rich(&text), false).more.unwrap();
    assert_eq!((more.hidden, more.code_lines), (1 + 3, None));
}

#[test]
fn a_long_message_that_mentions_us_or_is_opened_from_search_starts_open() {
    let mut chat = chat();
    let text = numbered(30);
    assert!(chat.body("m1", rich(&text), true).more.unwrap().open);
    // The person can still cut it.
    chat.toggle_more("m1", true);
    assert!(!chat.body("m1", rich(&text), true).more.unwrap().open);
    chat.open_more("m2");
    assert!(chat.body("m2", rich(&text), false).more.unwrap().open);
    assert!(!chat.body("m3", rich(&text), false).more.unwrap().open);
}

#[test]
fn says_away_after_a_name_and_on_a_line_of_its_own() {
    assert_eq!(away_label(""), "away");
    assert_eq!(away_label("lunch"), "away: lunch");
    assert_eq!(away_title(""), "Away");
    assert_eq!(away_title("lunch"), "Away: lunch");
    // The server's own message for an account with no app connected is offline, not away.
    assert_eq!(away_label(OFFLINE_AWAY), "offline");
    assert_eq!(away_title(OFFLINE_AWAY), "Offline");
    assert_eq!(Standing::of(true, Some(OFFLINE_AWAY)), Standing::Offline);
    assert_eq!(Standing::Offline.presence(), Presence::Gone);
}

#[test]
fn blocking_hides_what_they_said_and_unblocking_brings_it_back() {
    let mut chat = bare();
    connected(&mut chat);
    feed(
        &mut chat,
        &[
            "@msgid=a;time=2026-10-30T11:00:00.000Z :mal!u@h PRIVMSG #api :hello",
            "@msgid=b;time=2026-10-30T11:01:00.000Z :bob!u@h PRIVMSG #api :hi",
        ],
    );
    let at = chat.position("#api").unwrap();
    let from = |chat: &Chat, who: &str| {
        chat.conversations()[at]
            .entries
            .iter()
            .filter(|e| e.from == who)
            .count()
    };
    assert_eq!((from(&chat, "mal"), from(&chat, "bob")), (1, 1));
    chat.set_blocked(["Mal"]);
    assert!(chat.is_blocked("mal") && !chat.is_blocked("bob"));
    assert_eq!((from(&chat, "mal"), from(&chat, "bob")), (0, 1));
    // What they say next is kept out of sight too.
    feed(
        &mut chat,
        &["@msgid=c;time=2026-10-30T11:02:00.000Z :mal!u@h PRIVMSG #api :again"],
    );
    assert_eq!(from(&chat, "mal"), 0);
    chat.set_blocked(Vec::<String>::new());
    assert_eq!(from(&chat, "mal"), 2);
    assert!(chat.blocked().is_empty());
}

#[test]
fn only_someone_elses_words_and_someone_else_can_be_reported() {
    let mut chat = bare();
    connected(&mut chat);
    feed(
        &mut chat,
        &[
            "@msgid=a;time=2026-10-30T11:00:00.000Z :mal!u@h PRIVMSG #api :hello",
            "@msgid=b;time=2026-10-30T11:01:00.000Z :me!u@h PRIVMSG #api :hi",
            "@msgid=c;time=2026-10-30T11:02:00.000Z :mal-laptop!u@machine/mal PRIVMSG #api :an agent",
            "@msgid=d;time=2026-10-30T11:03:00.000Z :me-laptop!u@machine/me PRIVMSG #api :my agent",
        ],
    );
    assert!(chat.can_report("c") && !chat.can_report("d"));
    assert_eq!(
        chat.message_origin("a"),
        Some(("#api".into(), "mal".into()))
    );
    assert!(chat.can_report("a"));
    assert!(!chat.can_report("b") && !chat.can_report("nope"));
    assert!(chat.can_report_person("Mal"));
    assert!(!chat.can_report_person("ME") && !chat.can_report_person(" "));
}

#[test]
fn a_report_carries_ids_never_text_and_refuses_what_cant_be_reported() {
    use zirc_proto::report::{Kind, Reason, Report};
    let mut chat = bare();
    connected(&mut chat);
    feed(
        &mut chat,
        &[
            "@msgid=a;time=2026-10-30T11:00:00.000Z :mal!u@h PRIVMSG #api :something nasty",
            "@msgid=b;time=2026-10-30T11:01:00.000Z :me!u@h PRIVMSG #api :hi",
        ],
    );
    let what = Reported::Message("a".into());
    assert_eq!(chat.report_title(&what), "Report a message from mal");
    let report = chat
        .report(&what, Some(Reason::Harassment), "  again  ")
        .unwrap();
    assert_eq!(
        report,
        Report {
            kind: Kind::Message,
            reason: Reason::Harassment,
            account: "mal".into(),
            channel: Some("#api".into()),
            message: Some("a".into()),
            note: Some("again".into()),
        }
    );
    assert!(!serde_json::to_string(&report).unwrap().contains("nasty"));
    let person = Reported::Person("mal".into());
    assert_eq!(chat.report_title(&person), "Report mal");
    let report = chat.report(&person, Some(Reason::Spam), "").unwrap();
    assert_eq!((report.kind, report.note), (Kind::Person, None));
    assert_eq!(chat.report(&person, None, ""), Err(REPORT_NO_REASON.into()));
    assert!(
        chat.report(&person, Some(Reason::Other), &"x".repeat(501))
            .is_err()
    );
    let gone = Reported::Message("z".into());
    assert_eq!(
        chat.report(&gone, Some(Reason::Other), ""),
        Err(REPORT_GONE.into())
    );
    let ours = Reported::Message("b".into());
    assert_eq!(
        chat.report(&ours, Some(Reason::Other), ""),
        Err(REPORT_NOT_REPORTABLE.into())
    );
    let me = Reported::Person("me".into());
    assert_eq!(
        chat.report(&me, Some(Reason::Other), ""),
        Err(REPORT_SELF.into())
    );
}

#[test]
fn a_blocked_person_adds_nothing_unread_notifies_nobody_and_has_no_row() {
    let mut chat = bare();
    connected(&mut chat);
    chat.set_blocked(["mal"]);
    feed(
        &mut chat,
        &["@msgid=d;time=2026-10-30T11:00:00.000Z :mal!u@h PRIVMSG me :psst"],
    );
    let at = chat.position("mal").unwrap();
    assert_eq!(chat.conversations()[at].unread, 0);
    assert!(chat.direct_rows().iter().all(|row| row.name != "mal"));
    assert!(chat.take_fresh().is_empty());
    // Blocking someone who's already said something takes it out of the unread counts.
    let mut chat = bare();
    connected(&mut chat);
    feed(
        &mut chat,
        &["@msgid=e;time=2026-10-30T11:00:00.000Z :mal!u@h PRIVMSG me :hello"],
    );
    let at = chat.position("mal").unwrap();
    assert_eq!(chat.conversations()[at].unread, 1);
    chat.set_blocked(["mal"]);
    assert_eq!(chat.conversations()[at].unread, 0);
    assert!(chat.direct_rows().iter().all(|row| row.name != "mal"));
}

#[test]
fn the_server_s_own_conversation_shown_says_to_join_a_channel() {
    let mut chat = marking();
    assert_eq!(chat.nothing_joined(), Some(crate::chat::NOTHING_JOINED));
    // Sent as it is, the words every app says.
    assert_eq!(
        serde_json::to_value(crate::chat::NOTHING_JOINED).unwrap(),
        serde_json::json!({
            "title": "You're not in any channel yet",
            "body": "Join one to start talking with your team.",
            "action": "Join a channel",
        })
    );
    assert_eq!(chat.pick_a_conversation(), None);
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    // Joined, with none open yet: pick one.
    assert_eq!(chat.nothing_joined(), None);
    assert_eq!(
        chat.pick_a_conversation(),
        Some(crate::chat::PICK_A_CONVERSATION)
    );
    let _ = chat.select(chat.position("#api").unwrap());
    assert_eq!(chat.nothing_joined(), None);
    assert_eq!(chat.pick_a_conversation(), None);
}

#[test]
fn a_conversations_threshold_hides_what_asks_less_but_never_what_names_us() {
    let mut chat = chat();
    connected(&mut chat);
    let warned = Message {
        level: Some(zirc_proto::Level::Warn),
        ..Message::new(
            "ci",
            Event::Text {
                body: "master is ahead of e2e".into(),
            },
        )
    };
    feed_all(
        &mut chat,
        &[
            ":bob!u@h PRIVMSG #api :lunch?".to_owned(),
            zirc("zirc-alerts", &warned),
            ":bob!u@h PRIVMSG #api :me: look at this".to_owned(),
        ],
    );
    let shown = |chat: &Chat| -> Vec<String> {
        let api = api(chat);
        api.entries
            .iter()
            .filter(|e| api.shows(e))
            .map(entry_text)
            .collect()
    };
    assert_eq!(chat.threshold("#api"), zirc_proto::Level::Info);
    assert_eq!(shown(&chat).len(), 3);
    let before = api(&chat).unread;
    chat.set_threshold("#api", zirc_proto::Level::Warn);
    // The chatter goes; the warning and the line that names us stay.
    assert_eq!(shown(&chat).len(), 2, "{:?}", shown(&chat));
    assert!(shown(&chat).iter().all(|line| !line.contains("lunch")));
    assert!(api(&chat).unread <= before);
    assert_eq!(
        chat.thresholds().into_iter().collect::<Vec<_>>(),
        [("#api".to_owned(), zirc_proto::Level::Warn)]
    );
    // The "Show" menu ticks it, and every app is sent the level as the menu sets it.
    let choices = chat.show_choices("#api");
    let ticked = choices.iter().find(|choice| choice.checked).unwrap();
    assert_eq!(
        serde_json::to_value(ticked).unwrap(),
        serde_json::json!({"level": "warn", "label": "Warnings", "checked": true})
    );
    // Errors only: the warning goes too, never the mention.
    chat.set_threshold("#API", zirc_proto::Level::Error);
    assert_eq!(shown(&chat).len(), 1);
    // As an app puts back what it kept; back to the default, nothing's kept.
    chat.set_thresholds([("#api".to_owned(), zirc_proto::Level::Info)]);
    assert_eq!(shown(&chat).len(), 3);
    assert!(chat.thresholds().is_empty());
    // #ops shows warn and up until the person picks otherwise, before it's even open.
    assert_eq!(chat.threshold("#ops"), zirc_proto::Level::Warn);
    chat.set_threshold("#ops", zirc_proto::Level::Info);
    feed(&mut chat, &[":me!u@h JOIN #ops"]);
    assert_eq!(chat.threshold("#ops"), zirc_proto::Level::Info);
}

#[test]
fn everything_shows_whats_only_true_now_from_when_its_picked() {
    let mut chat = chat();
    connected(&mut chat);
    let entries = |chat: &Chat| api(chat).entries.len();
    feed_all(&mut chat, &[zirc("mbp", &working("cc-1", &["web/a.ts"]))]);
    assert_eq!(entries(&chat), 0);
    chat.set_threshold("#api", zirc_proto::Level::Debug);
    feed_all(&mut chat, &[zirc("mbp", &working("cc-1", &["web/b.ts"]))]);
    assert_eq!(entries(&chat), 1);
}

/// A decision posted by an older zirc, one plain line, is drawn in the house style as a
/// message: a title, its quote and bullets, with no empty line before them.
#[test]
fn a_decision_is_a_markdown_message_whichever_zirc_posted_it() {
    let mut chat = Scene::in_api();
    let decision = zirc_proto::decision::Decision {
        by: "mattias".into(),
        quote: "yes, both".into(),
        about: "the reinstall recipe".into(),
        session: Some("cc-1".into()),
        link: None,
        for_: vec![],
        replaces: None,
    };
    let Event::Custom { kind, data, .. } = decision.event() else {
        unreachable!()
    };
    let old = Message::new(
        "cc-1",
        Event::Custom {
            kind,
            data,
            text: "mattias decided about the reinstall recipe: \"yes, both\" (to cc-1)".into(),
        },
    );
    // Longer than one IRC line, so it travels as a multiline message: the body whole.
    let body = zirc_proto::encode_up_to(&old, 4096).unwrap();
    let mut line = zirc_line::Line::new("PRIVMSG", ["#api", body.as_str()]);
    line.source = Some("mattias-matt!u@h".to_owned());
    feed_all(&mut chat, &[line.to_string()]);
    let conversation = chat.conversation("#api").unwrap();
    let items = chat.items(conversation, |time| time.date_naive());
    let message = items
        .iter()
        .find_map(|item| match item {
            Item::Message { entry, text, .. } => Some((entry, *text)),
            _ => None,
        })
        .expect("a decision is a message");
    assert!(
        message
            .1
            .starts_with("**Decision:** the reinstall recipe\n> yes, both")
    );
    let blocks = message.0.rich();
    assert!(matches!(&blocks[0], Block::Paragraph(spans) if spans[0].bold));
    assert!(matches!(&blocks[1], Block::Quote(_)));
    assert!(matches!(&blocks[2], Block::List { .. }));
}

// The web client reads these three as JSON and the phones as UniFFI records of the same
// fields (apps/mobile/core/src/view.rs): a renamed or dropped field fails here first.
#[test]
fn the_card_the_note_and_the_header_are_the_wire_records() {
    use serde_json::json;
    let chat = alice_and_her_named_session();
    assert_eq!(
        serde_json::to_value(chat.agent_card("alice-mbp", "cc-1", noon())).unwrap(),
        json!({
            "session": "cc-1",
            "title": "the login bug",
            "titled": true,
            "handle": "analytics",
            "whose": "Alice's agent",
            "afterHandle": " · Alice's agent",
            "tool": "claude-code",
            "place": "Claude Code · alice-mbp",
            "standing": null,
            "now": "✎ about to change src/auth · 8m",
            "nowLead": "✎ about to change",
            "nowPaths": " src/auth",
            "nowAgo": " · 8m",
            "task": "refactor login",
            "heard": "Active now",
            "active": true,
            "mine": false,
            "messageLabel": "Message",
        })
    );
    assert_eq!(
        serde_json::to_value(super::agent_activity::ReadOnlyNote {
            text: "Only Henrik Sandberg can message checkout".into(),
            button: "Message Henrik Sandberg".into(),
            dm: "henrik".into(),
            remove: String::new(),
        })
        .unwrap(),
        json!({
            "text": "Only Henrik Sandberg can message checkout",
            "button": "Message Henrik Sandberg",
            "dm": "henrik",
            "remove": "",
        })
    );
    assert_eq!(
        serde_json::to_value(OrgHeader {
            name: "Acme".into(),
            initials: "A".into(),
            members: 3,
            members_label: "3 members".into(),
            stale: false,
            mark_all_read: None,
        })
        .unwrap(),
        json!({"name": "Acme", "initials": "A", "members": 3, "membersLabel": "3 members", "stale": false, "markAllRead": null})
    );
}

#[test]
fn a_push_of_several_commits_lists_them_under_its_verb() {
    let push = |subjects: &[&str]| {
        view::event_parts(
            "bob-mbp",
            Some("bob"),
            &Message::new(
                "cc-1",
                Event::Push {
                    branch: "master".into(),
                    head: "31811c1aaaa".into(),
                    forced: false,
                    subjects: subjects.iter().map(|&s| s.to_owned()).collect(),
                    paths: vec!["a.rs".into(), "b.rs".into()],
                    base: None,
                    lines: std::collections::BTreeMap::new(),
                    repo: None,
                    on: None,
                },
            ),
            chrono::Utc::now(),
            view::Names {
                owner: &str::to_owned,
                session: &str::to_owned,
            },
        )
    };
    let two = push(&["Sign the copy", "Jump to's last row"]);
    assert_eq!(two.verb, "pushed master to 31811c1 (2 commits)");
    assert_eq!(two.details, "Sign the copy\nJump to's last row");
    assert_eq!(two.paths, ["a.rs", "b.rs"]);
    // One commit says its subject in the verb, as before.
    let one = push(&["Sign the copy"]);
    assert_eq!(one.verb, "pushed master to 31811c1: Sign the copy");
    assert!(one.details.is_empty());
}

/// What `typed` in `here` comes to, through [`Chat::run`].
fn ran(chat: &mut Chat, here: &str, typed: &str) -> Ran {
    let command = crate::command::parse(typed).unwrap();
    chat.run(&Here::new(here, (15, 0)), command)
}

/// The lines an outcome sends, as text.
fn sends(outcome: &Ran) -> Vec<String> {
    match outcome {
        Ran::Send(lines) | Ran::Replied(lines) | Ran::Reacted { lines, .. } => texts(lines),
        other => panic!("sends nothing: {other:?}"),
    }
}

#[test]
fn a_typed_line_is_said_and_a_command_carried_out_as_every_app_does() {
    use crate::command::said;
    let mut chat = bare();
    connected(&mut chat);
    feed(
        &mut chat,
        &[
            "@msgid=a;time=2026-10-30T11:00:00.000Z :ada!u@h PRIVMSG #api :ship it?",
            ":s 332 me #api :the API",
        ],
    );
    assert_eq!(
        sends(&ran(&mut chat, "#api", "hello")),
        ["PRIVMSG #api hello"]
    );
    let action = sends(&ran(&mut chat, "#api", "/me waves"));
    assert!(action[0].contains("\u{1}ACTION waves\u{1}"), "{action:?}");
    // /react and /reply answer the newest message said.
    let reacted = ran(&mut chat, "#api", "/react tada");
    assert!(
        matches!(reacted, Ran::Reacted { emoji: "🎉", .. }),
        "{reacted:?}"
    );
    assert!(sends(&reacted)[0].contains("=a"), "{reacted:?}");
    let replied = ran(&mut chat, "#api", "/reply yes");
    assert!(matches!(&replied, Ran::Replied(_)), "{replied:?}");
    assert!(sends(&replied)[0].contains("+draft/reply=a"), "{replied:?}");
    assert_eq!(
        ran(&mut chat, "#api", "/react nosuch"),
        Ran::Note(said::no_emoji("nosuch"))
    );
    // /topic alone says the topic; with words, sets it.
    assert_eq!(
        ran(&mut chat, "#api", "/topic"),
        Ran::Note("the API".into())
    );
    assert_eq!(
        sends(&ran(&mut chat, "#api", "/topic new")),
        ["TOPIC #api new"]
    );
    assert_eq!(
        sends(&ran(&mut chat, "#api", "/topic a new one")),
        ["TOPIC #api :a new one"]
    );
    // /part asks first, as Leave does.
    assert_eq!(
        ran(&mut chat, "#api", "/part bye"),
        Ran::Leave {
            channel: "#api".into(),
            reason: Some("bye".into())
        }
    );
    let joins = sends(&ran(&mut chat, "#api", "/join #web,#ops")).join(" ");
    assert!(
        joins.contains("JOIN") && joins.contains("#web") && joins.contains("#ops"),
        "{joins}"
    );
    assert_eq!(
        ran(&mut chat, "#api", "/whois ada"),
        Ran::Card("ada".into())
    );
    assert_eq!(ran(&mut chat, "#api", "/nick neo"), Ran::Nick("neo".into()));
    assert_eq!(
        ran(&mut chat, "#api", "/invite ada"),
        Ran::Note(said::NOT_A_GROUP.into())
    );
    assert_eq!(
        said::refused(&crate::command::Command::Quote("x".into())),
        "/quote is for zirc's terminal client"
    );
    // In a direct conversation, /close is the app's; /part needs a channel.
    let _ = ran(&mut chat, "#api", "/query ada");
    assert_eq!(
        ran(&mut chat, "ada", "/close"),
        Ran::CloseDirect("ada".into())
    );
    assert_eq!(
        ran(&mut chat, "ada", "/part"),
        Ran::Note(said::PART_NEEDS.into())
    );
    assert_eq!(
        ran(&mut chat, "ada", "/topic hi"),
        Ran::Note(said::TOPIC_FOR_CHANNELS.into())
    );
}

#[test]
fn nothing_is_said_where_nobody_can_read_it() {
    use crate::command::said;
    let mut chat = bare();
    connected(&mut chat);
    let server = chat.conversations()[0].name.clone();
    assert_eq!(chat.conversations()[0].kind, Kind::Server);
    assert_eq!(
        ran(&mut chat, &server, "hi"),
        Ran::Note(said::NOT_A_CONVERSATION.into())
    );
    feed(&mut chat, &[":me!u@h PART #api"]);
    assert_eq!(
        ran(&mut chat, "#api", "hi"),
        Ran::Note(said::JOIN_FIRST.into())
    );
}

#[test]
fn the_server_channel_shows_notices_and_sends_what_is_typed_there_as_it_is() {
    let mut chat = Scene::in_api();
    // Until it's opened, the server's conversation stands for nothing shown.
    assert_eq!(chat.title("irc.zirc.test"), crate::chat::SERVER_TITLE);
    assert!(!chat.server_shown());
    let here = |chat: &Chat| chat.selected().name.clone();
    assert!(chat.open("irc.zirc.test").is_empty());
    assert!(chat.server_shown());
    assert_eq!(here(&chat), "irc.zirc.test");
    assert!(
        chat.nothing_joined().is_none(),
        "it's a channel of its own now"
    );

    let name = here(&chat);
    let ran = chat.run(
        &Here::new(&name, (15, 0)),
        crate::command::Command::Say("PRIVMSG NickServ :PUSH LIST".into()),
    );
    let Ran::Send(lines) = ran else {
        panic!("sent as it is: {ran:?}");
    };
    assert_eq!(texts(&lines), ["PRIVMSG NickServ :PUSH LIST"]);
    // Typing there tells no one.
    assert!(chat.typed(&name, false).is_empty());
    // The answer comes after what asked, though the server's clock says a little earlier.
    feed(
        &mut chat,
        &[
            "@time=2000-01-01T00:00:00.000Z :NickServ!NickServ@s NOTICE me :Nickname me has 1 push subscription(s)",
        ],
    );
    let said: Vec<&EntryKind> = chat.conversations()[0]
        .entries
        .iter()
        .map(|e| &e.kind)
        .collect();
    assert_eq!(
        said,
        [
            &EntryKind::Message("PRIVMSG NickServ :PUSH LIST".into()),
            &EntryKind::Notice("Nickname me has 1 push subscription(s)".into()),
        ]
    );
    // Never unread nor counted, and no row in the sidebar: Jump to finds it by its name, never
    // among the recent (the user's choice, 19 Nov 2026).
    assert_eq!(chat.conversations()[0].unread, 0);
    let rows = chat.sidebar_list(&ListOptions::default());
    assert!(rows.iter().all(|row| row.name != "irc.zirc.test"));
    assert!(chat.switcher("").iter().all(|name| name != "irc.zirc.test"));
    assert_eq!(chat.switcher("serv"), ["irc.zirc.test"]);

    // Never listed, even while it's shown: apps draw it by the shown row.
    assert!(
        chat.channel_rows()
            .iter()
            .all(|row| row.name != "irc.zirc.test")
    );
    let shown = chat.shown_row().expect("the Server channel is open");
    assert_eq!(
        (shown.name.as_str(), shown.title.as_str()),
        ("irc.zirc.test", crate::chat::SERVER_TITLE)
    );
    assert_eq!(chat.short_title("irc.zirc.test"), crate::chat::SERVER_TITLE);
    // Every app's header says the same: its short title, the Server channel's too.
    assert_eq!(
        chat.header_title("irc.zirc.test"),
        crate::chat::SERVER_TITLE
    );
    assert_eq!(chat.header_title("#api"), chat.short_title("#api"));
    // The AGENT tag is any agent session's, a teammate's too, and nothing else's.
    assert!(chat.shows_agent_tag("henrik-mbp/cc-1"));
    assert!(!chat.shows_agent_tag("#api") && !chat.shows_agent_tag("irc.zirc.test"));

    // Another conversation shown, the server's stands for nothing shown again.
    let _ = chat.open("#api");
    assert!(!chat.server_shown());
    assert_eq!(
        chat.shown_row().map(|row| row.name),
        Some("#api".to_owned())
    );
    assert!(
        chat.channel_rows()
            .iter()
            .all(|row| row.name != "irc.zirc.test")
    );
}

#[test]
fn the_server_channel_shows_what_irc_clients_show_but_not_the_apps_own_asking() {
    let mut chat = Scene::in_api();
    let before = chat.conversations()[0].entries.len();
    feed(
        &mut chat,
        &[
            ":irc.zirc.test 001 me :Welcome to the zirc IRC Network me",
            ":irc.zirc.test 005 me CHANTYPES=# NETWORK=zirc :are supported by this server",
            ":irc.zirc.test 251 me :There are 3 users and 0 invisible on 1 server(s)",
            ":irc.zirc.test 375 me :- irc.zirc.test Message of the day - ",
            ":irc.zirc.test 372 me :- be kind",
            ":irc.zirc.test 376 me :End of MOTD command",
            ":irc.zirc.test 900 me me!u@h me :You are now logged in as me",
            ":irc.zirc.test 265 me 3 5 :Current local users 3, max 5",
            ":irc.zirc.test 221 me +Zi",
            // The apps' own asking: no one's to read.
            ":irc.zirc.test 352 me #api u h irc.zirc.test bob H :0 Bob",
            ":irc.zirc.test 322 me #web 3 :Web",
        ],
    );
    let said = |chat: &Chat| -> Vec<String> {
        chat.conversations()[0].entries[before..]
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::Notice(text) | EntryKind::Message(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        said(&chat),
        [
            "Welcome to the zirc IRC Network me",
            "CHANTYPES=# NETWORK=zirc are supported by this server",
            "There are 3 users and 0 invisible on 1 server(s)",
            "- irc.zirc.test Message of the day - ",
            "- be kind",
            "End of MOTD command",
            "You are now logged in as me",
            "Current local users 3, max 5",
            "your modes: +Zi",
        ]
    );

    // Typed there, whatever answers it shows too, even what the apps read for themselves.
    let _ = chat.open("irc.zirc.test");
    // An app asking about it as if it were someone gets nothing to send.
    assert!(chat.whois("irc.zirc.test").is_empty());
    assert_eq!(
        chat.direct_kind("irc.zirc.test"),
        None,
        "no one's face in its header"
    );
    let _ = ran(&mut chat, "irc.zirc.test", "WHOIS bob");
    feed(
        &mut chat,
        &[
            ":irc.zirc.test 311 me bob u h * :Bob",
            ":irc.zirc.test 318 me bob :End of /WHOIS list",
        ],
    );
    assert_eq!(
        said(&chat)[9..],
        ["WHOIS bob", "bob u h * Bob", "bob End of /WHOIS list"]
    );
}

#[test]
fn an_action_and_a_notice_are_messages_in_the_run_their_words_styled() {
    let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
    connected(&mut chat);
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:10:00.000Z :alice!u@h PRIVMSG #api :hi",
            "@time=2026-10-30T11:10:30.000Z :alice!u@h PRIVMSG #api :\u{1}ACTION waves\u{1}",
            "@time=2026-10-30T11:11:00.000Z :alice!u@h NOTICE #api :build is green",
        ],
    );
    let lines = chat.lines("#api", chrono::FixedOffset::east_opt(0).unwrap(), false);
    let said: Vec<(WordStyle, RowShape, bool)> = lines
        .iter()
        .filter(|line| line.from == "alice")
        .map(|line| (line.style, line.shape, line.continues))
        .collect();
    assert_eq!(
        said,
        [
            (WordStyle::Plain, RowShape::Message, false),
            (WordStyle::Italic, RowShape::Message, true),
            (WordStyle::Muted, RowShape::Message, true),
        ]
    );
}
