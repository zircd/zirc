//! Search in one conversation, for every app: what's loaded matches at once, newest first,
//! then older matches a page at a time from the organisation's history, which zirc.dev
//! serves as its visibility policy lets the person read ([`zirc_proto::login::HISTORY_PATH`],
//! case-insensitive). Each hit has the message's id, to jump to it and highlight it in
//! context. The asking is the app's: [`Search::server_query`] says what to ask next, and
//! [`Search::take_page`] takes the answer.

use std::collections::HashSet;
use std::ops::Range;

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use zirc_proto::login::HistoryPage;

use super::conversation::{EntryKind, Kind, agent, fold, is_channel};
use serde::Serialize;
use ts_rs::TS;

use super::{Chat, Face, describe};

/// How far back the server is asked to look: as far as it reads.
const REACH: Duration = Duration::days(400);

/// How many hits a [`SearchView`] shows.
const SHOWN: usize = 100;

/// A message that matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The server's id for it, to jump to; none for one the server didn't name.
    pub id: Option<String>,
    /// When it was said.
    pub time: DateTime<Utc>,
    /// Who said it: a nick.
    pub from: String,
    /// The agent session that said it, for a machine's message.
    pub session: Option<String>,
    /// What it says, as people read it.
    pub text: String,
    /// Whether it's in the conversation as loaded, to jump to at once; one from the server
    /// is older than what's loaded, and the app loads history back to it first.
    pub loaded: bool,
}

/// What to ask the organisation's history for next: the query of [`HISTORY_PATH`].
///
/// [`HISTORY_PATH`]: zirc_proto::login::HISTORY_PATH
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerQuery {
    /// The conversation as the server names it: `#channel`, or `@account` for direct
    /// messages with them.
    pub conversation: String,
    /// The text to find.
    pub text: String,
    /// Said by this nick or account (`from:`), if the search says.
    pub sender: Option<String>,
    /// From when: as far back as the server reads.
    pub since: DateTime<Utc>,
    /// Until when: before the oldest loaded message, as what's loaded was searched already.
    pub until: Option<DateTime<Utc>>,
    /// Where the page before ended.
    pub cursor: Option<String>,
}

impl ServerQuery {
    /// Its query string's pairs, `limit` included.
    #[must_use]
    pub fn params(&self, limit: usize) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("conversation", self.conversation.clone()),
            ("q", self.text.clone()),
            ("since", self.since.to_rfc3339()),
            ("limit", limit.to_string()),
        ];
        if let Some(until) = self.until {
            params.push(("until", until.to_rfc3339()));
        }
        if let Some(cursor) = &self.cursor {
            params.push(("cursor", cursor.clone()));
        }
        if let Some(sender) = &self.sender {
            params.push(("sender", sender.clone()));
        }
        params
    }
}

/// A search in one conversation: its hits so far, newest first.
#[derive(Debug, Clone)]
pub struct Search {
    /// The conversation searched.
    pub conversation: String,
    /// What's searched for, as typed.
    pub query: String,
    /// The matches so far, newest first.
    pub hits: Vec<Hit>,
    /// The words to find and whose messages, as [`needle`] reads `query`.
    wanted: (String, Option<String>),
    /// Whether there's nothing more to ask the server for.
    pub done: bool,
    /// The conversation as the server names it, and the agent session whose it is.
    server: Option<(String, Option<String>)>,
    /// The oldest loaded message's time.
    oldest: Option<DateTime<Utc>>,
    /// Where the next page starts.
    cursor: Option<String>,
    /// The ids found, so a page doesn't repeat them.
    seen: HashSet<String>,
    /// When it started, for how far back to ask.
    started: DateTime<Utc>,
}

impl Search {
    /// What to ask the server for next, or `None` when it's all been found.
    #[must_use]
    pub fn server_query(&self) -> Option<ServerQuery> {
        if self.done {
            return None;
        }
        let (conversation, _) = self.server.clone()?;
        Some(ServerQuery {
            conversation,
            text: self.wanted.0.clone(),
            sender: self.wanted.1.clone(),
            since: self.started - REACH,
            until: self.oldest,
            cursor: self.cursor.clone(),
        })
    }

    /// Takes in a page the server gave for [`Search::server_query`]: its matches, older than
    /// those found, are added, and it's done when there's no page before.
    pub fn take_page(&mut self, page: &HistoryPage) {
        let session = self
            .server
            .as_ref()
            .and_then(|(_, session)| session.clone());
        for message in &page.messages {
            let said_by = message
                .zirc
                .as_ref()
                .and_then(|zirc| zirc.get("session"))
                .and_then(|session| session.as_str())
                .map(str::to_owned);
            // An agent's conversation is the session's own lines in the machine's.
            if session.is_some() && said_by != session {
                continue;
            }
            if !self.seen.insert(message.msgid.clone()) {
                continue;
            }
            let Ok(time) = DateTime::parse_from_rfc3339(&message.time) else {
                continue;
            };
            self.hits.push(Hit {
                id: Some(message.msgid.clone()),
                time: time.with_timezone(&Utc),
                from: message.sender.nick.clone(),
                session: said_by,
                text: message.text.clone(),
                loaded: false,
            });
        }
        self.cursor.clone_from(&page.next);
        self.done = page.next.is_none();
    }
}

/// What every app's search offers under the matches in one conversation whose history may
/// have more ([`SearchView::more`]), to ask for them.
pub const SEARCH_OLDER: &str = "Search older messages";

/// What that says while the history is asked.
pub const SEARCHING_OLDER: &str = super::SEARCHING;

/// A search as every app shows it beside the conversation: in one conversation (an `in:`
/// chip) or in every one, what's loaded and what the history gave, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchView {
    /// What's searched for.
    pub query: String,
    /// The conversation it's in, by name; none when it's everywhere.
    pub scope: Option<String>,
    /// That conversation as shown.
    pub scope_title: Option<String>,
    /// The matches, newest first, at most a hundred.
    pub hits: Vec<SearchHit>,
    /// Whether the organisation's history may have older ones, for the app to ask for with
    /// [`Search::server_query`].
    pub more: bool,
}

impl SearchView {
    /// What a search that matched nothing says, as every app says it: that older messages
    /// may have it, below, when the history may; else to try other words.
    #[must_use]
    pub fn empty_hint(&self) -> &'static str {
        if self.more {
            "Older messages may have it: search them below."
        } else {
            super::finder::TRY_OTHER_WORDS
        }
    }
}

/// A match, as a [`SearchView`] shows it: ready to draw and send as it is, every app reads
/// this record, the phones through `UniFFI` and web as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchHit {
    /// The conversation it's in, by name, to open.
    pub conversation: String,
    /// That conversation as shown.
    pub title: String,
    /// The server's id for it, to jump to; none for one the server didn't name.
    pub id: Option<String>,
    /// When it was said, in milliseconds since 1970.
    pub time_ms: i64,
    /// That on the app's clock: [`super::clock`], `15:48`.
    pub clock: String,
    /// Who said it: their nick.
    pub from: String,
    /// What the match's head calls them, as a message's does: [`Chat::sender_name`], an
    /// agent session by its name, a person as they show.
    pub name: String,
    /// What it says, in pieces: the matches marked, to highlight.
    pub parts: Vec<HitPart>,
    /// Whether it's in the conversation as loaded, to jump to; an older one from the
    /// history opens its conversation only.
    pub loaded: bool,
    /// Who said it as a row's avatar shows them, beside the name: [`Chat::sender_avatar`].
    pub face: Face,
    /// Where it was, as a hit in a list of every conversation says it: the conversation as
    /// shown for a channel or an agent's; for direct messages, `DM` when the other person
    /// said it and `to Leo Okafor` when you did, so the name isn't said twice.
    pub place: String,
    /// Its day and time, as every list says it: `15:48` today, `Sat 08:00` within the
    /// week, `2 Nov`, `2 Nov 2025` ([`super::list_when`]).
    pub when: String,
    /// What every app draws beside the name, muted: its [`Self::when`], and in a list of
    /// every conversation's hits where it was before it, `#api · Sat 08:00`.
    pub aside: String,
    /// What a screen reader says of it: "Leo Okafor in #api, 9:14: the gateway's upstream
    /// timeout is 30s".
    pub a11y: String,
}

/// A piece of a match's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct HitPart {
    /// The words.
    pub text: String,
    /// Whether they're what was searched for.
    pub hit: bool,
}

impl Chat {
    /// `hit`, in the conversation `name` shown as `title`, as a search shows it: `words`
    /// marked, times on the clock `wall` with `now` its present.
    pub(super) fn search_hit(
        &self,
        name: &str,
        title: String,
        hit: &Hit,
        words: &str,
        wall: impl Fn(DateTime<Utc>) -> NaiveDateTime,
        now: NaiveDateTime,
    ) -> SearchHit {
        let parts = parts(&hit.text, &highlights(&hit.text, words));
        let said: String = parts.iter().map(|part| part.text.as_str()).collect();
        let speaker = self.sender_name(&hit.from, hit.session.as_deref());
        let when = super::list_when(wall(hit.time), now);
        SearchHit {
            a11y: format!("{speaker} in {title}, {when}: {said}"),
            conversation: name.to_owned(),
            title,
            id: hit.id.clone(),
            time_ms: hit.time.timestamp_millis(),
            clock: super::clock(wall(hit.time)),
            from: hit.from.clone(),
            name: speaker,
            parts,
            loaded: hit.loaded,
            face: self.face(&self.sender_avatar(&hit.from)),
            place: self.hit_place(name, &hit.from),
            aside: when.clone(),
            when,
        }
    }
}

/// `text` in pieces, those in `ranges` marked.
pub(super) fn parts(text: &str, ranges: &[Range<usize>]) -> Vec<HitPart> {
    let mut parts = Vec::new();
    let mut at = 0;
    for range in ranges {
        if range.start > at {
            parts.push(HitPart {
                text: text[at..range.start].to_owned(),
                hit: false,
            });
        }
        parts.push(HitPart {
            text: text[range.clone()].to_owned(),
            hit: true,
        });
        at = range.end;
    }
    if at < text.len() {
        parts.push(HitPart {
            text: text[at..].to_owned(),
            hit: false,
        });
    }
    parts
}

/// The words a search typed in one conversation finds, and whose messages (`from:`), read as
/// every app, the TUI and the CLI read a search ([`zirc_proto::search::Query`]). Its other
/// tokens narrow a search across conversations, not one conversation's; text that isn't a
/// search, such as `after:yesterday`, is looked for as it is.
fn needle(query: &str, conversation: &str) -> (String, Option<String>) {
    match zirc_proto::search::Query::parse(query, Some(conversation)) {
        Ok(parsed) => (
            parsed.text.trim().trim_matches('"').to_owned(),
            parsed.from.map(|from| fold(&from)),
        ),
        Err(_) => (query.trim().to_owned(), None),
    }
}

/// Where `query` is in `text`, ignoring case, as byte ranges to highlight; none for an
/// empty query.
#[must_use]
pub fn highlights(text: &str, query: &str) -> Vec<Range<usize>> {
    let wanted: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        // How many of the text's chars, from `at`, spell `wanted` lowercased, if they do.
        let mut lowered = Vec::new();
        let mut spans = 0;
        for &(_, c) in &chars[at..] {
            if lowered.len() >= wanted.len() {
                break;
            }
            lowered.extend(c.to_lowercase());
            spans += 1;
        }
        if lowered == wanted {
            let end = chars.get(at + spans).map_or(text.len(), |&(byte, _)| byte);
            found.push(chars[at].0..end);
            at += spans;
        } else {
            at += 1;
        }
    }
    found
}

impl Chat {
    /// `query` searched for in the conversation called `scope`, or in every one but the
    /// server's when there's none, as every app shows it: what's loaded, and for one
    /// conversation what `older` found in the organisation's history before it (the
    /// [`Chat::search`] the app keeps, asking with [`Search::server_query`] and taking pages
    /// with [`Search::take_page`]). Times read on the app's clock, `wall`.
    #[must_use]
    pub fn search_view(
        &self,
        query: &str,
        scope: Option<&str>,
        older: Option<&Search>,
        wall: impl Fn(DateTime<Utc>) -> NaiveDateTime,
    ) -> SearchView {
        let names: Vec<String> = match scope {
            Some(name) => vec![name.to_owned()],
            None => self
                .conversations()
                .iter()
                .filter(|conversation| conversation.kind != Kind::Server)
                .map(|conversation| conversation.name.clone())
                .collect(),
        };
        let now = wall((self.clock)());
        let found = |name: &str, title: &str, hit: &Hit| {
            self.search_hit(
                name,
                title.to_owned(),
                hit,
                &needle(query, name).0,
                &wall,
                now,
            )
        };
        let mut hits: Vec<SearchHit> = Vec::new();
        for name in &names {
            let title = self.title(name);
            hits.extend(
                self.search(name, query)
                    .hits
                    .iter()
                    .map(|hit| found(name, &title, hit)),
            );
        }
        if let (Some(older), Some(name)) = (older, scope) {
            let title = self.title(name);
            let ids: HashSet<Option<String>> = hits.iter().map(|hit| hit.id.clone()).collect();
            hits.extend(
                older
                    .hits
                    .iter()
                    .filter(|hit| !hit.loaded && !ids.contains(&hit.id))
                    .map(|hit| found(name, &title, hit)),
            );
        }
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.time_ms));
        hits.truncate(SHOWN);
        SearchView {
            query: query.to_owned(),
            scope: scope.map(str::to_owned),
            scope_title: scope.map(|name| self.title(name)),
            hits,
            more: older.is_some_and(|older| older.server_query().is_some()),
        }
    }

    /// Searches the conversation called `name` for `query`, ignoring case: what's loaded
    /// matches at once, newest first; [`Search::server_query`] says how to find older.
    #[must_use]
    pub fn search(&self, name: &str, query: &str) -> Search {
        let query = query.trim().to_owned();
        let wanted = needle(&query, name);
        let (text_wanted, from) = (&wanted.0, &wanted.1);
        let conversation = self.conversation(name);
        let mut hits = Vec::new();
        let mut seen = HashSet::new();
        let asks = !text_wanted.is_empty() || from.is_some();
        if asks && let Some(conversation) = conversation {
            for entry in conversation.entries.iter().rev() {
                let said = matches!(
                    entry.kind,
                    EntryKind::Message(_)
                        | EntryKind::Action(_)
                        | EntryKind::Notice(_)
                        | EntryKind::Zirc(_)
                );
                // A session's title card is what its card shows, never something said.
                if !said || entry.is_session_card() {
                    continue;
                }
                let (text, session) = match &entry.kind {
                    EntryKind::Message(text)
                    | EntryKind::Action(text)
                    | EntryKind::Notice(text) => (text.clone(), None),
                    EntryKind::Zirc(message) => (
                        describe::describe(&message.event).headline,
                        Some(message.session.clone()).filter(|s| !s.is_empty()),
                    ),
                    _ => continue,
                };
                // What it says, or else what its data carries ([`zirc_proto::Message::search_text`]).
                let found = |text: &str| !highlights(text, text_wanted).is_empty();
                let matches = text_wanted.is_empty()
                    || found(&text)
                    || matches!(&entry.kind, EntryKind::Zirc(message) if found(&message.search_text()));
                if from.as_ref().is_some_and(|from| fold(&entry.from) != *from) || !matches {
                    continue;
                }
                if let Some(id) = &entry.id {
                    seen.insert(id.clone());
                }
                hits.push(Hit {
                    id: entry.id.clone(),
                    time: entry.time,
                    from: entry.from.clone(),
                    session,
                    text,
                    loaded: true,
                });
            }
        }
        let server = conversation
            .filter(|c| c.kind != Kind::Server && asks)
            .map(|c| match agent(&c.name) {
                Some((machine, session)) => (format!("@{machine}"), Some(session.to_owned())),
                None if is_channel(&c.name) => (c.name.clone(), None),
                None => (format!("@{}", c.name), None),
            });
        let oldest = conversation.and_then(|c| {
            c.entries
                .iter()
                .filter(|entry| entry.id.is_some())
                .map(|entry| entry.time)
                .min()
        });
        Search {
            conversation: name.to_owned(),
            done: server.is_none(),
            query,
            hits,
            wanted,
            server,
            oldest,
            cursor: None,
            seen,
            started: (self.clock)(),
        }
    }
}

/// How well what's typed in the quick switcher, `query`, matches a conversation shown as
/// `title`, with `note` beside it (what an agent is taking on, whose machine it is), ignoring
/// case and a leading `#` or `@`: `0` when its title starts with it, `1` when a word of its
/// title does, `2` when every word typed is somewhere in its title or note; `None` when it
/// doesn't match. Lower is better, and nothing typed matches everything as `0`. Every app's
/// quick switcher ranks with this one ([`Chat::switcher`]).
#[must_use]
pub fn switch_rank(query: &str, title: &str, note: Option<&str>) -> Option<u8> {
    let bare = |text: &str| fold(text.trim().trim_start_matches(['#', '@']));
    let query = bare(query);
    let plain = bare(title);
    if query.is_empty() || plain.starts_with(&query) {
        return Some(0);
    }
    if plain
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(&query))
    {
        return Some(1);
    }
    let haystack = format!("{plain} {}", fold(note.unwrap_or_default()));
    query
        .split_whitespace()
        .all(|word| haystack.contains(word))
        .then_some(2)
}

/// How many conversations Jump to lists at most, in every app (the user's choice, 16 Nov
/// 2026).
pub const JUMP_ROWS: usize = 20;

impl Chat {
    /// The conversations Jump to offers for `query`, by name, at most [`JUMP_ROWS`]. With
    /// nothing typed, the most recent first ([`Chat::last_said`]), those with nothing said by
    /// what they show; else best first, as [`switch_rank`] ranks their titles and an agent
    /// session's task, then what's unread ([`Chat::is_unread`]), then in the order of the
    /// conversations. The server's only for what's typed: it has no row in the sidebar, so
    /// Jump to is where it's found (the user's choice, 19 Nov 2026), never among the recent.
    #[must_use]
    pub fn switcher(&self, query: &str) -> Vec<String> {
        let typed = !query.trim().is_empty();
        let names = self
            .conversations()
            .iter()
            .filter(|conversation| typed || conversation.kind != Kind::Server)
            .map(|conversation| conversation.name.as_str());
        if !typed {
            let mut recent: Vec<&str> = names.collect();
            recent.sort_by_cached_key(|name| {
                (
                    std::cmp::Reverse(self.last_said(name)),
                    fold(&self.title(name)),
                )
            });
            return recent
                .into_iter()
                .take(JUMP_ROWS)
                .map(str::to_owned)
                .collect();
        }
        let mut found: Vec<(u8, bool, usize, &str)> = names
            .enumerate()
            .filter_map(|(at, name)| {
                let task = self.agent(name).and_then(|agent| agent.task);
                let rank = switch_rank(query, &self.title(name), task.as_deref())?;
                Some((rank, !self.is_unread(name), at, name))
            })
            .collect();
        found.sort_unstable();
        found
            .into_iter()
            .take(JUMP_ROWS)
            .map(|(.., name)| name.to_owned())
            .collect()
    }
}
