//! The search screen, for every app: one query ([`SearchQuery`]) in one conversation or
//! everywhere the person may read, as the design lead's `search-scope-ux.md` lays it out.
//! What's loaded matches at once; the organisation's history answers the rest a page at a
//! time ([`ServerSearch`]: the app asks with [`ServerSearch::params`] and gives back what it
//! got with [`ServerSearch::take_page`]), with how many matches each conversation has, so a
//! search in one conversation can say how many there are elsewhere. [`Chat::search_screen`]
//! is what the app draws: the chips, the header, the hits flat or grouped by conversation,
//! the sort menu, the widening row, the limits line and what an empty search says.
//!
//! The server sorts best first or newest first; it can't yet page oldest first, nor filter by
//! `agent:`, `has:` or a kind of event, so those are offered disabled, or said under the
//! results, never pretended.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use zirc_proto::login::{ConversationCount, HistoryPage};
use zirc_proto::search::{Query as SearchQuery, Scope, SearchSort as Sort};

use super::conversation::{Kind, agent, fold, is_channel};
use super::search::{Hit, SearchHit};
use serde::Serialize;
use ts_rs::TS;

use super::{Avatar, Chat, Face};

/// What a search that found nothing suggests, when the widening row doesn't.
pub(super) const TRY_OTHER_WORDS: &str = "Try other words, or fewer of them.";

/// How far back the server is asked to look: as far as it reads.
const REACH: Duration = Duration::days(400);

/// How many conversations a search everywhere shows before "and N more".
pub const GROUPS_SHOWN: usize = 6;

/// How many hits each conversation's group shows.
pub const GROUP_HITS: usize = 3;

/// How many hits a flat list shows.
const SHOWN: usize = 200;

/// How many hits a page of the history holds.
pub const PAGE: usize = 50;

/// The widening row's action.
pub const SEARCH_EVERYWHERE: &str = "Search everywhere";

/// What asks for the next page of the history.
pub const MORE_RESULTS: &str = "More results";

/// Why searching everywhere can't be chosen offline.
pub const OFFLINE_EVERYWHERE: &str = "Offline: only what this device has loaded";

/// A search of the organisation's history: its pages so far, and how many matches each
/// conversation has.
#[derive(Debug, Clone)]
pub struct ServerSearch {
    /// What's searched for.
    pub query: SearchQuery,
    /// The conversation as the server names it, for a search in one; and the agent session
    /// whose lines alone count, for an agent's conversation.
    server: Option<(String, Option<String>)>,
    /// The matches so far, in the server's order.
    hits: Vec<(String, Hit)>,
    /// How many matches each conversation has, when the server counted.
    counts: Option<Vec<ConversationCount>>,
    /// Whether the server's index answered; `None` until a page came.
    indexed: Option<bool>,
    /// Where the next page starts.
    cursor: Option<String>,
    /// Whether there's no page to ask for.
    done: bool,
    /// Whether a page is being asked for.
    asking: bool,
    /// Whether the last ask failed.
    failed: bool,
    /// The ids found, so pages don't repeat them.
    seen: HashSet<String>,
    /// When it started, for how far back to ask.
    started: DateTime<Utc>,
}

impl ServerSearch {
    /// What to ask the history for next, the query string's pairs with `limit`; `None` when
    /// there's nothing more, or nothing to find.
    #[must_use]
    pub fn params(&self, limit: usize) -> Option<Vec<(&'static str, String)>> {
        if self.done {
            return None;
        }
        let mut params: Vec<(&'static str, String)> = self
            .query
            .params()
            .into_iter()
            .filter(|(key, _)| *key != "conversation")
            .collect();
        if let Some((conversation, _)) = &self.server {
            params.push(("conversation", conversation.clone()));
        }
        if self.query.after.is_none() {
            params.push(("since", (self.started - REACH).to_rfc3339()));
        }
        params.push(("limit", limit.to_string()));
        if let Some(cursor) = &self.cursor {
            params.push(("cursor", cursor.clone()));
        }
        Some(params)
    }

    /// Says a page is being asked for.
    pub fn asking(&mut self) {
        self.asking = true;
        self.failed = false;
    }

    /// Takes in zirc.dev's answer to [`ServerSearch::params`], as every app reads it: a
    /// page, or none when it didn't take the account, which, as an answer it couldn't give,
    /// is a failure, as is any error.
    pub fn take_answer<E>(&mut self, answer: Result<Option<HistoryPage>, E>) {
        match answer {
            Ok(Some(page)) => self.take_page(&page),
            Ok(None) | Err(_) => self.failed(),
        }
    }

    /// Says the page couldn't be had: the app is offline, or the server refused.
    pub fn failed(&mut self) {
        self.asking = false;
        self.failed = true;
    }

    /// Whether a page is being asked for.
    #[must_use]
    pub fn is_asking(&self) -> bool {
        self.asking
    }

    /// Whether a page has been asked for since it was made: being asked, answered or failed.
    #[must_use]
    pub fn asked(&self) -> bool {
        self.asking || self.failed || self.indexed.is_some()
    }

    /// Takes in a page the server gave for [`ServerSearch::params`].
    pub fn take_page(&mut self, page: &HistoryPage) {
        let session = self.server.as_ref().and_then(|(_, s)| s.clone());
        for message in &page.messages {
            let said_by = message
                .zirc
                .as_ref()
                .and_then(|zirc| zirc.get("session"))
                .and_then(|session| session.as_str())
                .filter(|session| !session.is_empty())
                .map(str::to_owned);
            // An agent's conversation is the session's own lines in its machine's.
            if session.is_some() && said_by != session {
                continue;
            }
            if !self.seen.insert(message.msgid.clone()) {
                continue;
            }
            let Ok(time) = DateTime::parse_from_rfc3339(&message.time) else {
                continue;
            };
            self.hits.push((
                message.conversation.clone(),
                Hit {
                    id: Some(message.msgid.clone()),
                    time: time.with_timezone(&Utc),
                    from: message.sender.nick.clone(),
                    session: said_by,
                    text: message.text.clone(),
                    loaded: false,
                },
            ));
        }
        if page.counts.is_some() || self.counts.is_none() {
            self.counts.clone_from(&page.counts);
        }
        self.indexed = Some(page.indexed);
        self.cursor.clone_from(&page.next);
        self.done = page.next.is_none();
        self.asking = false;
        self.failed = false;
    }

    /// Whether the history may have more, for [`MORE_RESULTS`].
    #[must_use]
    pub fn more(&self) -> bool {
        !self.done && self.indexed.is_some()
    }
}

/// A chip in the search box: a token of the query, which can be taken away. Every app reads
/// this record, the phones through `UniFFI` and the web chat as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ScreenChip {
    /// What it shows: `in #api`, `from leo`.
    pub label: String,
    /// The token it stands for, to take it away: `in:#api`.
    pub token: String,
    /// What a screen reader says: "Searching in #api. Remove to search everywhere".
    pub a11y: String,
    /// Whether it's the scope.
    pub scope: bool,
    /// Whether it can be taken away: a guest's scope can't ([`super::Finding::chips`]).
    pub removable: bool,
}

/// One of the sort menu's choices. Every app reads this record, the phones through `UniFFI`
/// and the web chat as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct SortChoice {
    /// The order.
    pub sort: Sort,
    /// What it's called: "Best match", "Newest", "Oldest".
    pub label: String,
    /// Whether it's the order the results are in.
    pub chosen: bool,
}

/// A conversation's matches, in a search everywhere sorted best first: ready to draw and
/// send as it is, every app reads this record, the phones through `UniFFI` and web as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchGroup {
    /// The conversation, by name, to narrow to or open.
    pub conversation: String,
    /// As the sidebar names it.
    pub title: String,
    /// Its face: a person's or the agents'; none for a channel.
    pub face: Option<Face>,
    /// How many matches it has: what its `23 ›` says.
    #[ts(type = "number")]
    pub count: u64,
    /// What a screen reader says of that count, which narrows the search to it: `23 in
    /// #api: search only there`.
    pub count_a11y: String,
    /// What a screen reader calls its list of hits: `Results in #api`.
    pub hits_a11y: String,
    /// Its best few, newest first.
    pub hits: Vec<SearchHit>,
    /// The token that narrows the search to it: `in:#api`.
    pub token: String,
}

/// The row under one conversation's matches that widens the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elsewhere {
    /// "Also 14 in 5 other conversations", or nothing while they're counted.
    pub text: Option<String>,
    /// [`SEARCH_EVERYWHERE`].
    pub action: &'static str,
}

/// The muted line under the results that says what the search couldn't cover, as every app
/// draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchLimits {
    /// What it says.
    pub text: String,
    /// Whether it's a warning, drawn in the warning tone; never an error.
    pub warning: bool,
}

/// What a search that found nothing says, as every app draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchEmpty {
    /// "Nothing in #api matches "timout"."
    pub text: String,
    /// What to try, unless the widening row says it.
    pub hint: Option<String>,
}

/// A filter offered under the box of a search everywhere with nothing narrowing it, as every
/// app draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchFilter {
    /// What it shows: "In…", "From…".
    pub label: String,
    /// What it puts in the box, completed by a picker: `in:`, `from:`.
    pub token: String,
}

/// The search screen as every app draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchScreen {
    /// What's searched for.
    pub query: SearchQuery,
    /// The chips in the box, scope first.
    pub chips: Vec<ScreenChip>,
    /// The conversation it's scoped to, as shown; none everywhere.
    pub scope_title: Option<String>,
    /// What the empty box says: "Search #api", "Search everywhere".
    pub placeholder: String,
    /// "23 results in #api", "37 results in 6 conversations", "Searching…"; none before
    /// anything's typed.
    pub header: Option<String>,
    /// The order the hits are in.
    pub sort: Sort,
    /// The sort menu's button: "Sort: Newest".
    pub sort_label: String,
    /// The sort menu's choices.
    pub sorts: Vec<SortChoice>,
    /// The hits by conversation: a search everywhere, best first.
    pub groups: Vec<SearchGroup>,
    /// "and 2 more conversations", when groups were left out.
    pub more_groups: Option<String>,
    /// The hits in one list, otherwise.
    pub hits: Vec<SearchHit>,
    /// The widening row, in one conversation.
    pub elsewhere: Option<Elsewhere>,
    /// What the search couldn't cover.
    pub limits: Option<SearchLimits>,
    /// What a search that found nothing says.
    pub empty: Option<SearchEmpty>,
    /// Filters to add, for a search everywhere with none.
    pub filters: Vec<SearchFilter>,
    /// Whether the history may have more: [`MORE_RESULTS`].
    pub more: bool,
    /// Whether the history is being asked.
    pub searching: bool,
    /// What a screen reader announces once typing settles.
    pub a11y: String,
    /// Why searching everywhere can't be chosen, for a phone's segment to grey out with;
    /// none when it can.
    pub everywhere_unavailable: Option<&'static str>,
}

/// The segmented control a search opened in a conversation shows, in every app: that
/// conversation beside Everywhere, in place of the scope's chip ([`segments`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchSegments {
    /// The conversation as the sidebar names it: `#api`, `Leo Okafor`, `checkout`.
    pub here: String,
    /// [`super::EVERYWHERE`].
    pub everywhere: String,
    /// Whether Everywhere is chosen.
    pub everywhere_chosen: bool,
    /// Why Everywhere can't be chosen, when it can't: offline, or a server that searches
    /// one conversation at a time.
    pub everywhere_unavailable: Option<String>,
}

/// The control over `screen`, for a search opened in `opened_in`, the conversation the
/// sidebar names `here`: while it searches there or everywhere. None for one opened
/// everywhere, or narrowed to another conversation since, whose scope is a chip.
#[must_use]
pub fn segments(
    screen: &SearchScreen,
    opened_in: Option<&str>,
    here: Option<String>,
) -> Option<SearchSegments> {
    let opened_in = opened_in?;
    let everywhere_chosen = matches!(screen.query.scope, Scope::Server | Scope::AllServers);
    if !everywhere_chosen && screen.query.scope != segment_scope(Some(opened_in), false) {
        return None;
    }
    Some(SearchSegments {
        here: here?,
        everywhere: super::EVERYWHERE.to_owned(),
        everywhere_chosen,
        everywhere_unavailable: screen.everywhere_unavailable.map(str::to_owned),
    })
}

/// The scope the control picks: everywhere, or back in the conversation search was opened
/// in, `opened_in`.
#[must_use]
pub fn segment_scope(opened_in: Option<&str>, everywhere: bool) -> Scope {
    match (opened_in, everywhere) {
        (Some(here), false) => {
            SearchQuery::parse("", Some(here)).map_or(Scope::Server, |query| query.scope)
        }
        _ => Scope::Server,
    }
}

impl SearchScreen {
    /// What the bar over the conversation says while the search shows, back to the list:
    /// `23 results for “timeout” in #api`; none before anything's typed.
    #[must_use]
    pub fn bar(&self) -> Option<String> {
        if !self.found_any() {
            return None;
        }
        let header = self.header.as_ref()?;
        let words = self.query.text.trim().trim_matches('"');
        if words.is_empty() {
            return Some(header.clone());
        }
        Some(match header.split_once(" in ") {
            Some((count, place)) => format!("{count} for “{words}” in {place}"),
            None => format!("{header} for “{words}”"),
        })
    }

    /// Whether it shows anything found.
    #[must_use]
    pub fn found_any(&self) -> bool {
        !self.hits.is_empty() || !self.groups.is_empty()
    }
}

/// The base of a search with what's typed in its box after the chips: tokens followed by a
/// space become chips, and the rest stays typed. `None` when nothing's to become a chip.
#[must_use]
pub fn absorb(base: &SearchQuery, typed: &str) -> Option<(SearchQuery, String)> {
    let ends = typed.ends_with(char::is_whitespace);
    let words: Vec<&str> = typed.split_whitespace().collect();
    let complete = if ends {
        words.len()
    } else {
        words.len().saturating_sub(1)
    };
    let mut query = base.clone();
    let mut rest = Vec::new();
    let mut took = false;
    for (at, word) in words.iter().enumerate() {
        let token = at < complete && !word.starts_with('"');
        let merged = token
            .then(|| SearchQuery::parse(&format!("{query} {word}"), None).ok())
            .flatten()
            .filter(|merged| merged.text == query.text);
        match merged {
            Some(merged) => {
                query = merged;
                took = true;
            }
            None => rest.push(*word),
        }
    }
    let mut rest = rest.join(" ");
    if ends && !rest.is_empty() {
        rest.push(' ');
    }
    took.then_some((query, rest))
}

/// The search of `typed` on top of the chips in `base`.
///
/// # Errors
///
/// A token whose value can't be read, such as a date that isn't `YYYY-MM-DD`.
pub fn compose(
    base: &SearchQuery,
    typed: &str,
) -> Result<SearchQuery, zirc_proto::search::Unreadable> {
    SearchQuery::parse(&format!("{base} {typed}"), None)
}

/// What a sort is called.
#[must_use]
pub fn sort_name(sort: Sort) -> &'static str {
    match sort {
        Sort::Best => "Best match",
        Sort::New => "Newest",
        Sort::Old => "Oldest",
    }
}

/// What a search screen says while the history is asked and nothing is found yet, and its
/// More button says while it asks for the next page.
pub const SEARCHING: &str = "Searching…";

/// What the sort button says to a screen reader: `Sort results: Newest`.
#[must_use]
pub fn sort_a11y(sort: Sort) -> String {
    format!("Sort results: {}", sort_name(sort))
}

/// `n` results, said: "1 result", "23 results".
fn results(n: u64) -> String {
    if n == 1 {
        "1 result".to_owned()
    } else {
        format!("{n} results")
    }
}

/// `n` conversations, said.
fn conversations(n: usize) -> String {
    if n == 1 {
        "1 conversation".to_owned()
    } else {
        format!("{n} conversations")
    }
}

/// The name the server and the counts give a conversation, compared bare.
fn bare(name: &str) -> String {
    fold(name.trim_start_matches('@'))
}

impl Chat {
    /// The search conversation `opened_in` starts as, or one everywhere:
    /// scoped to it, with nothing typed.
    #[must_use]
    pub fn search_start(&self, opened_in: Option<&str>) -> SearchQuery {
        let opened_in = opened_in.filter(|name| {
            self.conversation(name)
                .is_some_and(|conversation| conversation.kind != Kind::Server)
        });
        let scope = opened_in
            .and_then(|name| SearchQuery::parse("", Some(name)).ok())
            .map_or(Scope::Server, |query| query.scope);
        SearchQuery {
            text: String::new(),
            scope,
            sort: None,
            from: None,
            agent: None,
            kinds: Vec::new(),
            has: Vec::new(),
            after: None,
            before: None,
        }
    }

    /// The conversation `query` is scoped to, by the name it has here; `None` everywhere.
    #[must_use]
    pub fn search_scope(&self, query: &SearchQuery) -> Option<String> {
        let wanted = match &query.scope {
            Scope::Conversation(name) => name.clone(),
            Scope::Agent { machine, session } => format!("{machine}/{session}"),
            Scope::Server | Scope::AllServers => return None,
        };
        let found = self
            .conversations()
            .iter()
            .find(|conversation| bare(&conversation.name) == bare(&wanted))
            .map(|conversation| conversation.name.clone());
        Some(found.unwrap_or(wanted))
    }

    /// The history search for `query`, to ask the server with.
    #[must_use]
    pub fn server_search(&self, query: &SearchQuery) -> ServerSearch {
        let scope = self.search_scope(query);
        let server = scope.as_deref().map(|name| match agent(name) {
            Some((machine, session)) => (format!("@{machine}"), Some(session.to_owned())),
            None if is_channel(name) => (name.to_owned(), None),
            None => (format!("@{name}"), None),
        });
        let asks = !query.text.trim().is_empty() || query.from.is_some();
        ServerSearch {
            query: query.clone(),
            server,
            hits: Vec::new(),
            counts: None,
            indexed: None,
            cursor: None,
            done: !asks,
            asking: false,
            failed: false,
            seen: HashSet::new(),
            started: (self.clock)(),
        }
    }

    /// Where a conversation the server names `server_name` is here: the conversation of that
    /// name, else the name as the server gave it.
    fn local_name(&self, server_name: &str) -> String {
        // The person's own direct messages come back as the pair, `@me,@leo`: here, `leo`.
        let me = fold(self.me());
        let server_name = match server_name.split_once(',') {
            Some((a, b)) if bare(a) == me => b,
            Some((a, b)) if bare(b) == me => a,
            _ => server_name,
        };
        self.conversations()
            .iter()
            .find(|conversation| bare(&conversation.name) == bare(server_name))
            .map_or_else(
                || server_name.trim_start_matches('@').to_owned(),
                |conversation| conversation.name.clone(),
            )
    }

    /// The search screen for `query`, with what the history found (`server`), whether the
    /// app can reach it (`online`), and whether every conversation's group shows
    /// (`all_groups`). Times read on the app's clock, `wall`.
    #[must_use]
    pub fn search_screen(
        &self,
        query: &SearchQuery,
        server: Option<&ServerSearch>,
        online: bool,
        all_groups: bool,
        wall: impl Fn(DateTime<Utc>) -> NaiveDateTime,
    ) -> SearchScreen {
        let scope = self.search_scope(query);
        let scope_title: Option<String> = scope.as_deref().map(|name| self.title(name));
        let typed = is_typed(query);
        let sort = query.sort();
        let server = server.filter(|server| server.query == *query);
        let everywhere = scope.is_none();
        let found = self.search_found(query, scope.as_ref(), server, &wall);

        // Counts per conversation, by the name here.
        let counts: Option<Vec<(String, u64)>> = server
            .and_then(|server| server.counts.as_ref())
            .map(|counts| {
                counts
                    .iter()
                    .map(|count| (self.local_name(&count.conversation), count.count))
                    .collect()
            });
        let counts = counts.as_deref();

        let grouped = everywhere && sort == Sort::Best;
        let (groups, more_groups, hits) = if grouped {
            let (groups, more) = self.search_groups(&found, counts, all_groups);
            (groups, more, Vec::new())
        } else {
            (Vec::new(), None, listed(sort, found))
        };

        let searching = typed && server.is_some_and(ServerSearch::is_asking);
        let found_any = !hits.is_empty() || !groups.is_empty();
        // Nothing found says so in its own words, not as "0 results".
        let header = (typed && (found_any || searching)).then(|| {
            let scope = scope.as_deref().zip(scope_title.as_deref());
            header_said(scope, counts, (&hits, &groups, grouped), searching)
        });

        // The widening row, in one conversation: how many elsewhere, counted only where the
        // person may read, as the server counted; nothing offline.
        let elsewhere = (typed && online && !everywhere)
            .then(|| elsewhere(counts, server, scope.as_ref()))
            // Counted, and none elsewhere: no row.
            .filter(|row| row.text.is_some() || counts.is_none());
        let limits = limits(query, server, online);
        let empty = (typed && !found_any && !searching)
            .then(|| nothing_found(query, scope_title.as_deref(), elsewhere.as_ref()));

        let placeholder = scope_title.as_ref().map_or_else(
            || "Search everywhere".to_owned(),
            |title| format!("Search {title}"),
        );
        let filters = if everywhere && query.chips().is_empty() {
            vec![
                SearchFilter {
                    label: "In…".to_owned(),
                    token: "in:".to_owned(),
                },
                SearchFilter {
                    label: "From…".to_owned(),
                    token: "from:".to_owned(),
                },
            ]
        } else {
            Vec::new()
        };
        // Only the orders that can be given, as every app lists them: the server can't list
        // oldest first yet, so that one shows offline, or once typed (`sort:old`).
        let sorts = [Sort::Best, Sort::New, Sort::Old]
            .into_iter()
            .filter(|&choice| choice != Sort::Old || !online || sort == Sort::Old)
            .map(|choice| SortChoice {
                sort: choice,
                label: sort_name(choice).to_owned(),
                chosen: choice == sort,
            })
            .collect();
        let a11y = match (&header, &empty) {
            (_, Some(empty)) => empty.text.clone(),
            (Some(header), None) => format!("{header}, {}", sort_name(sort).to_lowercase()),
            (None, None) => String::new(),
        };
        SearchScreen {
            chips: screen_chips(query, scope_title.as_deref()),
            placeholder,
            header,
            sort,
            sort_label: format!("Sort: {}", sort_name(sort)),
            scope_title,
            sorts,
            groups,
            more_groups,
            hits,
            elsewhere,
            limits,
            empty,
            filters,
            more: typed && online && server.is_some_and(ServerSearch::more),
            searching,
            a11y,
            everywhere_unavailable: (!online).then_some(OFFLINE_EVERYWHERE),
            query: query.clone(),
        }
    }

    /// What a search for `query` found, in `scope` or everywhere: the history's hits
    /// (`server`) and what this device has loaded besides. Times read on the app's clock,
    /// `wall`.
    fn search_found(
        &self,
        query: &SearchQuery,
        scope: Option<&String>,
        server: Option<&ServerSearch>,
        wall: &impl Fn(DateTime<Utc>) -> NaiveDateTime,
    ) -> Found {
        let words = query.text.trim().trim_matches('"').to_owned();
        let typed = is_typed(query);
        // What's loaded, in each conversation searched.
        let asked = match &query.from {
            Some(from) => format!("from:{from} {}", query.text),
            None => query.text.clone(),
        };
        let names: Vec<String> = match scope {
            Some(name) => vec![name.clone()],
            None => self
                .conversations()
                .iter()
                .filter(|conversation| conversation.kind != Kind::Server)
                .map(|conversation| conversation.name.clone())
                .collect(),
        };
        let now = wall((self.clock)());
        // Everywhere by time, every conversation's hits are one list, each saying where.
        let listed = scope.is_none() && query.sort() != Sort::Best;
        let found = |name: &str, hit: &Hit, loaded: bool| SearchHit {
            loaded,
            ..self.search_hit(name, self.title(name), hit, &words, (wall, now), listed)
        };
        let mut loaded: Vec<SearchHit> = Vec::new();
        let mut loaded_ids: HashSet<String> = HashSet::new();
        if typed {
            for name in &names {
                for hit in &self.search(name, &asked).hits {
                    if let Some(id) = &hit.id {
                        loaded_ids.insert(id.clone());
                    }
                    loaded.push(found(name, hit, true));
                }
            }
        }
        // What the history found, in its order, those loaded marked so.
        let mut fetched: Vec<SearchHit> = Vec::new();
        if let Some(server) = server {
            for (conversation, hit) in &server.hits {
                let name = scope
                    .cloned()
                    .unwrap_or_else(|| self.local_name(conversation));
                let here = hit.id.as_ref().is_some_and(|id| loaded_ids.contains(id));
                fetched.push(found(&name, hit, here));
            }
        }
        let fetched_ids: HashSet<Option<String>> = fetched.iter().map(|f| f.id.clone()).collect();
        let only_loaded: Vec<SearchHit> = loaded
            .iter()
            .filter(|hit| hit.id.is_none() || !fetched_ids.contains(&hit.id))
            .cloned()
            .collect();
        Found {
            fetched,
            only_loaded,
        }
    }

    /// The hits under their conversations, everywhere by best match: the conversations in
    /// the order of their best match, the history's first, those shown unless `all_groups`,
    /// and the row that says how many more there are.
    fn search_groups(
        &self,
        found: &Found,
        counts: Option<&[(String, u64)]>,
        all_groups: bool,
    ) -> (Vec<SearchGroup>, Option<String>) {
        let mut groups = Vec::new();
        let mut more_groups = None;
        // Conversations in the order of their best match, the history's first.
        let mut order: Vec<String> = Vec::new();
        let mut by: HashMap<String, Vec<SearchHit>> = HashMap::new();
        let mut newest_first = found.only_loaded.clone();
        newest_first.sort_by_key(|hit| std::cmp::Reverse(hit.time_ms));
        for hit in found.fetched.iter().chain(&newest_first) {
            if !by.contains_key(&hit.conversation) {
                order.push(hit.conversation.clone());
            }
            by.entry(hit.conversation.clone())
                .or_default()
                .push(hit.clone());
        }
        let count_of = |name: &str, shown: usize| {
            counts
                .and_then(|counts| counts.iter().find(|(n, _)| n == name))
                .map_or(shown as u64, |(_, count)| *count)
        };
        let total_groups = counts.map_or(order.len(), |counts| counts.len().max(order.len()));
        let shown = if all_groups {
            order.len()
        } else {
            GROUPS_SHOWN
        };
        for name in order.into_iter().take(shown) {
            let mut its = by.remove(&name).unwrap_or_default();
            let count = count_of(&name, its.len());
            its.truncate(GROUP_HITS);
            its.sort_by_key(|hit| std::cmp::Reverse(hit.time_ms));
            let title = self.title(&name);
            groups.push(SearchGroup {
                count_a11y: format!("{count} in {title}: search only there"),
                hits_a11y: format!("{} in {title}", super::RESULTS),
                title,
                face: self
                    .conversation_face(&name)
                    .map(|avatar| self.face(&avatar)),
                count,
                hits: its,
                token: format!(
                    "in:{}",
                    if is_channel(&name) || agent(&name).is_some() {
                        name.clone()
                    } else {
                        format!("@{name}")
                    }
                ),
                conversation: name,
            });
        }
        let left = total_groups.saturating_sub(groups.len());
        if left > 0 {
            more_groups = Some(format!(
                "and {left} more {}",
                if left == 1 {
                    "conversation"
                } else {
                    "conversations"
                }
            ));
        }
        (groups, more_groups)
    }
}

/// What a search found.
struct Found {
    /// What the history found, in its order, those loaded marked so.
    fetched: Vec<SearchHit>,
    /// What only this device has.
    only_loaded: Vec<SearchHit>,
}

/// Whether `query` asks for anything yet: words, or someone's messages.
fn is_typed(query: &SearchQuery) -> bool {
    !query.text.trim().trim_matches('"').is_empty() || query.from.is_some()
}

/// The hits in one list, sorted as `sort` says, the first [`SHOWN`] of them.
fn listed(sort: Sort, found: Found) -> Vec<SearchHit> {
    let Found {
        fetched,
        only_loaded,
    } = found;
    let mut hits: Vec<SearchHit> = match sort {
        // The history's order, then what only this device has, newest first.
        Sort::Best if !fetched.is_empty() => {
            let mut rest = only_loaded.clone();
            rest.sort_by_key(|hit| std::cmp::Reverse(hit.time_ms));
            fetched.iter().cloned().chain(rest).collect()
        }
        _ => {
            let mut all: Vec<SearchHit> = fetched.iter().cloned().chain(only_loaded).collect();
            all.sort_by_key(|hit| std::cmp::Reverse(hit.time_ms));
            if sort == Sort::Old {
                all.reverse();
            }
            all
        }
    };
    hits.truncate(SHOWN);
    hits
}

/// How many the server counted in the conversation `name`.
fn here_count(counts: Option<&[(String, u64)]>, name: &str) -> Option<u64> {
    counts.and_then(|counts| {
        counts
            .iter()
            .find(|(n, _)| bare(n) == bare(name))
            .map(|(_, c)| *c)
    })
}

/// The header: how many were found, in `scope` (its name and title) or in how many
/// conversations, or that it's still searching.
fn header_said(
    scope: Option<(&str, &str)>,
    counts: Option<&[(String, u64)]>,
    (hits, groups, grouped): (&[SearchHit], &[SearchGroup], bool),
    searching: bool,
) -> String {
    if let Some((name, title)) = scope {
        let n = here_count(counts, name)
            .filter(|_| agent(name).is_none())
            .unwrap_or(hits.len() as u64);
        if searching && n == 0 {
            SEARCHING.to_owned()
        } else {
            format!("{} in {title}", results(n))
        }
    } else {
        let (n, places) = match counts {
            Some(counts) if !counts.is_empty() => (
                counts.iter().map(|(_, c)| c).sum(),
                counts.len().max(groups.len()),
            ),
            _ if grouped => (groups.iter().map(|group| group.count).sum(), groups.len()),
            _ => {
                let places: HashSet<&str> =
                    hits.iter().map(|hit| hit.conversation.as_str()).collect();
                (hits.len() as u64, places.len())
            }
        };
        if searching && n == 0 {
            SEARCHING.to_owned()
        } else {
            format!("{} in {}", results(n), conversations(places))
        }
    }
}

/// The widening row, in one conversation: how many the server counted elsewhere.
fn elsewhere(
    counts: Option<&[(String, u64)]>,
    server: Option<&ServerSearch>,
    scope: Option<&String>,
) -> Elsewhere {
    let server_scope = server
        .and_then(|server| server.server.as_ref())
        .map(|(name, _)| bare(name));
    let text = counts.and_then(|counts| {
        let others: Vec<&(String, u64)> = counts
            .iter()
            .filter(|(name, _)| Some(bare(name)) != server_scope && Some(name) != scope)
            .collect();
        let n: u64 = others.iter().map(|(_, c)| c).sum();
        (n > 0).then(|| {
            format!(
                "Also {n} in {} other {}",
                others.len(),
                if others.len() == 1 {
                    "conversation"
                } else {
                    "conversations"
                }
            )
        })
    });
    Elsewhere {
        text,
        action: SEARCH_EVERYWHERE,
    }
}

/// What the search couldn't cover: offline, the server unreachable or still indexing, a
/// filter it doesn't search yet, or that oldest first is of what's been found.
fn limits(
    query: &SearchQuery,
    server: Option<&ServerSearch>,
    online: bool,
) -> Option<SearchLimits> {
    let typed = is_typed(query);
    let sort = query.sort();
    let mut return_old = false;
    let limits = if !online {
        Some(SearchLimits {
            text: "Offline: searching only what this device has loaded.".to_owned(),
            warning: true,
        })
    } else if server.is_some_and(|server| server.failed) {
        Some(SearchLimits {
            text: "Couldn't reach the server: these are what this device has loaded.".to_owned(),
            warning: true,
        })
    } else if typed && server.and_then(|server| server.indexed) == Some(false) {
        Some(SearchLimits {
            text: "Still indexing: some older messages may be missing.".to_owned(),
            warning: true,
        })
    } else {
        let ignored: Vec<String> = query
            .agent
            .iter()
            .map(|agent| format!("agent:{agent}"))
            .chain(query.has.iter().map(|has| format!("has:{has}")))
            .chain(
                query
                    .kinds
                    .iter()
                    .filter(|kind| *kind != "event" && *kind != "message")
                    .map(|kind| format!("type:{kind}")),
            )
            .collect();
        if ignored.is_empty() && typed && sort == Sort::Old {
            return_old = true;
        }
        (!ignored.is_empty()).then(|| SearchLimits {
            text: format!(
                "{} isn't searched yet: these matches don't check it.",
                ignored.join(", ")
            ),
            warning: false,
        })
    };

    // Oldest first is of what's been found: the server pages newest first.
    limits.or_else(|| {
        return_old.then(|| SearchLimits {
            text: "Oldest first among what's found so far: the server lists newest first."
                .to_owned(),
            warning: false,
        })
    })
}

/// That nothing matches, in `scope_title` or anywhere, and what to try.
fn nothing_found(
    query: &SearchQuery,
    scope_title: Option<&str>,
    elsewhere: Option<&Elsewhere>,
) -> SearchEmpty {
    let shown_words = query.text.trim();
    match scope_title {
        Some(title) if shown_words.is_empty() => SearchEmpty {
            text: format!("Nothing in {title} matches."),
            hint: None,
        },
        Some(title) => SearchEmpty {
            text: format!(
                "Nothing in {title} matches “{}”.",
                shown_words.trim_matches('"')
            ),
            hint: elsewhere
                .as_ref()
                .and_then(|row| row.text.as_ref())
                .is_none()
                .then(|| TRY_OTHER_WORDS.to_owned()),
        },
        None => SearchEmpty {
            text: if shown_words.is_empty() {
                "Nothing you can read matches.".to_owned()
            } else {
                format!(
                    "Nothing you can read matches “{}”.",
                    shown_words.trim_matches('"')
                )
            },
            hint: Some(TRY_OTHER_WORDS.to_owned()),
        },
    }
}

/// The chips in the box: the scope by its title, and each filter.
fn screen_chips(query: &SearchQuery, scope_title: Option<&str>) -> Vec<ScreenChip> {
    query
        .chips()
        .into_iter()
        .map(|chip| {
            let scope_chip = chip.token.starts_with("in:");
            let label = if scope_chip {
                format!(
                    "in {}",
                    scope_title.map_or_else(|| chip.label.clone(), str::to_owned)
                )
            } else {
                chip.label
            };
            ScreenChip {
                a11y: if scope_chip {
                    format!("Searching {label}. Remove to search everywhere")
                } else {
                    format!("Filter {label}. Remove it")
                },
                label,
                token: chip.token,
                scope: scope_chip,
                removable: true,
            }
        })
        .collect()
}

impl Chat {
    /// Where a hit in the conversation `name`, said by `from`, was, as a [`SearchHit::aside`]
    /// in one list of every conversation's hits says it: the conversation as shown for a
    /// channel or an agent's; for direct messages, `DM` when the other person said it and
    /// `to Leo Okafor` when you did, so the name isn't said twice.
    pub(super) fn hit_place(&self, name: &str, from: &str) -> String {
        let title = self.title(name);
        if is_channel(name) || agent(name).is_some() {
            title
        } else if fold(from) == fold(self.me()) {
            format!("to {title}")
        } else {
            "DM".to_owned()
        }
    }

    /// A conversation's face in a search's group: a person's for direct messages, the
    /// agents' for an agent or a machine; none for a channel.
    fn conversation_face(&self, name: &str) -> Option<Avatar> {
        if is_channel(name) {
            None
        } else if agent(name).is_some() {
            Some(Self::agents_avatar())
        } else {
            Some(self.sender_avatar(name))
        }
    }
}

/// How many searches [`RecentSearches`] keeps.
pub const RECENT_SEARCHES: usize = 10;

/// The searches made lately, newest first, as their text (chips and sort included), at most
/// [`RECENT_SEARCHES`], each once; the app keeps them per organisation, on the device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentSearches(pub Vec<String>);

impl RecentSearches {
    /// Remembers `query`, made now, unless nothing was searched for.
    pub fn remember(&mut self, query: &SearchQuery) {
        if query.text.trim().is_empty() && query.from.is_none() {
            return;
        }
        let text = query.to_string();
        self.0.retain(|kept| *kept != text);
        self.0.insert(0, text);
        self.0.truncate(RECENT_SEARCHES);
    }

    /// Forgets them all.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Each as the search it was, those that no longer read left out.
    #[must_use]
    pub fn queries(&self) -> Vec<SearchQuery> {
        self.0
            .iter()
            .filter_map(|text| SearchQuery::parse(text, None).ok())
            .collect()
    }

    /// Them as the empty search box lists them, in every app: each its chips' labels and its
    /// words, those that no longer read left out.
    #[must_use]
    pub fn listed(&self) -> Vec<Recent> {
        self.0
            .iter()
            .filter_map(|said| {
                let query = SearchQuery::parse(said, None).ok()?;
                Some(Recent {
                    query: said.clone(),
                    chips: query.chips().into_iter().map(|chip| chip.label).collect(),
                    text: query.text,
                })
            })
            .collect()
    }
}

/// What the recent searches' list is called, in every app.
pub const RECENT_TITLE: &str = "Recent searches";

/// What clears the recent searches, in every app.
pub const CLEAR_RECENT: &str = "Clear recent searches";

/// A search made lately, as the empty box lists it ([`RecentSearches::listed`]): the phones
/// read it through their core, the web chat as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Recent {
    /// The whole search, chips and sort included, which [`Finding::recall`] takes.
    pub query: String,
    /// Its chips' labels: `in #api`.
    pub chips: Vec<String>,
    /// Its words.
    pub text: String,
}

impl SearchScreen {
    /// The hit Return in the search box opens, in every app: the first listed, or the
    /// first conversation's first.
    #[must_use]
    pub fn first(&self) -> Option<&SearchHit> {
        self.hits
            .first()
            .or_else(|| self.groups.first().and_then(|group| group.hits.first()))
    }
}
