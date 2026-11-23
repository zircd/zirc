//! A tenant's message history, for zirc.dev to search on someone's behalf: Ergo's own SQLite
//! history on the tenant's disk, read-only, so it holds exactly what Ergo keeps (the
//! organisation's retention) and goes with the organisation. Who may read which conversation
//! is the organisation's choice (`Visibility`).
//!
//! Ergo keeps a channel message in `sequence`, under the channel; and a direct message in
//! `conversations`, once for each of its two accounts, under that account with the other as
//! its correspondent. Both by time, in nanoseconds, so each source is read newest first, a
//! batch at a time, and the two merged.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::Path;

use jiff::Timestamp;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::exporter::{self, PRIVMSG};

mod by_id;
pub use by_id::by_ids;
use zirc_proto::desired::Visibility;

/// A conversation: a channel, or the direct messages between two accounts (in order).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Conversation {
    /// A channel, by its name.
    Channel(String),
    /// A direct conversation between two accounts.
    Direct(String, String),
}

impl Conversation {
    /// `#channel`, `@account` (the viewer's direct messages with it), or `@a,@b`.
    #[must_use]
    pub fn parse(text: &str, viewer: &str) -> Option<Self> {
        let text = text.trim().to_lowercase();
        if text.starts_with('#') && !text.contains(',') {
            return Some(Self::Channel(text));
        }
        let mut accounts = text.split(',').map(|a| a.trim().strip_prefix('@'));
        match (accounts.next()?, accounts.next(), accounts.next()) {
            (Some(a), None, None) => Some(Self::direct(viewer, a)),
            (Some(a), Some(Some(b)), None) => Some(Self::direct(a, b)),
            _ => None,
        }
    }

    fn direct(a: &str, b: &str) -> Self {
        let (a, b) = (a.to_lowercase(), b.to_lowercase());
        if a <= b {
            Self::Direct(a, b)
        } else {
            Self::Direct(b, a)
        }
    }
}

impl std::fmt::Display for Conversation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Channel(name) => f.write_str(name),
            Self::Direct(a, b) => write!(f, "@{a},@{b}"),
        }
    }
}

/// Who's asking, as zirc.dev says: a person, for themselves or for one of their agents, with
/// the channels Ergo has them or any of their machines in.
pub struct Viewer {
    /// The viewer's account.
    pub account: String,
    /// The accounts of the viewer's machines, and of their own clients under another name,
    /// as the server was told whose each is ([`crate::machines::Machines`]): never guessed
    /// from a name, as a machine's says nothing of whose it is.
    pub machines: BTreeSet<String>,
    /// The channels the viewer is in.
    pub channels: BTreeSet<String>,
    /// Who may read what, as the organisation chose.
    pub policy: Visibility,
}

impl Viewer {
    /// Whether the viewer may see `conversation`, by the organisation's choice.
    #[must_use]
    pub fn may_see(&self, conversation: &Conversation) -> bool {
        match (self.policy, conversation) {
            (Visibility::Participant, Conversation::Channel(name)) => self.channels.contains(name),
            (Visibility::Channels | Visibility::Everything, Conversation::Channel(_))
            | (Visibility::Everything, Conversation::Direct(..)) => true,
            (_, Conversation::Direct(a, b)) => self.owns(a) || self.owns(b),
        }
    }

    /// The viewer's own account, or one of their machines'.
    #[must_use]
    pub fn owns(&self, account: &str) -> bool {
        account == self.account || self.machines.contains(account)
    }

    /// The viewer's machines' accounts as a JSON list, for a query's `json_each`.
    fn machines_json(&self) -> String {
        serde_json::to_string(&self.machines).unwrap_or_else(|_| "[]".to_owned())
    }
}

/// What to look for.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// What to read, or everything the viewer may see.
    pub conversation: Option<Conversation>,
    /// Sent by this account.
    pub sender: Option<String>,
    /// Containing this, any case.
    pub text: Option<String>,
    /// Nanoseconds since the epoch: at or after `since`, before `until`.
    pub since: i64,
    /// Only what was said before this time, in nanoseconds since the Unix epoch.
    pub until: i64,
    /// The most messages to return.
    pub limit: usize,
    /// Older than this point, from the page before's `next`.
    pub before: Option<Cursor>,
    /// zircd's automatic events and notices too, not only what people and agents say. A
    /// search for text finds them anyway ([`Query::wants_events`]).
    pub events: bool,
    /// The best matches first (FTS5's bm25, then the newest), not the newest first. Only for
    /// text the index can find; otherwise newest first.
    pub best: bool,
    /// How many best matches to skip, from the page before's `next` (`b<offset>`).
    pub skip: usize,
}

impl Query {
    /// Whether zircd's events and notices count: when asked for, and in any search for text,
    /// which finds them by their data too ([`zirc_proto::Message::search_text`]).
    #[must_use]
    pub fn wants_events(&self) -> bool {
        self.events || self.text.is_some()
    }
}

/// A point in the history: a message's time and id, which together order everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor {
    /// The message's time, in nanoseconds since the Unix epoch.
    pub nanotime: i64,
    /// The row's id in Ergo's history.
    pub id: i64,
}

impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.nanotime, self.id)
    }
}

impl std::str::FromStr for Cursor {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let (nanotime, id) = text.split_once('-').ok_or(())?;
        Ok(Self {
            nanotime: nanotime.parse().map_err(|_| ())?,
            id: id.parse().map_err(|_| ())?,
        })
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
/// Who sent a message.
pub struct Sender {
    /// The account it was sent from, if any.
    pub account: Option<String>,
    /// The nick it was sent under.
    pub nick: String,
    /// `person` or `agent`, by the account (`crate::usage`).
    pub kind: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
/// A message, as the search returns it.
pub struct Message {
    /// Its id on the server.
    pub msgid: String,
    /// When it was sent, RFC 3339.
    pub time: String,
    /// The conversation it was in.
    pub conversation: String,
    /// Who sent it.
    pub sender: Sender,
    #[serde(rename = "type")]
    /// What kind of line it is.
    pub kind: &'static str,
    /// As people read it: a zirc message's own text.
    pub text: String,
    /// The zirc event it carries, if it's one, with its session, and the task it's part of
    /// and its level ([`zirc_proto::Message::task`], [`zirc_proto::Message::level`]) where
    /// it says them: no event has fields of those names.
    pub zirc: Option<Value>,
}

/// A page of what a viewer may see that a query asks for, newest first, and where the next
/// page starts if there may be more.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Page {
    /// The messages found, newest first.
    pub messages: Vec<Message>,
    /// Where to continue from, if there's more.
    pub next: Option<String>,
    /// Whether the index answered (`crate::index`): then [`counts`] can too.
    pub indexed: bool,
}

/// How many messages a query finds in one conversation.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Count {
    /// The conversation, as [`Message::conversation`] names it.
    pub conversation: String,
    /// How many messages in it match.
    pub count: u64,
}

/// How many rows a search reads at most for one page, so one that matches little stays
/// bounded; its `next` carries on from where it stopped.
pub const SCAN: usize = 20_000;
const BATCH: usize = 500;

/// One of the two places messages are kept, read newest first a batch at a time.
struct Source<'a> {
    connection: &'a Connection,
    sql: String,
    rows: VecDeque<Row>,
    from: Option<Cursor>,
    done: bool,
    since: i64,
    until: i64,
    /// The values the SQL's `?4` and `?5` narrow it by, if it uses them.
    narrow: (String, String),
    /// The SQL's `?7` on: the index's text and filters, or the newest history id the index
    /// has, for a scan of only what's newer.
    extra: Vec<rusqlite::types::Value>,
}

struct Row {
    at: Cursor,
    msgid: String,
    data: Vec<u8>,
    conversation: Conversation,
}

impl Source<'_> {
    fn peek(&mut self) -> rusqlite::Result<Option<&Row>> {
        if self.rows.is_empty() && !self.done {
            self.fill()?;
        }
        Ok(self.rows.front())
    }

    fn fill(&mut self) -> rusqlite::Result<()> {
        let from = self.from.unwrap_or(Cursor {
            nanotime: self.until,
            id: 0,
        });
        let mut statement = self.connection.prepare_cached(&self.sql)?;
        let first: [rusqlite::types::Value; 6] = [
            self.since.into(),
            from.nanotime.into(),
            from.id.into(),
            self.narrow.0.clone().into(),
            self.narrow.1.clone().into(),
            i64::try_from(BATCH).unwrap_or(i64::MAX).into(),
        ];
        let mut rows =
            statement.query(rusqlite::params_from_iter(first.iter().chain(&self.extra)))?;
        let mut read = 0;
        while let Some(row) = rows.next()? {
            read += 1;
            let at = Cursor {
                nanotime: row.get(0)?,
                id: row.get(1)?,
            };
            let (target, other): (String, Option<String>) = (row.get(4)?, row.get(5)?);
            self.rows.push_back(Row {
                at,
                msgid: row.get(2)?,
                data: row.get(3)?,
                conversation: match other {
                    None => Conversation::Channel(target),
                    Some(other) => Conversation::direct(&target, &other),
                },
            });
            self.from = Some(at);
        }
        self.done = read < BATCH;
        Ok(())
    }
}

/// The page `query` asks for of `history`, as far as `viewer` may see.
///
/// # Errors
///
/// When the history can't be read.
pub fn search(history: &Path, viewer: &Viewer, query: &Query) -> rusqlite::Result<Page> {
    if !history.exists() {
        return Ok(Page::default());
    }
    let connection = crate::trim::open(history)?;
    // Text the index can find: it gives the rows that have it, and only what's newer than it
    // has is scanned. Every row comes from Ergo's history, so nothing Ergo no longer has is
    // ever returned, whatever the index still holds (`crate::index`).
    let indexed = match query.text.as_deref().and_then(crate::index::phrase) {
        Some(phrase) => attach_index(&connection, history)?.map(|last| (phrase, last)),
        None => None,
    };
    let after = indexed.as_ref().map_or(0, |(_, last)| *last);
    let channel = match &query.conversation {
        Some(Conversation::Channel(name)) => Some(name.clone()),
        _ => None,
    };
    let direct_only = matches!(query.conversation, Some(Conversation::Direct(..)));
    let channels = Source {
        connection: &connection,
        sql: format!(
            "SELECT s.nanotime, h.id, h.msgid, h.data, s.target, NULL FROM sequence s \
             JOIN history h ON h.id = s.history_id \
             WHERE s.nanotime >= ?1 AND (s.nanotime, h.id) < (?2, ?3) AND s.target LIKE '#%' \
             AND {} AND ?5 = ?5 AND h.id > ?7 ORDER BY s.nanotime DESC, h.id DESC LIMIT ?6",
            if channel.is_some() {
                "s.target = ?4"
            } else {
                "?4 = ?4"
            }
        ),
        rows: VecDeque::new(),
        from: query.before,
        done: direct_only,
        since: query.since,
        until: query.until,
        narrow: (channel.clone().unwrap_or_default(), String::new()),
        extra: vec![after.into()],
    };
    // Each direct message is kept for both its accounts: read one pair's under its first, a
    // viewer's own under theirs and their machines', or, for a viewer who may see them all,
    // every row. A message met twice is kept once.
    let (narrow, sql) = match &query.conversation {
        Some(Conversation::Direct(a, b)) => (
            (a.clone(), b.clone()),
            "c.target = ?4 AND c.correspondent = ?5",
        ),
        // The viewer's own, and their machines'.
        _ if viewer.policy != Visibility::Everything => (
            (viewer.account.clone(), viewer.machines_json()),
            "(c.target = ?4 OR c.target IN (SELECT value FROM json_each(?5)))",
        ),
        _ => (Default::default(), "?4 = ?4 AND ?5 = ?5"),
    };
    let direct = Source {
        connection: &connection,
        sql: format!(
            "SELECT c.nanotime, h.id, h.msgid, h.data, c.target, c.correspondent \
             FROM conversations c JOIN history h ON h.id = c.history_id \
             WHERE c.nanotime >= ?1 AND (c.nanotime, h.id) < (?2, ?3) AND {sql} AND h.id > ?7 \
             ORDER BY c.nanotime DESC, h.id DESC LIMIT ?6"
        ),
        rows: VecDeque::new(),
        from: query.before,
        done: matches!(query.conversation, Some(Conversation::Channel(_))),
        since: query.since,
        until: query.until,
        narrow,
        extra: vec![after.into()],
    };
    let Some((phrase, _)) = indexed else {
        return collect(viewer, query, vec![channels, direct]);
    };
    if query.best {
        return best(&connection, viewer, query, phrase);
    }
    let found = found(&connection, query, phrase);
    let mut page = collect(viewer, query, vec![found, channels, direct])?;
    page.indexed = true;
    Ok(page)
}

/// SQL conditions on the index's rows (`r`) for `query`'s conversation, with the values of
/// its `?4` and `?5`.
fn place(query: &Query) -> ((String, String), &'static str) {
    match &query.conversation {
        Some(Conversation::Channel(name)) => {
            ((name.clone(), String::new()), "r.channel = ?4 AND ?5 = ?5")
        }
        Some(Conversation::Direct(a, b)) => ((a.clone(), b.clone()), "r.a = ?4 AND r.b = ?5"),
        None => (Default::default(), "?4 = ?4 AND ?5 = ?5"),
    }
}

/// The text to match, the sender, and whether events count: the index's `?7` to `?9`.
fn filters(query: &Query, phrase: String) -> Vec<rusqlite::types::Value> {
    vec![
        phrase.into(),
        query.sender.as_ref().map(|s| s.to_lowercase()).into(),
        i64::from(query.wants_events()).into(),
    ]
}

/// What the index's `?7` to `?9` ask of a row.
const FILTERS: &str = "g.grams MATCH ?7 AND (?8 IS NULL OR r.sender = ?8) AND (?9 OR r.said)";

/// A page of the best matches first: FTS5's bm25, then the newest. Every row is joined to
/// Ergo's history and checked against the viewer, as newest first is.
fn best(
    connection: &Connection,
    viewer: &Viewer,
    query: &Query,
    phrase: String,
) -> rusqlite::Result<Page> {
    let (narrow, place) = place(query);
    let sql = format!(
        "SELECT r.nanotime, h.id, h.msgid, h.data, coalesce(r.channel, r.a), r.b \
         FROM idx.grams g JOIN idx.rows r ON r.id = g.rowid JOIN history h ON h.id = r.id \
         WHERE {FILTERS} AND r.nanotime >= ?1 AND r.nanotime < ?2 AND {place} \
         ORDER BY bm25(g.grams), r.nanotime DESC, r.id DESC LIMIT ?6 OFFSET ?3"
    );
    let mut statement = connection.prepare(&sql)?;
    let extra = filters(query, phrase);
    let (mut messages, mut skip) = (Vec::new(), query.skip);
    loop {
        let first: [rusqlite::types::Value; 6] = [
            query.since.into(),
            query.until.into(),
            i64::try_from(skip).unwrap_or(i64::MAX).into(),
            narrow.0.clone().into(),
            narrow.1.clone().into(),
            i64::try_from(BATCH).unwrap_or(i64::MAX).into(),
        ];
        let mut rows = statement.query(rusqlite::params_from_iter(first.iter().chain(&extra)))?;
        let mut scanned = 0;
        while let Some(row) = rows.next()? {
            scanned += 1;
            skip += 1;
            let (target, other): (String, Option<String>) = (row.get(4)?, row.get(5)?);
            let row = Row {
                at: Cursor {
                    nanotime: row.get(0)?,
                    id: row.get(1)?,
                },
                msgid: row.get(2)?,
                data: row.get(3)?,
                conversation: match other {
                    None => Conversation::Channel(target),
                    Some(other) => Conversation::direct(&target, &other),
                },
            };
            if viewer.may_see(&row.conversation)
                && let Some(message) = read(&row, query)
            {
                messages.push(message);
                if messages.len() >= query.limit {
                    return Ok(Page {
                        messages,
                        next: Some(format!("b{skip}")),
                        indexed: true,
                    });
                }
            }
        }
        if scanned < BATCH || skip - query.skip >= SCAN {
            let more = scanned == BATCH;
            return Ok(Page {
                messages,
                next: more.then(|| format!("b{skip}")),
                indexed: true,
            });
        }
    }
}

/// How many messages `query`'s text finds in each conversation `viewer` may read, most
/// first, whatever conversation `query` is in: the counts beside a search, such as “12 in #api”. Only conversations the viewer
/// may read are counted at all, so one they may not never shows, not even as nothing.
/// `None` when the index can't answer: text it can't find, or no index built yet.
///
/// # Errors
///
/// When the history or the index can't be read.
pub fn counts(
    history: &Path,
    viewer: &Viewer,
    query: &Query,
) -> rusqlite::Result<Option<Vec<Count>>> {
    if !history.exists() {
        return Ok(None);
    }
    let Some(phrase) = query.text.as_deref().and_then(crate::index::phrase) else {
        return Ok(None);
    };
    let connection = crate::trim::open(history)?;
    if attach_index(&connection, history)?.is_none() {
        return Ok(None);
    }
    // Everywhere the viewer may read, whatever conversation the page is in, so one answer
    // says how many matches there are elsewhere too.
    let sql = format!(
        "SELECT coalesce(r.channel, r.a), r.b, count(*) \
         FROM idx.grams g JOIN idx.rows r ON r.id = g.rowid JOIN history h ON h.id = r.id \
         WHERE {FILTERS} AND r.nanotime >= ?1 AND r.nanotime < ?2 AND ?3 = ?3 AND ?4 = ?4 \
         AND ?5 = ?5 AND ?6 = ?6 GROUP BY 1, 2"
    );
    let first: [rusqlite::types::Value; 6] = [
        query.since.into(),
        query.until.into(),
        0.into(),
        String::new().into(),
        String::new().into(),
        0.into(),
    ];
    let extra = filters(query, phrase);
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(first.iter().chain(&extra)))?;
    let mut counts = Vec::new();
    while let Some(row) = rows.next()? {
        let (target, other): (String, Option<String>) = (row.get(0)?, row.get(1)?);
        let conversation = match other {
            None => Conversation::Channel(target),
            Some(other) => Conversation::direct(&target, &other),
        };
        if viewer.may_see(&conversation) {
            let count: i64 = row.get(2)?;
            counts.push(Count {
                conversation: conversation.to_string(),
                count: u64::try_from(count).unwrap_or(0),
            });
        }
    }
    counts.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(a.conversation.cmp(&b.conversation))
    });
    Ok(Some(counts))
}

/// The index's rows with `phrase`'s text, narrowed as `query` asks, joined to Ergo's history,
/// so a row Ergo no longer has is never one of them.
fn found<'a>(connection: &'a Connection, query: &Query, phrase: String) -> Source<'a> {
    let (narrow, place) = place(query);
    Source {
        connection,
        sql: format!(
            "SELECT r.nanotime, h.id, h.msgid, h.data, coalesce(r.channel, r.a), r.b \
             FROM idx.grams g JOIN idx.rows r ON r.id = g.rowid JOIN history h ON h.id = r.id \
             WHERE {FILTERS} AND r.nanotime >= ?1 AND (r.nanotime, r.id) < (?2, ?3) \
             AND {place} ORDER BY r.nanotime DESC, r.id DESC LIMIT ?6"
        ),
        rows: VecDeque::new(),
        from: query.before,
        done: false,
        since: query.since,
        until: query.until,
        narrow,
        extra: filters(query, phrase),
    }
}

/// Attaches the search index beside `history` as `idx`, read-only, and says the newest history
/// id it has; `None` when there's none yet or it hasn't caught up once.
fn attach_index(connection: &Connection, history: &Path) -> rusqlite::Result<Option<i64>> {
    let index = crate::index::path_for(history);
    if !index.exists() {
        return Ok(None);
    }
    connection.execute(
        "ATTACH DATABASE ?1 AS idx",
        [index.to_string_lossy().into_owned()],
    )?;
    let ready = connection
        .query_row(
            "SELECT (SELECT value FROM idx.meta WHERE key = 'version'), \
                    (SELECT value FROM idx.meta WHERE key = 'built'), \
                    (SELECT value FROM idx.meta WHERE key = 'last')",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .ok();
    Ok(match ready {
        Some((Some(version), Some(built), Some(last)))
            if version == crate::index::VERSION && built == "1" =>
        {
            last.parse().ok()
        }
        _ => None,
    })
}

/// Merges the sources newest first, keeping what the viewer may see and the query wants.
fn collect(viewer: &Viewer, query: &Query, mut sources: Vec<Source<'_>>) -> rusqlite::Result<Page> {
    let (mut messages, mut seen, mut scanned) = (Vec::new(), HashSet::new(), 0);
    loop {
        // The source whose next row is newest.
        let mut newest: Option<(usize, Cursor)> = None;
        for (i, source) in sources.iter_mut().enumerate() {
            if let Some(row) = source.peek()?
                && newest.is_none_or(|(_, at)| row.at > at)
            {
                newest = Some((i, row.at));
            }
        }
        let Some((i, _)) = newest else {
            return Ok(Page {
                messages,
                ..Page::default()
            });
        };
        let row = sources[i].rows.pop_front().expect("peeked");
        scanned += 1;
        if seen.insert(row.at.id)
            && viewer.may_see(&row.conversation)
            && let Some(message) = read(&row, query)
        {
            messages.push(message);
        }
        if messages.len() >= query.limit || scanned >= SCAN {
            return Ok(Page {
                messages,
                next: Some(row.at.to_string()),
                indexed: false,
            });
        }
    }
}

/// What the index keeps of a history item, so it can be filtered without reading Ergo's.
pub(crate) struct Indexed {
    /// Its text as people read it: a zirc message's own.
    pub text: String,
    /// The account it was sent from, folded, if any.
    pub sender: Option<String>,
    /// Said by a person or an agent, not one of zircd's automatic events or a notice.
    pub said: bool,
    /// The zirc session that sent it, for an agent's.
    pub session: Option<String>,
}

/// What the index keeps of a history item, or `None` for what has no text, such as a TAGMSG.
pub(crate) fn indexed(data: &[u8]) -> Option<Indexed> {
    let said = exporter::said(data)?;
    if said.kind != PRIVMSG && said.kind != 2 {
        return None;
    }
    let zirc = zirc_proto::decode(&said.text).ok().flatten();
    let by_someone = said.kind == PRIVMSG
        && zirc
            .as_ref()
            .is_none_or(|m| matches!(m.event, zirc_proto::Event::Text { .. }));
    Some(Indexed {
        sender: Some(said.account.to_lowercase()).filter(|a| !a.is_empty() && a != "*"),
        said: by_someone,
        session: zirc.as_ref().map(|m| m.session.clone()),
        text: match zirc {
            Some(message) => message.search_text(),
            None => said.text,
        },
    })
}

/// The message in `row`, if it's one `query` wants.
fn read(row: &Row, query: &Query) -> Option<Message> {
    let said = exporter::said(&row.data)?;
    let account = Some(said.account.to_lowercase()).filter(|a| !a.is_empty() && a != "*");
    let kind_of = |account: &Option<String>| {
        if account.as_deref().is_some_and(crate::usage::is_agent) {
            "agent"
        } else {
            "person"
        }
    };
    let zirc = zirc_proto::decode(&said.text).ok().flatten();
    let kind = match said.kind {
        PRIVMSG if zirc.is_none() && said.text.starts_with("\u{1}ACTION ") => "action",
        PRIVMSG => "privmsg",
        2 => "notice",
        _ => return None,
    };
    if let Some(sender) = &query.sender
        && account.as_deref() != Some(sender.to_lowercase().as_str())
    {
        return None;
    }
    // What people and agents say, unless asked for everything: zircd's other events and
    // notices are machines talking (as `crate::usage` counts).
    let said_by_someone = kind != "notice"
        && zirc
            .as_ref()
            .is_none_or(|m| matches!(m.event, zirc_proto::Event::Text { .. }));
    if !query.wants_events() && !said_by_someone {
        return None;
    }
    let text = match &zirc {
        Some(message) => message.event.human_text(),
        None => said.text.clone(),
    };
    if let Some(wanted) = &query.text {
        let found = zirc
            .as_ref()
            .map_or_else(|| text.clone(), zirc_proto::Message::search_text);
        if !found.to_lowercase().contains(&wanted.to_lowercase()) {
            return None;
        }
    }
    Some(Message {
        msgid: row.msgid.clone(),
        time: Timestamp::from_nanosecond(i128::from(row.at.nanotime))
            .map(|t| t.to_string())
            .unwrap_or_default(),
        conversation: row.conversation.to_string(),
        sender: Sender {
            kind: kind_of(&account),
            account,
            nick: said.nick,
        },
        kind,
        text,
        zirc: zirc.map(|m| {
            let mut event = serde_json::to_value(&m.event).unwrap_or(Value::Null);
            if let Some(level) = m.level().and_then(|level| serde_json::to_value(level).ok()) {
                event["level"] = level;
            }
            if let Some(task) = m.task.as_deref().and_then(zirc_proto::task_id) {
                event["task"] = Value::String(task);
            }
            event["session"] = Value::String(m.session);
            event
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trim::testing::{add, add_direct, history};

    const SECOND: i64 = 1_000_000_000;

    /// A server's history: alice and bob in #team, carol alone in #secret, a post from alice's
    /// machine and zircd's status in #team, and direct messages between alice and bob and
    /// between bob and carol.
    fn sample() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#team", 10 * SECOND, "alice", "morning");
        add(&path, "m2", "#team", 20 * SECOND, "bob", "morning, alice");
        add(&path, "m3", "#secret", 30 * SECOND, "carol", "the plan");
        let zirc = |event| zirc_proto::encode(&zirc_proto::Message::new("cc-1", event)).unwrap();
        let mut taking = zirc_proto::Message::new(
            "cc-1",
            zirc_proto::Event::Text {
                body: "taking: the tests".into(),
            },
        );
        taking.task = Some("t-abc123".into());
        let post = zirc_proto::encode(&taking).unwrap();
        add(&path, "m4", "#team", 40 * SECOND, "alice-mbp", &post);
        let status = zirc(zirc_proto::Event::Working {
            paths: vec!["a.rs".into()],
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        });
        add(&path, "m5", "#team", 50 * SECOND, "alice-mbp", &status);
        add_direct(&path, "d1", 60 * SECOND, "alice", "bob", "lunch?");
        add_direct(&path, "d2", 70 * SECOND, "bob", "carol", "about alice");
        add_direct(
            &path,
            "d3",
            80 * SECOND,
            "alice-mbp",
            "alice",
            "done: the tests",
        );
        (dir, path)
    }

    fn viewer(account: &str, channels: &[&str], policy: Visibility) -> Viewer {
        Viewer {
            account: account.into(),
            // Each person's machines in these tests, as the server would be told of them.
            machines: [format!("{account}-mbp"), format!("{account}-ci")].into(),
            channels: channels.iter().map(|c| (*c).to_owned()).collect(),
            policy,
        }
    }

    fn query() -> Query {
        Query {
            since: 0,
            until: 1_000 * SECOND,
            limit: 50,
            ..Query::default()
        }
    }

    fn ids(path: &Path, viewer: &Viewer, query: &Query) -> Vec<String> {
        search(path, viewer, query)
            .unwrap()
            .messages
            .into_iter()
            .map(|m| m.msgid)
            .collect()
    }

    #[test]
    fn participants_see_their_own_channels_and_direct_messages() {
        let (_dir, path) = sample();
        let alice = viewer("alice", &["#team"], Visibility::Participant);
        // Her agent's direct messages with her are hers, met twice and kept once.
        assert_eq!(ids(&path, &alice, &query()), ["d3", "d1", "m4", "m2", "m1"]);
        let carol = viewer("carol", &["#secret"], Visibility::Participant);
        assert_eq!(ids(&path, &carol, &query()), ["d2", "m3"]);
        assert!(!alice.may_see(&Conversation::Channel("#secret".into())));
        assert!(!alice.may_see(&Conversation::direct("bob", "carol")));
        assert!(alice.may_see(&Conversation::direct("alice-ci", "bob")));
        // A name that starts as theirs does isn't theirs: only what the server was told is.
        assert!(!alice.may_see(&Conversation::direct("alice-desktop", "bob")));
        assert!(!alice.may_see(&Conversation::direct("alicea", "bob")));
    }

    #[test]
    fn channels_lets_everyone_read_every_channel_but_not_others_direct_messages() {
        let (_dir, path) = sample();
        let alice = viewer("alice", &["#team"], Visibility::Channels);
        assert_eq!(
            ids(&path, &alice, &query()),
            ["d3", "d1", "m4", "m3", "m2", "m1"]
        );
        assert!(alice.may_see(&Conversation::Channel("#secret".into())));
        assert!(!alice.may_see(&Conversation::direct("bob", "carol")));
    }

    #[test]
    fn everything_lets_everyone_read_others_direct_messages_too_once_each() {
        let (_dir, path) = sample();
        let alice = viewer("alice", &[], Visibility::Everything);
        assert_eq!(
            ids(&path, &alice, &query()),
            ["d3", "d2", "d1", "m4", "m3", "m2", "m1"]
        );
        let between = Query {
            conversation: Conversation::parse("@bob,@carol", "alice"),
            ..query()
        };
        let page = search(&path, &alice, &between).unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].conversation, "@bob,@carol");
    }

    #[test]
    fn a_search_narrows_by_conversation_sender_text_and_time_and_pages() {
        let (_dir, path) = sample();
        let alice = viewer("alice", &["#team"], Visibility::Participant);
        let with_bob = Query {
            conversation: Conversation::parse("@bob", "alice"),
            ..query()
        };
        assert_eq!(ids(&path, &alice, &with_bob), ["d1"]);
        let by_bob = Query {
            sender: Some("Bob".into()),
            ..query()
        };
        assert_eq!(ids(&path, &alice, &by_bob), ["m2"]);
        let morning = Query {
            text: Some("MORNING".into()),
            ..query()
        };
        assert_eq!(ids(&path, &alice, &morning), ["m2", "m1"]);
        let early = Query {
            until: 15 * SECOND,
            ..query()
        };
        assert_eq!(ids(&path, &alice, &early), ["m1"]);
        // Two at a time, each page carrying on from the last.
        let first = search(
            &path,
            &alice,
            &Query {
                limit: 3,
                ..query()
            },
        )
        .unwrap();
        let first_ids: Vec<&str> = first.messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(first_ids, ["d3", "d1", "m4"]);
        let rest = Query {
            limit: 3,
            before: first.next.as_deref().map(|n| n.parse().unwrap()),
            ..query()
        };
        assert_eq!(ids(&path, &alice, &rest), ["m2", "m1"]);
    }

    #[test]
    fn a_search_with_an_index_finds_the_same_and_never_what_ergo_no_longer_has() {
        let (_dir, path) = sample();
        let alice = viewer("alice", &["#team"], Visibility::Participant);
        let morning = Query {
            text: Some("MORNING".into()),
            ..query()
        };
        let scanned = ids(&path, &alice, &morning);
        let mut index = crate::index::open(&crate::index::path_for(&path)).unwrap();
        crate::index::catch_up(&mut index, &path).unwrap();
        assert_eq!(ids(&path, &alice, &morning), scanned);
        assert_eq!(scanned, ["m2", "m1"]);
        // Others' channels and direct messages stay out, as without the index.
        let plan = Query {
            text: Some("the plan".into()),
            ..query()
        };
        assert!(ids(&path, &alice, &plan).is_empty());
        let about = Query {
            text: Some("about alice".into()),
            ..query()
        };
        assert!(ids(&path, &alice, &about).is_empty());
        // Said after the index last caught up: found by scanning what's newer.
        add(
            &path,
            "m6",
            "#team",
            90 * SECOND,
            "bob",
            "good morning again",
        );
        assert_eq!(ids(&path, &alice, &morning), ["m6", "m2", "m1"]);
        // Redacted by its author: gone from Ergo, still in the index, never returned.
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "DELETE FROM sequence WHERE history_id = (SELECT id FROM history WHERE msgid = 'm2');
                 DELETE FROM history WHERE msgid = 'm2';",
            )
            .unwrap();
        assert_eq!(ids(&path, &alice, &morning), ["m6", "m1"]);
        // Narrowed to one conversation, through the index.
        let in_team = Query {
            conversation: Conversation::parse("#team", "alice"),
            text: Some("morning".into()),
            ..query()
        };
        assert_eq!(ids(&path, &alice, &in_team), ["m6", "m1"]);
        let with_bob = Query {
            conversation: Conversation::parse("@bob", "alice"),
            text: Some("lunch".into()),
            ..query()
        };
        assert_eq!(ids(&path, &alice, &with_bob), ["d1"]);
    }

    #[test]
    fn a_search_finds_a_message_by_its_data_with_or_without_the_index() {
        let (_dir, path) = sample();
        let alice = viewer("alice", &["#team"], Visibility::Participant);
        let find = |text: &str| Query {
            text: Some(text.into()),
            ..query()
        };
        let task = find("t-abc123");
        // Only in the post's data, and in the working set's path: found, events included.
        let path_of = find("a.rs");
        let scanned = (ids(&path, &alice, &task), ids(&path, &alice, &path_of));
        assert_eq!(scanned, (vec!["m4".to_owned()], vec!["m5".to_owned()]));
        let mut index = crate::index::open(&crate::index::path_for(&path)).unwrap();
        crate::index::catch_up(&mut index, &path).unwrap();
        assert_eq!(
            (ids(&path, &alice, &task), ids(&path, &alice, &path_of)),
            scanned
        );
        // A field's name finds nothing.
        assert!(ids(&path, &alice, &find("paths")).is_empty());
        // Without text, zircd's events stay out unless asked for.
        assert!(!ids(&path, &alice, &query()).contains(&"m5".to_owned()));
    }

    /// The sample's history with its index built.
    fn indexed_sample() -> (tempfile::TempDir, std::path::PathBuf) {
        let (dir, path) = sample();
        add(
            &path,
            "m6",
            "#secret",
            85 * SECOND,
            "carol",
            "morning plan, morning",
        );
        add(&path, "m7", "#team", 86 * SECOND, "carol", "a morning");
        let mut index = crate::index::open(&crate::index::path_for(&path)).unwrap();
        crate::index::catch_up(&mut index, &path).unwrap();
        (dir, path)
    }

    #[test]
    fn counts_cover_only_conversations_the_viewer_may_read() {
        let (_dir, path) = indexed_sample();
        let morning = Query {
            text: Some("morning".into()),
            ..query()
        };
        let alice = viewer("alice", &["#team"], Visibility::Participant);
        let found = counts(&path, &alice, &morning).unwrap().unwrap();
        // #secret matches too, but Alice isn't in it: it isn't there at all, not even as 0.
        assert_eq!(
            found,
            [Count {
                conversation: "#team".into(),
                count: 3
            }]
        );
        let everyone = viewer("alice", &[], Visibility::Channels);
        let found = counts(&path, &everyone, &morning).unwrap().unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|c| (c.conversation.as_str(), c.count))
            .collect();
        assert_eq!(names, [("#team", 3), ("#secret", 1)]);
        // By sender, in the index, not by reading the text.
        let by_carol = Query {
            sender: Some("Carol".into()),
            ..morning.clone()
        };
        let found = counts(&path, &everyone, &by_carol).unwrap().unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|c| (c.conversation.as_str(), c.count))
            .collect();
        assert_eq!(names, [("#secret", 1), ("#team", 1)]);
        // Searching one conversation still counts the others, for “also N elsewhere”.
        let here = Query {
            conversation: Conversation::parse("#team", "alice"),
            ..morning.clone()
        };
        assert_eq!(counts(&path, &everyone, &here).unwrap().unwrap().len(), 2);
        // Too short for the index: no counts, rather than wrong ones.
        let short = Query {
            text: Some("mo".into()),
            ..query()
        };
        assert_eq!(counts(&path, &everyone, &short).unwrap(), None);
    }

    #[test]
    fn best_first_ranks_by_match_and_pages_by_how_many_it_skipped() {
        let (_dir, path) = indexed_sample();
        let everyone = viewer("alice", &[], Visibility::Channels);
        let best = Query {
            text: Some("morning".into()),
            best: true,
            limit: 2,
            ..query()
        };
        let first = search(&path, &everyone, &best).unwrap();
        assert!(first.indexed);
        // bm25: m1 is the match and nothing else, so it beats longer messages.
        assert_eq!(first.messages[0].msgid, "m1");
        assert_eq!(first.messages.len(), 2);
        let rest = Query {
            skip: first.next.as_deref().unwrap()[1..].parse().unwrap(),
            ..best.clone()
        };
        let second = search(&path, &everyone, &rest).unwrap();
        let mut all: Vec<_> = first
            .messages
            .iter()
            .chain(&second.messages)
            .map(|m| m.msgid.clone())
            .collect();
        all.sort();
        assert_eq!(all, ["m1", "m2", "m6", "m7"]);
        // Alice in #team alone never gets #secret's, however good a match.
        let alice = viewer("alice", &["#team"], Visibility::Participant);
        let page = search(&path, &alice, &best).unwrap();
        assert!(page.messages.iter().all(|m| m.conversation == "#team"));
    }

    #[test]
    fn messages_say_who_sent_them_and_carry_zirc_s_event() {
        crate::machines::the_tests_machines();
        let (_dir, path) = sample();
        let alice = viewer("alice", &["#team"], Visibility::Participant);
        let page = search(&path, &alice, &query()).unwrap();
        let post = page.messages.iter().find(|m| m.msgid == "m4").unwrap();
        assert_eq!(post.sender.kind, "agent");
        assert_eq!(post.text, "taking: the tests");
        assert_eq!(post.zirc.as_ref().unwrap()["type"], "text");
        assert_eq!(post.zirc.as_ref().unwrap()["session"], "cc-1");
        // The message's task and level beside its event.
        assert_eq!(post.zirc.as_ref().unwrap()["task"], "t-abc123");
        assert_eq!(post.zirc.as_ref().unwrap()["level"], "info");
        // zircd's status only when asked for.
        assert!(page.messages.iter().all(|m| m.msgid != "m5"));
        let events = Query {
            events: true,
            ..query()
        };
        assert!(ids(&path, &alice, &events).contains(&"m5".to_owned()));
        let morning = page.messages.iter().find(|m| m.msgid == "m1").unwrap();
        assert_eq!(
            (
                morning.sender.kind,
                morning.conversation.as_str(),
                morning.kind
            ),
            ("person", "#team", "privmsg")
        );
    }
}
