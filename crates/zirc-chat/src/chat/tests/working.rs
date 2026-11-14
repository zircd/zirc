//! The channel list as one list, who is changing what per branch, and what sessions are
//! about to change ([`Chat::channel_rows`], [`Chat::working_people`], [`Chat::intending`]).

use super::*;

/// Connected as `me` and in `#api`, with `@beta` and `gamma`.
fn joined() -> Chat {
    let mut chat = bare();
    chat.connected(link(true));
    feed(
        &mut chat,
        &[
            ":srv 322 me #api 3 :the API",
            ":srv 323 me :End of LIST",
            ":me!u@h JOIN #api",
            ":srv 353 me = #api :me @beta gamma",
            ":srv 366 me #api :End",
        ],
    );
    chat
}

/// Every path `person` is changing, on any branch.
fn paths(person: &WorkingPerson) -> Vec<(String, bool)> {
    person
        .sets
        .iter()
        .flat_map(|set| set.paths.iter().cloned())
        .collect()
}

#[test]
fn lists_the_servers_channels_joined_or_not_then_direct_messages() {
    let mut chat = bare();
    chat.connected(link(true));
    feed(
        &mut chat,
        &[
            ":srv 322 me #api 3 :x",
            ":srv 322 me #web 1 :y",
            ":srv 323 me :End",
            ":me!u@h JOIN #api",
            ":beta!u@h PRIVMSG me :hi",
            ":beta!u@h INVITE me #dm-abc",
            ":me!u@h JOIN #dm-abc",
            ":srv 353 me @ #dm-abc :me beta gamma",
            ":srv 366 me #dm-abc :End",
        ],
    );
    let rows: Vec<(SidebarSection, String, bool, usize, usize)> = chat
        .channel_rows()
        .into_iter()
        .map(|row| (row.section, row.name, row.joined, row.users, row.unread))
        .collect();
    assert_eq!(
        rows,
        [
            (SidebarSection::Channels, "#api".into(), true, 1, 0),
            (SidebarSection::Channels, "#web".into(), false, 1, 0),
            (SidebarSection::Direct, "beta".into(), true, 0, 1),
            (SidebarSection::Direct, "#dm-abc".into(), true, 3, 0),
        ]
    );
}

#[test]
fn tracks_who_works_on_what_and_their_overlaps() {
    let mut chat = joined();
    for (nick, set) in [
        ("beta", &["a.rs", "b.rs"][..]),
        ("beta", &["a.rs"]),
        ("gamma", &["a.rs", "c.rs"]),
    ] {
        feed(&mut chat, &[&zirc(nick, &working("watcher", set))]);
    }
    let people = chat.working_people("#api");
    assert_eq!(people[0].person.nick, "beta");
    assert!(people[0].here && people[0].person.op);
    assert_eq!(paths(&people[0]), [("a.rs".into(), true)]);
    assert_eq!(
        paths(&people[1]),
        [("a.rs".into(), true), ("c.rs".into(), false)]
    );
    assert!(people[2].person.me && people[2].sets.is_empty());
}

#[test]
fn lists_work_announced_by_someone_not_in_the_member_list() {
    let mut chat = joined();
    feed(&mut chat, &[&zirc("delta", &working("watcher", &["d.rs"]))]);
    let delta = chat
        .working_people("#api")
        .into_iter()
        .find(|person| person.person.nick == "delta")
        .unwrap();
    assert!(!delta.here && !delta.person.op && !delta.person.me);
    assert_eq!(delta.files(), 1);
}

#[test]
fn keeps_a_machines_worktrees_apart() {
    let mut chat = joined();
    for (session, worktree, branch, paths, part) in [
        ("cc-1", "api-login", "fix-login", &["a.rs"][..], [1, 2]),
        ("cc-2", "api-hooks", "retry-hooks", &["b.rs"], [2, 2]),
    ] {
        let event = Event::Working {
            paths: paths.iter().map(|&p| p.to_owned()).collect(),
            continued: part[0] > 1,
            worktree: Some(worktree.into()),
            branch: Some(branch.into()),
            part: Some(part),
            base: None,
            committed: Vec::new(),
        };
        feed(&mut chat, &[&zirc("beta", &Message::new(session, event))]);
    }
    let beta = &chat.working_people("#api")[0];
    let sets: Vec<(Option<&str>, Option<&str>, usize)> = beta
        .sets
        .iter()
        .map(|set| {
            (
                set.branch.as_deref(),
                set.session.as_deref(),
                set.paths.len(),
            )
        })
        .collect();
    assert_eq!(
        sets,
        [
            (Some("retry-hooks"), Some("cc-2"), 1),
            (Some("fix-login"), Some("cc-1"), 1)
        ]
    );
    assert_eq!(beta.files(), 2);
}

#[test]
fn shows_what_sessions_are_about_to_change() {
    let mut chat = joined();
    let say = |chat: &mut Chat, from: &str, id: &str, event: Event| {
        feed(chat, &[&zirc(from, &Message::new(id, event))]);
    };
    let intend = |summary: &str, paths: &[&str]| Event::Intent {
        summary: summary.into(),
        paths: paths.iter().map(|&p| p.to_owned()).collect(),
        expires: None,
        lasts: Some(1800),
    };
    feed(
        &mut chat,
        &[&zirc(
            "carol",
            &working("watcher", &["src/billing/webhook.rs"]),
        )],
    );
    say(
        &mut chat,
        "bob",
        "cc-1",
        intend("retry webhooks", &["src/billing/", "docs/"]),
    );
    say(
        &mut chat,
        "carol",
        "cc-2",
        intend("tidy webhooks", &["src/billing/webhook.rs"]),
    );

    let intents = chat.intending("#api");
    let bob = intents.iter().find(|i| i.nick == "bob").unwrap();
    assert_eq!(
        bob.paths,
        [("src/billing/".into(), true), ("docs/".into(), false)]
    );
    assert_eq!(bob.with, ["carol"]);
    assert!(bob.expires.is_some());
    // Her own machine's set isn't something carol runs into.
    let carol = intents.iter().find(|i| i.nick == "carol").unwrap();
    assert_eq!(carol.with, ["bob"]);

    say(
        &mut chat,
        "bob",
        "cc-1",
        Event::Text {
            body: "done: retried".into(),
        },
    );
    let left: Vec<String> = chat.intending("#api").into_iter().map(|i| i.nick).collect();
    assert_eq!(left, ["carol"]);
}
