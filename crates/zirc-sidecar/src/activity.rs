//! A record of how a team works together, for measuring it (decided 8 Nov 2026: raw
//! collaboration events are kept in the tenant's sidecar, and expire with its messages):
//! one row of metadata for each line of Ergo's history, and for each of zircd's live
//! activity records ([`zirc_proto::activity`]) the recorder hears ([`crate::recorder`]).
//!
//! It lives in its own SQLite file beside Ergo's history ([`path_for`]), as the search index
//! does ([`crate::index`]), and is filled the same way: history rows by Ergo's own row id,
//! from the last one it has ([`catch_up`]). It holds no text: who, when, where, what kind,
//! what level, which task, and for zirc's own kinds a few fields of their data (paths,
//! branches, CI's state and build), never a post's words, a commit's subject, a decision's
//! quote or another team's custom data, which could carry text.
//!
//! It keeps a row only as long as the organisation keeps its messages: a history row's goes
//! when Ergo no longer has the row (retention, a redaction, a trim), and a live record's
//! once it's older than the oldest message Ergo still keeps ([`prune`]).
//!
//! Beside the rows, the first time each onboarding signal it sees was seen from each account
//! ([`zirc_proto::onboarding::First`], [`firsts`]): an account and a time, kept as long as
//! the store whatever the organisation keeps of its messages, for zirc.dev to read the steps
//! its members reached.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use serde_json::{Value, json};
use tracing::{info, warn};
use zirc_proto::{Event, Message};

use crate::exporter::{self, PRIVMSG};

/// The store's format. A file with another is built again from Ergo's history; live
/// records it held are lost, so a format that only adds is brought up to date in place
/// ([`migrate`]). 2: a state's level and owner, which 1 missed. 3: whether a working set
/// says again what its session said earlier in the hour ([`AGAIN`]), from 2 in place. 4: the
/// onboarding signals' firsts ([`FIRSTS`]), from 3 in place.
pub const VERSION: &str = "4";

/// An hour, in nanoseconds: how far apart a session's like working sets are both kept.
const HOUR: i64 = 3_600 * 1_000_000_000;

/// Whether the row being recorded (?1 its Ergo row, ?2 its time, ?3 its session, ?4 agent,
/// ?5 task, ?6 data) is a working set its session said, the same, earlier in the same hour:
/// most of the store, as zircd says a working set each time it changes or is asked, and
/// nothing a reader of who changed what, to the hour, needs twice ([`rows`]' `collapse`).
const AGAIN: &str = "EXISTS (
    SELECT 1 FROM rows e
    WHERE e.kind = 'working' AND e.session = ?3
      AND e.nanotime >= ?2 / 3600000000000 * 3600000000000 AND e.nanotime <= ?2
      AND e.agent = ?4 AND e.task IS ?5 AND e.data IS ?6
      AND (?1 IS NULL OR e.ergo IS NOT ?1)
)";

/// How many history rows one step of [`catch_up`] reads and commits.
const BATCH: usize = 1_000;

/// How long a write waits for another to finish.
const BUSY: Duration = Duration::from_secs(10);

/// How often [`keep_recorded`] reads what's new in the history.
pub const EVERY: Duration = Duration::from_secs(15);

/// How often [`keep_recorded`] prunes what the history no longer has.
const TIDY: Duration = Duration::from_hours(1);

/// The most rows [`rows`] returns at once.
pub const MOST: usize = 5_000;

/// The store for the history at `history`: `zirc_activity.db` in the same directory.
#[must_use]
pub fn path_for(history: &Path) -> PathBuf {
    history.with_file_name("zirc_activity.db")
}

/// The store, opened to write, made or made again if it's missing or of another format.
///
/// # Errors
///
/// When the file can't be opened or made.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    crate::sqlite::made(open_file(path)?, VERSION, |connection, found| {
        if found == Some("2") {
            migrate(connection)?;
            return add_firsts(connection);
        }
        if found == Some("3") {
            return add_firsts(connection);
        }
        if found.is_some() {
            info!(
                version = ?found,
                "the activity store's format changed: building it again"
            );
        }
        crate::sqlite::clear(connection)?;
        connection.execute_batch(&format!(
            "
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             -- One row per history row (ergo, its id) or live record (ergo NULL).
             CREATE TABLE rows (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 ergo INTEGER UNIQUE,
                 msgid TEXT,
                 nanotime INTEGER NOT NULL,
                 channel TEXT,
                 direct INTEGER NOT NULL DEFAULT 0,
                 account TEXT,
                 agent INTEGER NOT NULL DEFAULT 0,
                 session TEXT,
                 name TEXT,
                 tool TEXT,
                 kind TEXT NOT NULL,
                 level TEXT,
                 task TEXT,
                 data TEXT,
                 -- A working set said again in its hour ([`AGAIN`]).
                 again INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX rows_time ON rows (nanotime, id);
             CREATE INDEX rows_kind ON rows (kind, nanotime);
             {REPEATS}
             {FIRSTS}
             INSERT INTO meta VALUES ('version', '{VERSION}'), ('last', '0');"
        ))
    })
}

/// Each session's working sets by time, for [`AGAIN`].
const REPEATS: &str =
    "CREATE INDEX rows_repeats ON rows (session, nanotime) WHERE kind = 'working';";

/// Brings a store of format 2 up to [`VERSION`], keeping its live records: marks the working
/// sets said again in their hour, as [`insert`] does from now on. Inside [`open`]'s write
/// transaction ([`crate::sqlite::made`]), so a second connection opening meanwhile waits and
/// finds it done.
fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(&format!(
        "ALTER TABLE rows ADD COLUMN again INTEGER NOT NULL DEFAULT 0;
         {REPEATS}
         UPDATE rows SET again = 1 WHERE id IN (
             SELECT id FROM (
                 SELECT id, ROW_NUMBER() OVER (
                     PARTITION BY session, agent, task, nanotime / {HOUR}, data ORDER BY id
                 ) AS nth
                 FROM rows WHERE kind = 'working' AND session IS NOT NULL
             ) WHERE nth > 1
         );"
    ))?;
    info!("the activity store marks working sets said again in their hour");
    Ok(())
}

/// The first time each onboarding signal was seen from each account ([`note_first`]), never
/// pruned.
const FIRSTS: &str = "CREATE TABLE firsts (
    signal TEXT NOT NULL,
    account TEXT NOT NULL,
    nanotime INTEGER NOT NULL,
    PRIMARY KEY (signal, account)
) WITHOUT ROWID;";

/// Brings a store of format 3 up to [`VERSION`]: the firsts, from the rows it has. Inside
/// [`open`]'s write transaction, as [`migrate`] is.
fn add_firsts(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(FIRSTS)?;
    let mut statement = connection.prepare(
        "SELECT nanotime, account, agent, direct, kind, data FROM rows ORDER BY nanotime",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, bool>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut noted = 0;
    for row in rows {
        let (nanotime, account, agent, direct, kind, data) = row?;
        let data = data.and_then(|data| serde_json::from_str(&data).ok());
        let fact = Fact {
            account,
            agent,
            kind,
            data,
            ..Fact::default()
        };
        noted += usize::from(note_first(connection, nanotime, direct, &fact)?);
    }
    connection.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'version'",
        [VERSION],
    )?;
    info!(noted, "the activity store keeps onboarding's firsts");
    Ok(())
}

/// Keeps `fact`'s onboarding signal, if it gives one, as its account's first of it unless an
/// earlier one is kept. Whether it gave one.
fn note_first(
    connection: &Connection,
    nanotime: i64,
    direct: bool,
    fact: &Fact,
) -> rusqlite::Result<bool> {
    let Some(signal) = zirc_proto::onboarding::Signal::of_row(
        &fact.kind,
        fact.account.as_deref(),
        fact.agent,
        direct,
        fact.data.as_ref(),
    ) else {
        return Ok(false);
    };
    let Some(account) = &fact.account else {
        return Ok(false);
    };
    connection
        .prepare_cached(
            "INSERT INTO firsts (signal, account, nanotime) VALUES (?1, ?2, ?3)
             ON CONFLICT (signal, account) DO UPDATE SET nanotime = excluded.nanotime
             WHERE excluded.nanotime < firsts.nanotime",
        )?
        .execute(params![signal.id(), account, nanotime])?;
    Ok(true)
}

/// Each onboarding signal's first from each account, for zirc.dev.
///
/// # Errors
///
/// When the store can't be read.
pub fn firsts(connection: &Connection) -> rusqlite::Result<Vec<zirc_proto::onboarding::First>> {
    let mut statement = connection
        .prepare_cached("SELECT signal, account, nanotime FROM firsts ORDER BY nanotime")?;
    let firsts = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut kept = Vec::new();
    for first in firsts {
        let (signal, account, nanotime) = first?;
        // One a newer sidecar kept that this one doesn't know.
        if let Some(signal) = zirc_proto::onboarding::Signal::from_id(&signal) {
            kept.push(zirc_proto::onboarding::First {
                signal,
                account,
                at: nanotime / 1_000_000,
            });
        }
    }
    Ok(kept)
}

fn open_file(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY)?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA secure_delete=ON;")?;
    Ok(connection)
}

fn last(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("SELECT value FROM meta WHERE key = 'last'", [], |row| {
        row.get::<_, String>(0)
            .map(|value| value.parse().unwrap_or(0))
    })
}

/// What one row says: everything but its place and time, which its source gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fact {
    /// The sender's account, lowercase, when it has one.
    pub account: Option<String>,
    /// Whether the account is a machine's, whose lines are its agents'.
    pub agent: bool,
    /// The zirc session the message names.
    pub session: Option<String>,
    /// What its session is called ([`Message::name`]).
    pub name: Option<String>,
    /// The agent tool its session runs in ([`Message::tool`]).
    pub tool: Option<String>,
    /// `text` for a plain line or post, `notice`, the event's kind (`intent`, `commit`), or
    /// a custom event's (`x-zirc/landing`).
    pub kind: String,
    /// Its level, as [`Message::level`] says.
    pub level: Option<String>,
    /// The task it's part of ([`Message::task`]).
    pub task: Option<String>,
    /// A few fields of its data ([`metadata`]), never text.
    pub data: Option<Value>,
    /// The repository it's about ([`Message::repo`]): a row said in `#agents` is kept under
    /// the repository's scope, `#<repo>` ([`zirc_chat::names::scope_of`]).
    pub repo: Option<String>,
}

impl Fact {
    /// What a zirc `message` from `account` says. A kind only CI says ([`only_cis`]) from
    /// anyone else is kept as words: what counts landings, deploys and checks, and what zirc
    /// suggests from them, is CI's word alone, as zircd shows it.
    ///
    /// [`only_cis`]: zirc_proto::ci::only_cis
    #[must_use]
    pub fn of_message(account: Option<String>, message: &Message) -> Self {
        let kind = kind_of(&message.event);
        if zirc_proto::ci::only_cis(&kind) && !zirc_proto::ci::speaks_for_ci(account.as_deref()) {
            return Self {
                agent: account.as_deref().is_some_and(crate::usage::is_agent),
                session: Some(message.session.clone()).filter(|s| !s.is_empty()),
                account,
                kind: "text".into(),
                repo: message.repo.clone(),
                ..Self::default()
            };
        }
        Self {
            agent: account.as_deref().is_some_and(crate::usage::is_agent),
            account,
            session: Some(message.session.clone()).filter(|s| !s.is_empty()),
            name: message.name.clone(),
            tool: message.tool.clone(),
            kind,
            level: message
                .level()
                .and_then(|level| serde_json::to_value(level).ok())
                .and_then(|value| value.as_str().map(str::to_owned)),
            task: message.task.as_deref().and_then(zirc_proto::task_id),
            data: metadata(message),
            repo: message.repo.clone(),
        }
    }
}

/// An event's kind as a row keeps it.
fn kind_of(event: &Event) -> String {
    match event {
        Event::Custom { kind, .. } => kind.clone(),
        Event::Intent { .. } => "intent".into(),
        Event::Working { .. } => "working".into(),
        Event::Commit { .. } => "commit".into(),
        Event::Push { .. } => "push".into(),
        Event::Text { .. } => "text".into(),
        Event::Query { .. } => "query".into(),
        Event::Answer { .. } => "answer".into(),
        Event::Note { .. } => "note".into(),
        Event::File(_) => "file".into(),
    }
}

/// The fields of `message` worth counting by, and none that are text: an intent's or a
/// working set's paths, a commit's or push's branch and paths, CI's state, build, branch,
/// session, task and what it reverts or fixes, a state's name and how it stands, a
/// request's id and where it stands, and the
/// whole of zircd's activity records, which carry no text. Nothing for another team's
/// custom event, whose data is theirs and could say anything.
#[must_use]
pub fn metadata(message: &Message) -> Option<Value> {
    let it = match &message.event {
        Event::Intent { paths, lasts, .. } => json!({"paths": paths, "lasts": lasts}),
        Event::Working {
            paths,
            worktree,
            branch,
            ..
        } => json!({"paths": paths, "worktree": worktree.is_some(), "branch": branch}),
        Event::Commit {
            branch, sha, paths, ..
        } => json!({"branch": branch, "sha": sha, "paths": paths}),
        Event::Push {
            branch,
            head,
            forced,
            subjects,
            paths,
            base,
            ..
        } => json!({
            "branch": branch, "head": head, "forced": forced, "commits": subjects.len(),
            "paths": paths, "base": base,
        }),
        Event::Text { .. } => json!({"everyone": message.everyone}),
        Event::Custom { kind, data, .. } => {
            let keep: &[&str] = match kind.as_str() {
                zirc_proto::ci::LANDING_KIND | zirc_proto::ci::DEPLOY_KIND => &[
                    "landing",
                    "pipeline",
                    "build",
                    "branch",
                    "session",
                    "commit",
                    "state",
                    "failed",
                    "cause",
                    "task",
                    "reverts",
                    "fixes",
                    "try",
                    "stacked_on",
                    "test",
                ],
                // A check of the default branch or a flaky test: which build, how it went and
                // what failed, for counting flakes and broken heads.
                zirc_proto::ci::CHECK_KIND => {
                    &["pipeline", "build", "commit", "state", "failed", "cause"]
                }
                // Its name, how it stands and who's on it ([`zirc_proto::state::State`]), never
                // why, which is words.
                zirc_proto::state::KIND => &["name", "level", "owner"],
                zirc_proto::LEASE_KIND => &["name", "action", "session", "until"],
                // A question to a person: which one, where it stands and whether a yes can
                // be undone ([`zirc_proto::request::Request`]), never the question.
                zirc_proto::request::KIND => {
                    let request = data.get("request")?;
                    let state = request.get("state");
                    return Some(json!({
                        "id": request.get("id"),
                        "state": state.and_then(|s| s.get("state")).or(state),
                        "irreversible": request.get("irreversible"),
                    }));
                }
                kind if zirc_proto::activity::KINDS.contains(&kind) => return Some(data.clone()),
                _ => return None,
            };
            let mut kept = serde_json::Map::new();
            for key in keep {
                if let Some(value) = data.get(*key) {
                    let value = if *key == "failed" {
                        // Each failed target, without where its results are.
                        json!(value.as_array().map(|failed| {
                            failed
                                .iter()
                                .filter_map(|f| f.get("target").cloned())
                                .collect::<Vec<_>>()
                        }))
                    } else {
                        value.clone()
                    };
                    kept.insert((*key).to_owned(), value);
                }
            }
            Value::Object(kept)
        }
        Event::Query { .. } | Event::Answer { .. } | Event::Note { .. } | Event::File(_) => {
            return None;
        }
    };
    Some(it)
}

/// What a history item says, or `None` for what this doesn't count (a kept TAGMSG, a
/// JOIN).
fn fact_of(data: &[u8]) -> Option<Fact> {
    let said = exporter::said(data)?;
    let account = said.account();
    match said.kind {
        PRIVMSG => Some(match zirc_proto::decode(&said.text).ok().flatten() {
            Some(message) => Fact::of_message(account, &message),
            None => Fact {
                agent: account.as_deref().is_some_and(crate::usage::is_agent),
                account,
                kind: "text".into(),
                ..Fact::default()
            },
        }),
        2 => Some(Fact {
            agent: account.as_deref().is_some_and(crate::usage::is_agent),
            account,
            kind: "notice".into(),
            ..Fact::default()
        }),
        _ => None,
    }
}

/// Records every history row newer than the last recorded, [`BATCH`] at a time, and says
/// how many it read.
///
/// # Errors
///
/// When either database can't be read or the store written.
pub fn catch_up(connection: &mut Connection, history: &Path) -> rusqlite::Result<usize> {
    with_history(connection, history, read_new)
}

fn with_history<T>(
    connection: &mut Connection,
    history: &Path,
    step: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    crate::trim::open(history)?; // Ergo's history, of the schema this knows.
    let uri = format!("file:{}?mode=ro", history.display());
    connection.execute("ATTACH DATABASE ?1 AS hist", [uri])?;
    let outcome = step(connection);
    connection.flush_prepared_statement_cache();
    let detached = connection.execute("DETACH DATABASE hist", []);
    let value = outcome?;
    detached?;
    Ok(value)
}

fn read_new(connection: &mut Connection) -> rusqlite::Result<usize> {
    let mut read = 0;
    loop {
        let from = last(connection)?;
        let items = {
            let mut statement = connection.prepare_cached(
                "SELECT h.id, h.msgid, h.data, s.target, s.nanotime, c.nanotime \
                 FROM hist.history h \
                 LEFT JOIN hist.sequence s ON s.history_id = h.id \
                 LEFT JOIN hist.conversations c ON c.history_id = h.id \
                 WHERE h.id > ?1 ORDER BY h.id LIMIT ?2",
            )?;
            let items = statement.query_map(
                params![from, i64::try_from(BATCH * 2).unwrap_or(i64::MAX)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                },
            )?;
            items.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let Some(newest) = items.last().map(|item| item.0) else {
            return Ok(read);
        };
        let transaction = connection.transaction()?;
        let mut previous = None;
        for (id, msgid, data, channel, channel_time, direct_time) in &items {
            // A direct message's second row, recorded under its first.
            if previous == Some(*id) {
                continue;
            }
            previous = Some(*id);
            read += 1;
            let (channel, direct, nanotime) = match (channel, channel_time, direct_time) {
                (Some(channel), Some(time), _) if channel.starts_with('#') => {
                    (Some(channel.clone()), false, *time)
                }
                (_, _, Some(time)) => (None, true, *time),
                _ => continue,
            };
            let Some(fact) = fact_of(data) else {
                continue;
            };
            insert(
                &transaction,
                Some(*id),
                Some(msgid),
                nanotime,
                channel.as_deref(),
                direct,
                &fact,
            )?;
        }
        transaction.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'last'",
            [newest.to_string()],
        )?;
        transaction.commit()?;
    }
}

pub(crate) fn insert(
    connection: &Connection,
    ergo: Option<i64>,
    msgid: Option<&str>,
    nanotime: i64,
    channel: Option<&str>,
    direct: bool,
    fact: &Fact,
) -> rusqlite::Result<()> {
    let data = fact.data.as_ref().map(Value::to_string);
    let channel = channel.map(|channel| zirc_chat::names::scope_of(channel, fact.repo.as_deref()));
    let again = fact.kind == "working"
        && fact.session.is_some()
        && connection
            .prepare_cached(&format!("SELECT {AGAIN}"))?
            .query_row(
                params![ergo, nanotime, fact.session, fact.agent, fact.task, data],
                |row| row.get::<_, bool>(0),
            )?;
    connection.execute(
        "INSERT OR REPLACE INTO rows \
         (ergo, msgid, nanotime, channel, direct, account, agent, session, name, tool, kind, \
          level, task, data, again) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            ergo,
            msgid,
            nanotime,
            channel,
            direct,
            fact.account,
            fact.agent,
            fact.session,
            fact.name,
            fact.tool,
            fact.kind,
            fact.level,
            fact.task,
            data,
            again,
        ],
    )?;
    note_first(connection, nanotime, direct, fact)?;
    Ok(())
}

/// Records a live `message` the recorder heard from `account` at `nanotime`, outside any
/// history: zircd's activity records, sent to the recorder alone. Anything else live is
/// left out, so a client can't fill the store by writing to the recorder.
///
/// # Errors
///
/// When the store can't be written.
pub fn record_live(
    connection: &Connection,
    nanotime: i64,
    account: Option<String>,
    message: &Message,
) -> rusqlite::Result<bool> {
    let Event::Custom { kind, .. } = &message.event else {
        return Ok(false);
    };
    if !zirc_proto::activity::KINDS.contains(&kind.as_str()) || account.is_none() {
        return Ok(false);
    }
    insert(
        connection,
        None,
        None,
        nanotime,
        None,
        false,
        &Fact::of_message(account, message),
    )?;
    Ok(true)
}

/// How often one part of the team's playbook was used, and when last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Used {
    /// How many times.
    pub count: u64,
    /// When last, in nanoseconds since the Unix epoch.
    pub last: i64,
}

/// How often each hint was shown and each gotcha told since `since` (nanoseconds since the
/// Unix epoch), by the hint's id and the gotcha's message id: the hints first, then the
/// gotchas ([`zirc_proto::activity::HintShown`], [`zirc_proto::activity::GotchaTold`]).
///
/// # Errors
///
/// When the store can't be read.
pub fn playbook_usage(
    connection: &Connection,
    since: i64,
) -> rusqlite::Result<(
    std::collections::BTreeMap<String, Used>,
    std::collections::BTreeMap<String, Used>,
)> {
    use zirc_proto::activity::{GOTCHA_TOLD_KIND, HINT_SHOWN_KIND};
    let sql = "SELECT kind, json_extract(data, CASE kind WHEN ?1 THEN '$.hint' ELSE '$.gotcha' END),                       COUNT(*), MAX(nanotime)                FROM rows WHERE kind IN (?1, ?2) AND nanotime >= ?3 GROUP BY 1, 2";
    let mut statement = connection.prepare_cached(sql)?;
    let found = statement.query_map(params![HINT_SHOWN_KIND, GOTCHA_TOLD_KIND, since], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let (mut hints, mut gotchas) = (
        std::collections::BTreeMap::new(),
        std::collections::BTreeMap::new(),
    );
    for found in found {
        let (kind, id, count, last) = found?;
        let Some(id) = id else { continue };
        let used = Used {
            count: u64::try_from(count).unwrap_or(0),
            last,
        };
        if kind == HINT_SHOWN_KIND {
            hints.insert(id, used);
        } else {
            gotchas.insert(id, used);
        }
    }
    Ok((hints, gotchas))
}

/// Which part of the team's playbook [`playbook_uses`] lists: a hint by its id, or a gotcha by
/// its message id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item<'a> {
    /// A hint, by its id.
    Hint(&'a str),
    /// A gotcha, by its message id.
    Gotcha(&'a str),
}

/// One time a hint was shown or a gotcha told ([`playbook_uses`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    /// When, in nanoseconds since the Unix epoch.
    pub nanotime: i64,
    /// The machine's account it was said on.
    pub account: Option<String>,
    /// The agent session it was said to.
    pub session: Option<String>,
    /// The session's name.
    pub name: Option<String>,
    /// The agent tool the session runs in.
    pub tool: Option<String>,
    /// A hint's layer, or how a gotcha was told.
    pub how: Option<String>,
}

/// How often `item` was used each UTC day of the `days` up to `now` (nanoseconds since the
/// Unix epoch), the oldest first and today so far last, and up to `limit` of its uses in those
/// days before `before` (or `now`), newest first.
///
/// # Errors
///
/// When the store can't be read.
pub fn playbook_uses(
    connection: &Connection,
    item: Item<'_>,
    now: i64,
    days: u32,
    before: Option<i64>,
    limit: usize,
) -> rusqlite::Result<(Vec<u64>, Vec<Use>)> {
    use zirc_proto::activity::{GOTCHA_TOLD_KIND, HINT_SHOWN_KIND};
    const DAY: i64 = 86_400 * 1_000_000_000;
    let (kind, id, field, how) = match item {
        Item::Hint(id) => (HINT_SHOWN_KIND, id, "$.hint", "$.layer"),
        Item::Gotcha(id) => (GOTCHA_TOLD_KIND, id, "$.gotcha", "$.how"),
    };
    // The start of the first of the days.
    let since = (now.div_euclid(DAY) - i64::from(days) + 1) * DAY;
    let mut daily = vec![0_u64; usize::try_from(days).unwrap_or_default()];
    let mut statement = connection.prepare_cached(
        "SELECT (nanotime - ?3) / ?5, COUNT(*) FROM rows \
         WHERE kind = ?1 AND nanotime >= ?3 AND nanotime <= ?4 AND json_extract(data, ?6) = ?2 \
         GROUP BY 1",
    )?;
    let counted = statement.query_map(params![kind, id, since, now, DAY, field], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for counted in counted {
        let (day, count) = counted?;
        if let Some(slot) = usize::try_from(day).ok().and_then(|day| daily.get_mut(day)) {
            *slot = u64::try_from(count).unwrap_or(0);
        }
    }
    let mut statement = connection.prepare_cached(
        "SELECT nanotime, account, session, name, tool, json_extract(data, ?7) FROM rows \
         WHERE kind = ?1 AND nanotime >= ?3 AND nanotime < ?4 AND json_extract(data, ?6) = ?2 \
         ORDER BY nanotime DESC, id DESC LIMIT ?5",
    )?;
    let before = before.map_or(now.saturating_add(1), |before| {
        before.min(now.saturating_add(1))
    });
    let uses = statement
        .query_map(
            params![
                kind,
                id,
                since,
                before,
                i64::try_from(limit).unwrap_or(i64::MAX),
                field,
                how
            ],
            |row| {
                Ok(Use {
                    nanotime: row.get(0)?,
                    account: row.get(1)?,
                    session: row.get(2)?,
                    name: row.get(3)?,
                    tool: row.get(4)?,
                    how: row.get(5)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok((daily, uses))
}

/// Takes out every row whose history row Ergo no longer has, and every live record older
/// than the oldest message Ergo still keeps, and says how many.
///
/// # Errors
///
/// When either database can't be read or the store written.
pub fn prune(connection: &mut Connection, history: &Path) -> rusqlite::Result<usize> {
    with_history(connection, history, |connection| {
        let transaction = connection.transaction()?;
        let mut pruned = transaction.execute(
            "DELETE FROM rows WHERE ergo IS NOT NULL AND ergo NOT IN (SELECT id FROM hist.history)",
            [],
        )?;
        let oldest: Option<i64> = transaction.query_row(
            "SELECT MIN(nanotime) FROM (SELECT nanotime FROM hist.sequence \
             UNION ALL SELECT nanotime FROM hist.conversations)",
            [],
            |row| row.get(0),
        )?;
        if let Some(oldest) = oldest {
            pruned += transaction.execute(
                "DELETE FROM rows WHERE ergo IS NULL AND nanotime < ?1",
                [oldest],
            )?;
        }
        transaction.commit()?;
        Ok(pruned)
    })
}

/// One row, as [`rows`] gives it to zirc.dev: metadata only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// Its id in the store, which [`rows`] pages by.
    pub id: i64,
    /// When, in nanoseconds since the Unix epoch.
    pub nanotime: i64,
    /// The line's id on the server, for a history row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msgid: Option<String>,
    /// The channel it was said in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// A direct message's, whose two accounts aren't kept.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub direct: bool,
    /// Whether it's a live record, never in history.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub live: bool,
    /// The sender's account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Whether the account is a machine's.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agent: bool,
    /// The zirc session it names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// What its session is called.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The agent tool its session runs in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Its kind ([`Fact::kind`]).
    pub kind: String,
    /// Its level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The task it's part of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// A few fields of its data, never text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Rows from `since` (nanoseconds) on, oldest first, after the row `after` when paging, at
/// most `limit` ([`MOST`] at most).
///
/// With `collapse`, without the working sets said again in their hour ([`AGAIN`]), most of the
/// store: the Insights page reads 16 days of rows for its 7-day window, which without it took
/// the web app past its memory and the edge's timeout. Marked as each is recorded, so it costs
/// a read nothing. One whose first in the hour is older than `since`, or pruned, is left out
/// all the same.
///
/// # Errors
///
/// When the store can't be read.
pub fn rows(
    connection: &Connection,
    since: i64,
    after: i64,
    limit: usize,
    collapse: bool,
) -> rusqlite::Result<Vec<Row>> {
    let sql = "SELECT id, nanotime, msgid, channel, direct, ergo IS NULL, account, agent, \
                      session, name, tool, kind, level, task, data \
               FROM rows WHERE nanotime >= ?1 AND id > ?2 AND NOT (?4 AND again) \
               ORDER BY id LIMIT ?3";
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map(
        params![
            since,
            after,
            i64::try_from(limit.min(MOST)).unwrap_or(0),
            collapse
        ],
        |row| {
            Ok(Row {
                id: row.get(0)?,
                nanotime: row.get(1)?,
                msgid: row.get(2)?,
                channel: row.get(3)?,
                direct: row.get(4)?,
                live: row.get(5)?,
                account: row.get(6)?,
                agent: row.get(7)?,
                session: row.get(8)?,
                name: row.get(9)?,
                tool: row.get(10)?,
                kind: row.get(11)?,
                level: row.get(12)?,
                task: row.get(13)?,
                data: row
                    .get::<_, Option<String>>(14)?
                    .and_then(|data| serde_json::from_str(&data).ok()),
            })
        },
    )?;
    rows.map(|row| row.map(ci_word_only)).collect()
}

/// `row` as its readers may take it: a kind only CI says, kept from another account before
/// the store kept such rows as words ([`Fact::of_message`]), is words too.
fn ci_word_only(mut row: Row) -> Row {
    if zirc_proto::ci::only_cis(&row.kind) && !zirc_proto::ci::speaks_for_ci(row.account.as_deref())
    {
        row.kind = "text".into();
        row.level = None;
        row.data = None;
    }
    row
}

/// What [`keep_recorded`] does each time: catches up, and every [`TIDY`] prunes.
pub struct Keeper {
    history: PathBuf,
    connection: Option<Connection>,
    tidied: Option<std::time::Instant>,
}

impl Keeper {
    /// A keeper of the store for the history at `history`.
    #[must_use]
    pub fn new(history: PathBuf) -> Self {
        Self {
            history,
            connection: None,
            tidied: None,
        }
    }

    /// One round.
    ///
    /// # Errors
    ///
    /// When either database can't be read or the store written.
    pub fn tick(&mut self) -> rusqlite::Result<()> {
        if !self.history.exists() {
            return Ok(());
        }
        let connection = match &mut self.connection {
            Some(connection) => connection,
            empty => empty.insert(open(&path_for(&self.history))?),
        };
        catch_up(connection, &self.history)?;
        if self.tidied.is_none_or(|at| at.elapsed() >= TIDY) {
            let pruned = prune(connection, &self.history)?;
            if pruned > 0 {
                info!(pruned, "removed activity the history no longer keeps");
            }
            self.tidied = Some(std::time::Instant::now());
        }
        Ok(())
    }
}

/// Keeps the store for the history at `history` up to date until the process ends.
pub async fn keep_recorded(history: PathBuf) {
    let keeper = std::sync::Arc::new(std::sync::Mutex::new(Keeper::new(history)));
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        let keeper = keeper.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            keeper
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .tick()
        })
        .await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "couldn't keep the activity store"),
            Err(error) => warn!(%error, "keeping the activity store didn't finish"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trim::testing::{add, add_direct, history};
    use zirc_proto::activity::{Record, Wake};

    const SECOND: i64 = 1_000_000_000;

    fn zirc(message: &Message) -> String {
        zirc_proto::encode(message).unwrap()
    }

    #[test]
    fn keeps_who_when_and_what_kind_but_no_words() {
        crate::machines::the_tests_machines();
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(
            &path,
            "m1",
            "#api",
            10 * SECOND,
            "ada",
            "the login page is broken",
        );
        let mut post = Message::new(
            "cc-1",
            Event::Text {
                body: "taking: login".into(),
            },
        );
        post.task = Some("t-abc".into());
        post.name = Some("login".into());
        add(&path, "m2", "#api", 11 * SECOND, "ada-mbp", &zirc(&post));
        let landing = Message::new(
            "ci",
            Event::Custom {
                kind: zirc_proto::ci::LANDING_KIND.into(),
                data: json!({"state": "failed", "build": "7", "subject": "secret plans",
                             "failed": [{"target": "//web:e2e", "results": "https://x"}],
                             "fixes": ["build 6"], "cause": "test"}),
                text: "cc-1: build 7 failed".into(),
            },
        );
        add(
            &path,
            "m3",
            "#api/landings",
            12 * SECOND,
            "zirc-alerts",
            &zirc(&landing),
        );
        let other = Message::new(
            "x",
            Event::Custom {
                kind: "x-acme/deploy".into(),
                data: json!({"note": "words"}),
                text: "deployed".into(),
            },
        );
        add(&path, "m4", "#api", 13 * SECOND, "acme-ci", &zirc(&other));
        add_direct(&path, "m5", 14 * SECOND, "ada", "bob", "lunch?");

        let mut store = open(&path_for(&path)).unwrap();
        assert_eq!(catch_up(&mut store, &path).unwrap(), 5);
        let got = rows(&store, 0, 0, 100, false).unwrap();
        let kinds: Vec<_> = got.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["text", "text", "x-zirc/landing", "x-acme/deploy", "text"]
        );
        assert_eq!(got[0].account.as_deref(), Some("ada"));
        assert!(!got[0].agent);
        assert!(got[1].agent, "a machine's account is an agent's");
        assert_eq!(got[1].task.as_deref(), Some("t-abc"));
        assert_eq!(got[1].name.as_deref(), Some("login"));
        assert_eq!(got[1].level.as_deref(), Some("info"));
        assert_eq!(
            got[2].data,
            Some(
                json!({"state": "failed", "build": "7", "failed": ["//web:e2e"], "cause": "test",
                       "fixes": ["build 6"]})
            )
        );
        assert_eq!(got[2].level.as_deref(), Some("error"));
        assert_eq!(got[3].data, None, "another team's data could say anything");
        assert!(got[4].direct && got[4].channel.is_none());
        let all = serde_json::to_string(&got).unwrap();
        for words in ["broken", "taking", "secret", "words", "lunch", "https://x"] {
            assert!(!all.contains(words), "{words} in {all}");
        }
        // Read again, nothing new.
        assert_eq!(catch_up(&mut store, &path).unwrap(), 0);
        assert_eq!(
            rows(&store, 0, got[1].id, 2, false).unwrap()[0].id,
            got[2].id
        );
    }

    #[test]
    fn collapsing_leaves_out_a_working_set_said_again_in_the_same_hour() {
        crate::machines::the_tests_machines();
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let hour = 3_600 * SECOND;
        let working = |session: &str, paths: &[&str]| {
            zirc(&Message::new(
                session,
                Event::Working {
                    paths: paths.iter().map(|p| (*p).to_string()).collect(),
                    continued: false,
                    worktree: None,
                    branch: Some("login".into()),
                    part: None,
                    base: None,
                    committed: Vec::new(),
                },
            ))
        };
        let said = [
            ("m1", 10 * SECOND, working("cc-1", &["a.rs"])),
            ("m2", 20 * SECOND, working("cc-1", &["a.rs"])), // again: left out
            ("m3", 30 * SECOND, working("cc-2", &["a.rs"])), // another session
            ("m4", 40 * SECOND, working("cc-1", &["a.rs", "b.rs"])), // other paths
            ("m5", 50 * SECOND, working("cc-1", &["a.rs"])), // again: left out
            ("m6", hour + 10 * SECOND, working("cc-1", &["a.rs"])), // the next hour
            (
                "m7",
                hour + 20 * SECOND,
                zirc(&Message::new("cc-1", Event::Text { body: "hi".into() })),
            ),
        ];
        for (id, at, line) in &said {
            add(&path, id, "#api", *at, "ada-mbp", line);
        }
        let mut store = open(&path_for(&path)).unwrap();
        catch_up(&mut store, &path).unwrap();
        let msgids = |rows: &[Row]| {
            rows.iter()
                .map(|r| r.msgid.clone().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(&store, 0, 0, 100, false).unwrap().len(),
            said.len(),
            "every row unasked"
        );
        let all = rows(&store, 0, 0, 100, true).unwrap();
        assert_eq!(msgids(&all), ["m1", "m3", "m4", "m6", "m7"]);
        // Page by page, the pages agree on which rows are left out.
        let mut paged = Vec::new();
        let mut after = 0;
        loop {
            let page = rows(&store, 0, after, 2, true).unwrap();
            let Some(last) = page.last() else { break };
            after = last.id;
            paged.extend(page);
        }
        assert_eq!(msgids(&paged), msgids(&all));
        // Marked as recorded: a store of the format before is marked the same, its live
        // records kept.
        let wake = Message::new(
            "cc-1",
            Wake {
                id: "w1".into(),
                why: "mention".into(),
                kind: "text".into(),
                level: None,
                line: None,
                automatic: false,
            }
            .event(),
        );
        assert!(record_live(&store, hour + 30 * SECOND, Some("ada-mbp".into()), &wake).unwrap());
        store
            .execute_batch(
                "DROP INDEX rows_repeats; ALTER TABLE rows DROP COLUMN again; DROP TABLE firsts;
                 UPDATE meta SET value = '2' WHERE key = 'version';",
            )
            .unwrap();
        drop(store);
        let store = open(&path_for(&path)).unwrap();
        let again = rows(&store, 0, 0, 100, true).unwrap();
        assert_eq!(msgids(&again[..5]), ["m1", "m3", "m4", "m6", "m7"]);
        assert_eq!(again.len(), 6, "the live record kept");
    }

    #[test]
    fn a_row_in_agents_is_kept_under_its_repositorys_scope() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("activity.db")).unwrap();
        let said = |repo: Option<&str>| Message {
            repo: repo.map(str::to_owned),
            ..Message::new("cc-1", Event::Text { body: "x".into() })
        };
        for (at, channel, repo) in [(1, "#agents", Some("API")), (2, "#auth", Some("api"))] {
            let fact = Fact::of_message(Some("ada-mbp".into()), &said(repo));
            insert(&store, None, None, at, Some(channel), false, &fact).unwrap();
        }
        let kept = rows(&store, 0, 0, 10, true).unwrap();
        let channels: Vec<_> = kept.iter().map(|row| row.channel.as_deref()).collect();
        // A topic's line is the topic's, whatever repository it says.
        assert_eq!(channels, [Some("#api"), Some("#auth")]);
    }

    #[test]
    fn a_state_keeps_how_it_stands_and_not_why() {
        let state = zirc_proto::state::State {
            repo: "#api".into(),
            name: "master".into(),
            level: zirc_proto::state::Level::Broken,
            why: Some("the login page fails".into()),
            owner: Some("cc-1".into()),
            link: Some("https://x".into()),
            until: None,
        };
        let message = Message::new("ci", state.event());
        assert_eq!(
            metadata(&message),
            Some(json!({"name": "master", "level": "broken", "owner": "cc-1"}))
        );
    }

    #[test]
    fn a_request_keeps_which_and_where_it_stands_and_not_its_question() {
        let mut request = zirc_proto::request::Request {
            id: "ada-mbp/7".into(),
            question: "Push the secret plan?".into(),
            yes: "pushes it".into(),
            no: "drops it".into(),
            recommend: None,
            irreversible: true,
            session: "cc-1".into(),
            by: "ada".into(),
            state: zirc_proto::request::State::Open,
        };
        let open = metadata(&Message::new("cc-1", request.event())).unwrap();
        assert_eq!(
            open,
            json!({"id": "ada-mbp/7", "state": "open", "irreversible": true})
        );
        request.state = zirc_proto::request::State::Answered {
            answer: zirc_proto::request::Answer::Yes,
            emoji: "👍".into(),
            at: "2026-11-11T10:00:00Z".into(),
            recommended: false,
        };
        let answered = metadata(&Message::new("cc-1", request.event())).unwrap();
        assert_eq!(answered["state"], "answered");
        assert!(!answered.to_string().contains("secret"), "{answered}");
    }

    #[test]
    fn counts_each_hint_shown_and_gotcha_told_by_id_since_a_time() {
        use zirc_proto::activity::{GotchaTold, HintShown, Told};
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let store = open(&path_for(&path)).unwrap();
        let shown = |hint: &str| {
            Message::new(
                "cc-1",
                HintShown {
                    hint: hint.into(),
                    kind: "overlap".into(),
                    layer: "repo".into(),
                }
                .event(),
            )
        };
        let told = Message::new(
            "cc-1",
            GotchaTold {
                gotcha: "msg-1".into(),
                how: Told::Intent,
            }
            .event(),
        );
        for (at, message) in [
            (10, shown("aa")),
            (20, shown("aa")),
            (30, shown("bb")),
            (40, told.clone()),
            (5, told),
        ] {
            assert!(record_live(&store, at * SECOND, Some("ada-mbp".into()), &message).unwrap());
        }
        let (hints, gotchas) = playbook_usage(&store, 8 * SECOND).unwrap();
        assert_eq!(
            hints.get("aa"),
            Some(&Used {
                count: 2,
                last: 20 * SECOND
            })
        );
        assert_eq!(hints.get("bb").map(|used| used.count), Some(1));
        // The one told before the window isn't counted.
        assert_eq!(
            gotchas.get("msg-1"),
            Some(&Used {
                count: 1,
                last: 40 * SECOND
            })
        );
    }

    #[test]
    fn lists_one_items_uses_newest_first_with_how_many_each_day() {
        use zirc_proto::activity::{GotchaTold, HintShown, Told};
        const DAY: i64 = 86_400 * SECOND;
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let store = open(&path_for(&path)).unwrap();
        let shown = |hint: &str, layer: &str| {
            let mut message = Message::new(
                "cc-1",
                HintShown {
                    hint: hint.into(),
                    kind: "overlap".into(),
                    layer: layer.into(),
                }
                .event(),
            );
            message.name = Some("controller".into());
            message
        };
        let told = Message::new(
            "cc-2",
            GotchaTold {
                gotcha: "msg-1".into(),
                how: Told::Start,
            }
            .event(),
        );
        // Noon on day 10: the three days counted are days 8, 9 and 10 so far.
        let now = 10 * DAY + DAY / 2;
        for (at, message) in [
            (8 * DAY - SECOND, shown("aa", "repo")),
            (8 * DAY, shown("aa", "repo")),
            (10 * DAY + SECOND, shown("aa", "org")),
            (10 * DAY + 2 * SECOND, shown("aa", "repo")),
            (10 * DAY + 3 * SECOND, shown("bb", "repo")),
            (10 * DAY + 4 * SECOND, told),
        ] {
            assert!(record_live(&store, at, Some("ada-mbp".into()), &message).unwrap());
        }
        let (daily, uses) = playbook_uses(&store, Item::Hint("aa"), now, 3, None, 2).unwrap();
        assert_eq!(daily, [1, 0, 2]);
        assert_eq!(
            uses.iter()
                .map(|u| (u.nanotime, u.how.as_deref()))
                .collect::<Vec<_>>(),
            [
                (10 * DAY + 2 * SECOND, Some("repo")),
                (10 * DAY + SECOND, Some("org"))
            ]
        );
        assert_eq!(uses[0].account.as_deref(), Some("ada-mbp"));
        assert_eq!(uses[0].session.as_deref(), Some("cc-1"));
        assert_eq!(uses[0].name.as_deref(), Some("controller"));
        // The page after the last one shown.
        let (_, older) =
            playbook_uses(&store, Item::Hint("aa"), now, 3, Some(10 * DAY + SECOND), 2).unwrap();
        assert_eq!(
            older.iter().map(|u| u.nanotime).collect::<Vec<_>>(),
            [8 * DAY]
        );
        let (daily, uses) = playbook_uses(&store, Item::Gotcha("msg-1"), now, 3, None, 2).unwrap();
        assert_eq!(daily, [0, 0, 1]);
        assert_eq!(uses[0].how.as_deref(), Some("start"));
    }

    #[test]
    fn live_records_go_with_the_oldest_message_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#api", 100 * SECOND, "ada", "hello");
        let mut store = open(&path_for(&path)).unwrap();
        catch_up(&mut store, &path).unwrap();
        let wake = Message::new(
            "cc-1",
            Wake {
                id: "w1".into(),
                why: "mention".into(),
                kind: "text".into(),
                level: None,
                line: Some("m1".into()),
                automatic: false,
            }
            .event(),
        );
        assert!(record_live(&store, 50 * SECOND, Some("ada-mbp".into()), &wake).unwrap());
        assert!(record_live(&store, 150 * SECOND, Some("ada-mbp".into()), &wake).unwrap());
        let post = Message::new("cc-1", Event::Text { body: "hi".into() });
        assert!(
            !record_live(&store, 150 * SECOND, Some("ada-mbp".into()), &post).unwrap(),
            "only activity records are taken live"
        );
        assert!(
            !record_live(&store, 150 * SECOND, None, &wake).unwrap(),
            "nor without an account"
        );
        assert_eq!(
            prune(&mut store, &path).unwrap(),
            1,
            "older than the oldest message"
        );
        let left = rows(&store, 0, 0, 10, false).unwrap();
        assert_eq!(left.len(), 2);
        assert!(left[1].live);
        assert_eq!(left[1].kind, zirc_proto::activity::WAKE_KIND);
        // A history row Ergo dropped goes too.
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("DELETE FROM history", [])
            .unwrap();
        assert_eq!(prune(&mut store, &path).unwrap(), 1);
    }

    #[test]
    fn keeps_each_signals_first_from_each_account_past_pruning() {
        use zirc_proto::activity::{Notice, Noticed, Watching};
        use zirc_proto::onboarding::{First, Signal};
        crate::machines::the_tests_machines();
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#api", 100 * SECOND, "ada", "hello");
        let mut store = open(&path_for(&path)).unwrap();
        catch_up(&mut store, &path).unwrap();
        let watching = Message::new("cc-1", Watching {}.event());
        let overlap = Message::new(
            "cc-1",
            Notice {
                what: Noticed::Overlap,
                overlap: None,
                local: true,
                woke: true,
                escalated: false,
                with: None,
            }
            .event(),
        );
        for (at, account, message) in [
            (60, "ada-mbp", &watching),
            (40, "ada-mbp", &watching),
            (70, "bob-mbp", &watching),
            (80, "ada-mbp", &overlap),
            // A person's own account isn't an agent's.
            (90, "ada", &watching),
        ] {
            assert!(record_live(&store, at * SECOND, Some(account.into()), message).unwrap());
        }
        // Every live record is older than the oldest message, and goes; the firsts stay.
        assert_eq!(prune(&mut store, &path).unwrap(), 5);
        let first = |signal, account: &str, at: i64| First {
            signal,
            account: account.into(),
            at: at * 1_000,
        };
        assert_eq!(
            firsts(&store).unwrap(),
            [
                first(Signal::Agent, "ada-mbp", 40),
                first(Signal::Agent, "bob-mbp", 70),
                first(Signal::Overlap, "ada-mbp", 80),
            ]
        );
    }

    #[test]
    fn a_store_of_format_3_gets_its_firsts_from_its_rows() {
        crate::machines::the_tests_machines();
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#api", 100 * SECOND, "ada-mbp", "on it");
        add(&path, "m2", "#api", 200 * SECOND, "ada", "thanks");
        let mut store = open(&path_for(&path)).unwrap();
        catch_up(&mut store, &path).unwrap();
        store
            .execute_batch("DROP TABLE firsts; UPDATE meta SET value = '3' WHERE key = 'version';")
            .unwrap();
        drop(store);
        let store = open(&path_for(&path)).unwrap();
        assert_eq!(rows(&store, 0, 0, 10, false).unwrap().len(), 2, "rows kept");
        let firsts = firsts(&store).unwrap();
        assert_eq!(firsts.len(), 1, "{firsts:?}");
        assert_eq!(firsts[0].signal, zirc_proto::onboarding::Signal::Agent);
        assert_eq!(
            (firsts[0].account.as_str(), firsts[0].at),
            ("ada-mbp", 100_000)
        );
    }

    #[test]
    fn keeps_a_landing_anyone_but_ci_says_as_words() {
        let data =
            serde_json::json!({"build": "7", "branch": "x", "state": "failed", "session": "cc-a"});
        let message = Message::new(
            "cc-a",
            Event::Custom {
                kind: zirc_proto::ci::LANDING_KIND.into(),
                text: "landed".into(),
                data,
            },
        );
        let forged = Fact::of_message(Some("ada-mbp".into()), &message);
        assert_eq!((forged.kind.as_str(), forged.data), ("text", None));
        let ci = Fact::of_message(Some(zirc_proto::ci::NICK.into()), &message);
        assert_eq!(ci.kind, zirc_proto::ci::LANDING_KIND);
        assert!(ci.data.is_some());
    }

    #[test]
    fn serves_a_landing_kept_from_anyone_but_ci_before_as_words() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("activity.db")).unwrap();
        let landing = |account: &str| Fact {
            account: Some(account.into()),
            kind: zirc_proto::ci::LANDING_KIND.into(),
            data: Some(json!({"build": "7", "state": "failed"})),
            ..Fact::default()
        };
        insert(
            &store,
            None,
            None,
            1,
            Some("#api/landings"),
            false,
            &landing("ada-mbp"),
        )
        .unwrap();
        insert(
            &store,
            None,
            None,
            2,
            Some("#api/landings"),
            false,
            &landing("zirc-ci"),
        )
        .unwrap();
        let got = rows(&store, 0, 0, 100, false).unwrap();
        let kinds: Vec<_> = got
            .iter()
            .map(|r| (r.kind.as_str(), r.data.is_some()))
            .collect();
        assert_eq!(
            kinds,
            [("text", false), (zirc_proto::ci::LANDING_KIND, true)]
        );
    }
}
