//! The search screen: one query, in one conversation or everywhere, with the history's pages
//! and counts.

use zirc_proto::login::{ConversationCount, HistoryMessage, HistoryPage, HistorySender};

use super::*;
use crate::chat::{RecentSearches, SearchQuery, SearchScope, SearchSort, absorb_search};

/// A chat in `#api` and `#web`, with Leo's direct messages, some of it about timeouts.
fn scene() -> Chat {
    let mut chat = Scene::in_api().into_chat();
    feed(
        &mut chat,
        &[
            ":me!u@h JOIN #api",
            ":me!u@h JOIN #web",
            "@time=2026-11-06T09:14:00.000Z;msgid=a1 :leo!u@h PRIVMSG #api :the gateway's upstream timeout is 30s",
            "@time=2026-11-06T09:20:00.000Z;msgid=a2 :maya!u@h PRIVMSG #api :raising it to 60",
            "@time=2026-11-06T10:00:00.000Z;msgid=w1 :priya!u@h PRIVMSG #web :checkout timeout on slow 3G",
            "@time=2026-11-06T11:00:00.000Z;msgid=d1 :leo!u@h PRIVMSG me :did you bump the timeout?",
        ],
    );
    chat
}

fn said(id: &str, time: &str, conversation: &str, nick: &str, text: &str) -> HistoryMessage {
    HistoryMessage {
        msgid: id.into(),
        time: time.into(),
        conversation: conversation.into(),
        sender: HistorySender {
            account: Some(nick.into()),
            nick: nick.into(),
            kind: "person".into(),
        },
        text: text.into(),
        zirc: None,
        support: None,
    }
}

fn counts(pairs: &[(&str, u64)]) -> Vec<ConversationCount> {
    pairs
        .iter()
        .map(|(conversation, count)| ConversationCount {
            conversation: (*conversation).into(),
            count: *count,
        })
        .collect()
}

fn page(messages: Vec<HistoryMessage>, counts: Option<Vec<ConversationCount>>) -> HistoryPage {
    HistoryPage {
        policy: "channels".into(),
        messages,
        next: None,
        indexed: true,
        counts,
    }
}

fn wall(t: DateTime<Utc>) -> chrono::NaiveDateTime {
    t.naive_utc()
}

#[test]
fn search_starts_where_it_was_opened_and_the_chip_says_so() {
    let chat = scene();
    let here = chat.search_start(Some("#api"));
    assert_eq!(here.scope, SearchScope::Conversation("#api".into()));
    assert_eq!(chat.search_start(None).scope, SearchScope::Server);
    let query = crate::chat::compose_search(&here, "timeout").unwrap();
    let screen = chat.search_screen(&query, None, true, false, wall);
    assert_eq!(screen.placeholder, "Search #api");
    assert_eq!(screen.chips.len(), 1);
    assert_eq!(screen.chips[0].label, "in #api");
    assert!(screen.chips[0].scope);
    assert_eq!(
        screen.chips[0].a11y,
        "Searching in #api. Remove to search everywhere"
    );
    assert!(screen.filters.is_empty());
    // Everywhere, the filters show instead.
    let everywhere = chat.search_screen(&query.widened(), None, true, false, wall);
    assert!(everywhere.chips.is_empty());
    assert_eq!(everywhere.placeholder, "Search everywhere");
    let labels: Vec<_> = everywhere
        .filters
        .iter()
        .map(|f| f.label.as_str())
        .collect();
    assert_eq!(labels, ["In…", "From…"]);
}

#[test]
fn in_one_conversation_it_is_flat_newest_first_and_says_how_many_elsewhere() {
    let chat = scene();
    let query = SearchQuery::parse("timeout", Some("#api")).unwrap();
    let mut server = chat.server_search(&query);
    let params = server.params(50).unwrap();
    assert!(params.contains(&("conversation", "#api".to_owned())));
    assert!(params.contains(&("counts", "1".to_owned())));
    assert!(params.contains(&("sort", "new".to_owned())));

    // Before the server answers: what's loaded, and the widening row with no number.
    server.asking();
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    assert_eq!(screen.hits.len(), 1);
    assert!(screen.hits[0].loaded);
    assert_eq!(screen.header.as_deref(), Some("1 result in #api"));
    let row = screen.elsewhere.unwrap();
    assert_eq!((row.text, row.action), (None, "Search everywhere"));

    // The server's older match and its counts, only where the person may read.
    server.take_page(&page(
        vec![
            said(
                "a1",
                "2026-11-06T09:14:00Z",
                "#api",
                "leo",
                "the gateway's upstream timeout is 30s",
            ),
            said(
                "a0",
                "2026-11-02T08:00:00Z",
                "#api",
                "ci",
                "build 412 failed: timeout_test.rs",
            ),
        ],
        Some(counts(&[
            ("#api", 23),
            ("#web", 3),
            ("leo", 4),
            ("#ops", 7),
        ])),
    ));
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    let ids: Vec<_> = screen.hits.iter().map(|h| h.id.as_deref()).collect();
    assert_eq!(ids, [Some("a1"), Some("a0")]);
    assert!(screen.hits[0].loaded && !screen.hits[1].loaded);
    assert!(
        screen.hits[0]
            .parts
            .iter()
            .any(|p| p.hit && p.text == "timeout")
    );
    assert_eq!(screen.header.as_deref(), Some("23 results in #api"));
    assert_eq!(
        screen.elsewhere.unwrap().text.as_deref(),
        Some("Also 14 in 3 other conversations")
    );
    assert_eq!(screen.sort_label, "Sort: Newest");
    assert!(screen.groups.is_empty() && !screen.rooms);
    assert_eq!(screen.a11y, "23 results in #api, newest");

    // Offline: no row, and the limits line says why.
    let offline = chat.search_screen(&query, None, false, false, wall);
    assert!(offline.elsewhere.is_none());
    assert!(offline.everywhere_unavailable.is_some());
    assert!(screen.everywhere_unavailable.is_none());
    assert_eq!(
        offline.limits.unwrap().text,
        "Offline: searching only what this device has loaded."
    );
}

#[test]
fn everywhere_by_best_match_groups_by_conversation_and_by_time_is_one_list() {
    let chat = scene();
    let query = SearchQuery::parse("timeout", None).unwrap();
    let mut server = chat.server_search(&query);
    assert!(
        !server
            .params(50)
            .unwrap()
            .iter()
            .any(|(k, _)| *k == "conversation")
    );
    server.take_page(&page(
        vec![
            said(
                "a1",
                "2026-11-06T09:14:00Z",
                "#api",
                "leo",
                "the gateway's upstream timeout is 30s",
            ),
            said(
                "d1",
                "2026-11-06T11:00:00Z",
                "leo",
                "leo",
                "did you bump the timeout?",
            ),
            said(
                "a0",
                "2026-11-02T08:00:00Z",
                "#api",
                "ci",
                "build 412 failed: timeout_test.rs",
            ),
            said(
                "w1",
                "2026-11-06T10:00:00Z",
                "#web",
                "priya",
                "checkout timeout on slow 3G",
            ),
        ],
        Some(counts(&[
            ("#api", 23),
            ("leo", 4),
            ("#web", 3),
            ("#a", 1),
            ("#b", 1),
            ("#c", 1),
            ("#d", 1),
            ("#e", 1),
        ])),
    ));
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    assert_eq!(screen.sort_label, "Sort: Best match");
    let groups: Vec<(&str, u64, usize)> = screen
        .groups
        .iter()
        .map(|g| (g.conversation.as_str(), g.count, g.hits.len()))
        .collect();
    assert_eq!(groups, [("#api", 23, 2), ("leo", 4, 1), ("#web", 3, 1)]);
    // In a group, newest first; a direct message narrows by its @.
    assert_eq!(screen.groups[0].hits[0].id.as_deref(), Some("a1"));
    assert_eq!(screen.groups[1].token, "in:@leo");
    assert!(screen.groups[0].face.is_none() && screen.groups[1].face.is_some());
    // Its count, as a screen reader says it, narrows to it.
    assert_eq!(
        screen.groups[0].count_a11y,
        format!(
            "{} in {}: search only there",
            screen.groups[0].count, screen.groups[0].title
        )
    );
    assert_eq!(
        screen.header.as_deref(),
        Some("35 results in 8 conversations")
    );
    assert_eq!(
        screen.more_groups.as_deref(),
        Some("and 5 more conversations")
    );
    assert!(screen.elsewhere.is_none());

    // Newest first: one list, each row saying its conversation.
    let newest = SearchQuery::parse("sort:new timeout", None).unwrap();
    let mut server = chat.server_search(&newest);
    assert!(
        server
            .params(50)
            .unwrap()
            .contains(&("sort", "new".to_owned()))
    );
    server.take_page(&page(Vec::new(), None));
    let screen = chat.search_screen(&newest, Some(&server), true, false, wall);
    assert!(screen.groups.is_empty() && screen.rooms);
    let ids: Vec<_> = screen.hits.iter().map(|h| h.id.as_deref()).collect();
    assert_eq!(ids, [Some("d1"), Some("w1"), Some("a1")]);
    // A direct message's place doesn't name the person again.
    let places: Vec<_> = screen.hits.iter().map(|h| h.place.as_str()).collect();
    assert_eq!(places, ["DM", "#web", "#api"]);
    assert_eq!(
        chat.hit_place("leo", "me"),
        format!("to {}", chat.title("leo"))
    );
}

#[test]
fn the_sort_menu_offers_oldest_disabled_with_a_reason_while_online() {
    let chat = scene();
    let query = SearchQuery::parse("timeout", Some("#api")).unwrap();
    let screen = chat.search_screen(&query, None, true, false, wall);
    let sorts: Vec<_> = screen
        .sorts
        .iter()
        .map(|s| (s.label.as_str(), s.unavailable.is_some()))
        .collect();
    assert_eq!(
        sorts,
        [("Best match", false), ("Newest", false), ("Oldest", true)]
    );
    // Typed, it orders what's found, and says so.
    let old = SearchQuery::parse("sort:old timeout", Some("#web")).unwrap();
    let screen = chat.search_screen(&old, None, true, false, wall);
    assert_eq!(screen.sort, SearchSort::Old);
    assert!(
        screen
            .limits
            .unwrap()
            .text
            .starts_with("Oldest first among")
    );
}

#[test]
fn limits_say_the_first_that_applies_and_unsupported_tokens_are_named() {
    let chat = scene();
    let query = SearchQuery::parse("agent:checkout has:file timeout", None).unwrap();
    let mut server = chat.server_search(&query);
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    assert_eq!(
        screen.limits.unwrap().text,
        "agent:checkout, has:file isn't searched yet: these matches don't check it."
    );
    let mut unindexed = page(Vec::new(), None);
    unindexed.indexed = false;
    server.take_page(&unindexed);
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    assert!(screen.limits.unwrap().text.starts_with("Still indexing"));
    server.failed();
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    assert!(
        screen
            .limits
            .unwrap()
            .text
            .starts_with("Couldn't reach the server")
    );
}

#[test]
fn an_empty_search_says_where_it_looked_and_what_to_try() {
    let chat = scene();
    let query = SearchQuery::parse("timout", Some("#api")).unwrap();
    let mut server = chat.server_search(&query);
    server.take_page(&page(Vec::new(), Some(counts(&[]))));
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    assert!(screen.header.is_none() && screen.bar().is_none());
    let empty = screen.empty.unwrap();
    assert_eq!(empty.text, "Nothing in #api matches “timout”.");
    assert_eq!(
        empty.hint.as_deref(),
        Some("Try other words, or fewer of them.")
    );
    assert!(screen.elsewhere.is_none(), "counted, and nothing elsewhere");
    // With matches elsewhere, the widening row says it instead of the hint.
    let mut server = chat.server_search(&query);
    server.take_page(&page(Vec::new(), Some(counts(&[("#web", 2)]))));
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    assert_eq!(screen.empty.unwrap().hint, None);
    assert_eq!(
        screen.elsewhere.unwrap().text.as_deref(),
        Some("Also 2 in 1 other conversation")
    );
    let everywhere = chat.search_screen(&query.widened(), None, true, false, wall);
    assert_eq!(
        everywhere.empty.unwrap().text,
        "Nothing you can read matches “timout”."
    );
    // Nothing typed: nothing said.
    let blank = chat.search_screen(&chat.search_start(None), None, true, false, wall);
    assert!(blank.header.is_none() && blank.empty.is_none());
}

#[test]
fn typed_tokens_become_chips_and_the_rest_stays_typed() {
    let base = SearchQuery::parse("", Some("#api")).unwrap();
    let (query, rest) = absorb_search(&base, "from:leo time").unwrap();
    assert_eq!(query.from.as_deref(), Some("leo"));
    assert_eq!(rest, "time");
    // A token still being typed stays typed.
    assert!(absorb_search(&base, "timeout from:le").is_none());
    let (query, rest) = absorb_search(&base, "in:* ").unwrap();
    assert_eq!(query.scope, SearchScope::Server);
    assert_eq!(rest, "");
    assert!(absorb_search(&base, "after:yesterday ").is_none());
}

#[test]
fn an_agent_s_conversation_is_searched_as_its_session_s_lines() {
    let chat = scene();
    let query = SearchQuery::parse("in:me-mbp/cc-1 tests", None).unwrap();
    let mut server = chat.server_search(&query);
    assert!(
        server
            .params(50)
            .unwrap()
            .contains(&("conversation", "@me-mbp".to_owned()))
    );
    let mut mine = said(
        "s1",
        "2026-11-06T09:00:00Z",
        "me-mbp",
        "me-mbp",
        "tests pass",
    );
    mine.zirc = Some(serde_json::json!({ "session": "cc-1" }));
    let mut theirs = said(
        "s2",
        "2026-11-06T09:01:00Z",
        "me-mbp",
        "me-mbp",
        "tests fail",
    );
    theirs.zirc = Some(serde_json::json!({ "session": "cc-2" }));
    server.take_page(&page(vec![mine, theirs], None));
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    let ids: Vec<_> = screen.hits.iter().map(|h| h.id.as_deref()).collect();
    assert_eq!(ids, [Some("s1")]);
}

#[test]
fn a_hit_says_its_day_and_time_from_the_person_s_today() {
    use crate::chat::hit_when;
    let at = |text: &str| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap();
    let now = at("2026-11-07 17:00");
    assert_eq!(hit_when(at("2026-11-07 00:00"), now), "00:00");
    assert_eq!(hit_when(at("2026-11-06 23:59"), now), "Yesterday 23:59");
    assert_eq!(hit_when(at("2026-11-03 16:20"), now), "Tue 16:20");
    assert_eq!(hit_when(at("2026-11-01 08:00"), now), "Sun 08:00");
    assert_eq!(hit_when(at("2026-10-31 08:00"), now), "31 Oct 08:00");
    assert_eq!(hit_when(at("2026-01-01 08:00"), now), "1 Jan 08:00");
    assert_eq!(hit_when(at("2025-12-31 23:30"), now), "31 Dec 2025 23:30");
}

#[test]
fn a_hit_s_day_is_the_person_s_own_around_midnight() {
    // 23:30 UTC on the 6th, read at 00:30 UTC on the 7th: yesterday in UTC, today an hour
    // east of it.
    let mut chat = scene();
    chat.clock = || Utc.with_ymd_and_hms(2026, 11, 7, 0, 30, 0).unwrap();
    let query = SearchQuery::parse("late", Some("#api")).unwrap();
    let mut server = chat.server_search(&query);
    server.take_page(&page(
        vec![said(
            "z1",
            "2026-11-06T23:30:00Z",
            "#api",
            "leo",
            "late deploy",
        )],
        None,
    ));
    let utc = chat.search_screen(&query, Some(&server), true, false, wall);
    assert_eq!(utc.hits[0].when, "Yesterday 23:30");
    assert_eq!(utc.hits[0].clock, "23:30");
    let east = |t: DateTime<Utc>| t.naive_utc() + chrono::Duration::hours(1);
    let paris = chat.search_screen(&query, Some(&server), true, false, east);
    assert_eq!(paris.hits[0].when, "00:30");
    // West of UTC, the 6th's evening is today and the 7th hasn't come.
    let west = |t: DateTime<Utc>| t.naive_utc() - chrono::Duration::hours(5);
    assert_eq!(
        chat.search_screen(&query, Some(&server), true, false, west)
            .hits[0]
            .when,
        "18:30"
    );
}

#[test]
fn recent_searches_keep_ten_newest_first_each_once() {
    let mut recent = RecentSearches::default();
    for n in 0..12 {
        recent.remember(&SearchQuery::parse(&format!("word{n}"), Some("#api")).unwrap());
    }
    recent.remember(&SearchQuery::parse("word5", Some("#api")).unwrap());
    recent.remember(&SearchQuery::parse("", None).unwrap());
    assert_eq!(recent.0.len(), 10);
    assert_eq!(recent.0[0], "in:#api word5");
    assert_eq!(
        recent.queries()[0].scope,
        SearchScope::Conversation("#api".into())
    );
    recent.clear();
    assert!(recent.0.is_empty());
}

#[test]
fn the_server_s_name_for_one_s_own_direct_messages_is_the_conversation_here() {
    let chat = scene();
    let query = SearchQuery::parse("timeout", None).unwrap();
    let mut server = chat.server_search(&query);
    server.take_page(&page(
        vec![said(
            "d1",
            "2026-11-06T11:00:00Z",
            "@leo,@me",
            "leo",
            "did you bump the timeout?",
        )],
        Some(counts(&[("@leo,@me", 1), ("#api", 2)])),
    ));
    let screen = chat.search_screen(&query, Some(&server), true, false, wall);
    let leo = screen
        .groups
        .iter()
        .find(|group| group.hits.iter().any(|hit| hit.id.as_deref() == Some("d1")))
        .expect("Leo's group");
    assert_eq!(leo.conversation, "leo");
    assert_eq!(leo.count, 1);
    assert_eq!(leo.token, "in:@leo");
}
