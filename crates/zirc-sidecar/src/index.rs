//! A full-text index of a server's history, so a search for a word said months ago needn't
//! read every message since (`crate::history`).
//!
//! It lives in its own SQLite file beside Ergo's history ([`path_for`]), which Ergo owns and
//! [`crate::trim`] rewrites: the index never touches Ergo's file. It is derived: each row is
//! keyed on Ergo's own row id, it is filled by reading the history from the last id it has
//! ([`catch_up`]), and it is thrown away and built again when its format changes.
//!
//! It never makes a deleted message findable. A search joins every candidate back to Ergo's
//! history, so a row Ergo no longer has (redacted by its author, expired by retention, removed
//! by an owner, a guest forgotten) is never returned, whatever the index still holds; and
//! [`prune`] takes such rows out of the index too, with `secure_delete` on and its log
//! emptied, so their words leave the disk as [`crate::trim`] makes them leave Ergo's.
//!
//! It holds no text: the FTS5 table is contentless (`content=''`), so what was said stays in
//! Ergo's history alone, and the index only says which rows have which three-letter runs: of
//! a zirc message's [`zirc_proto::Message::search_text`], its data's values included.
//! Who may read a row is checked when searching, never stored here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use tracing::{info, warn};

/// The index's format. A file with another is built again from Ergo's history.
pub const VERSION: &str = "3";

/// How many history rows one step of [`catch_up`] reads and commits.
const BATCH: usize = 1_000;

/// How long a write waits for another to finish.
const BUSY: Duration = Duration::from_secs(10);

/// How often [`keep_indexed`] reads what's new in the history.
pub const EVERY: Duration = Duration::from_secs(5);

/// How often [`keep_indexed`] checks every row and compacts the index.
const TIDY: Duration = Duration::from_hours(1);

/// The shortest text the index can find: FTS5's trigram tokenizer needs three characters.
pub const SHORTEST: usize = 3;

/// The index for the history at `history`: `zirc_search.db` in the same directory.
#[must_use]
pub fn path_for(history: &Path) -> PathBuf {
    history.with_file_name("zirc_search.db")
}

/// The index, opened to write, made or made again if it's missing or of another format.
///
/// # Errors
///
/// When the file can't be opened or made.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let connection = open_file(path)?;
    let version: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
            row.get(0)
        })
        .optional()
        .unwrap_or(None);
    if version.as_deref() == Some(VERSION) {
        return Ok(connection);
    }
    if version.is_some() {
        info!(
            ?version,
            "the search index's format changed: building it again"
        );
    }
    drop(connection);
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        match std::fs::remove_file(PathBuf::from(file)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(rusqlite::Error::InvalidPath(path.to_owned()));
            }
            _ => {}
        }
    }
    let connection = open_file(path)?;
    connection.execute_batch(&format!(
        "BEGIN;
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         -- One row per history row: where and when it was said, Ergo's id as the key.
         CREATE TABLE rows (
             id INTEGER PRIMARY KEY,
             msgid TEXT NOT NULL,
             nanotime INTEGER NOT NULL,
             channel TEXT,
             a TEXT,
             b TEXT,
             sender TEXT,
             said INTEGER NOT NULL,
             session TEXT
         );
         CREATE INDEX rows_time ON rows (nanotime, id);
         CREATE INDEX rows_sender ON rows (sender, nanotime);
         CREATE INDEX rows_msgid ON rows (msgid);
         -- Three-letter runs of each row's text, case folded; the text itself isn't kept.
         CREATE VIRTUAL TABLE grams USING fts5(
             text, content='', contentless_delete=1, tokenize='trigram'
         );
         INSERT INTO meta VALUES ('version', '{VERSION}'), ('last', '0'), ('built', '0');
         COMMIT;"
    ))?;
    Ok(connection)
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
    // Freed pages are zeroed, so a pruned row's runs don't stay in the file.
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA secure_delete=ON;")?;
    Ok(connection)
}

fn meta(connection: &Connection, key: &str) -> rusqlite::Result<i64> {
    connection.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
            .map(|value| value.parse().unwrap_or(0))
    })
}

fn set_meta(connection: &Connection, key: &str, value: i64) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE meta SET value = ?2 WHERE key = ?1",
        params![key, value.to_string()],
    )?;
    Ok(())
}

/// Whether the index has caught up with the history at least once, so it can answer searches.
///
/// # Errors
///
/// When the index can't be read.
pub fn built(connection: &Connection) -> rusqlite::Result<bool> {
    Ok(meta(connection, "built")? == 1)
}

/// Runs `step` with Ergo's history at `history` attached to `connection` as `hist`, read-only,
/// and detaches it after, so the index never keeps Ergo's file open between steps, where it
/// would stand in the way of Ergo's writes and checkpoints.
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

/// Indexes every history row newer than the last indexed, [`BATCH`] at a time, and says how
/// many it read. Once it reaches the end, the index is [`built`].
///
/// # Errors
///
/// When either database can't be read or the index written.
pub fn catch_up(connection: &mut Connection, history: &Path) -> rusqlite::Result<usize> {
    with_history(connection, history, read_new)
}

fn read_new(connection: &mut Connection) -> rusqlite::Result<usize> {
    let mut read = 0;
    loop {
        let last = meta(connection, "last")?;
        let rows = {
            // A channel's message is in `sequence`; a direct message in `conversations`, twice.
            let mut statement = connection.prepare_cached(
                "SELECT h.id, h.msgid, h.data, s.target, s.nanotime, \
                        c.target, c.correspondent, c.nanotime \
                 FROM hist.history h \
                 LEFT JOIN hist.sequence s ON s.history_id = h.id \
                 LEFT JOIN hist.conversations c ON c.history_id = h.id \
                 WHERE h.id > ?1 ORDER BY h.id LIMIT ?2",
            )?;
            let rows = statement.query_map(
                params![last, i64::try_from(BATCH * 2).unwrap_or(i64::MAX)],
                |row| {
                    Ok(Item {
                        id: row.get(0)?,
                        msgid: row.get(1)?,
                        data: row.get(2)?,
                        channel: row.get::<_, Option<String>>(3)?,
                        channel_time: row.get(4)?,
                        target: row.get(5)?,
                        correspondent: row.get(6)?,
                        direct_time: row.get(7)?,
                    })
                },
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let Some(newest) = rows.last().map(|item| item.id) else {
            set_meta(connection, "built", 1)?;
            return Ok(read);
        };
        let transaction = connection.transaction()?;
        let mut previous = None;
        for item in &rows {
            // A direct message's second row, already indexed under its first.
            if previous == Some(item.id) {
                continue;
            }
            previous = Some(item.id);
            read += 1;
            let Some((channel, pair, nanotime)) = item.place() else {
                continue;
            };
            let Some(found) = crate::history::indexed(&item.data) else {
                continue;
            };
            transaction.execute(
                "INSERT OR REPLACE INTO rows \
                 (id, msgid, nanotime, channel, a, b, sender, said, session) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    item.id,
                    item.msgid,
                    nanotime,
                    channel,
                    pair.0,
                    pair.1,
                    found.sender,
                    found.said,
                    found.session
                ],
            )?;
            transaction.execute(
                "INSERT INTO grams (rowid, text) VALUES (?1, ?2)",
                params![item.id, found.text],
            )?;
        }
        set_meta(&transaction, "last", newest)?;
        transaction.commit()?;
    }
}

/// A history row, as [`catch_up`] reads it.
struct Item {
    id: i64,
    msgid: String,
    data: Vec<u8>,
    channel: Option<String>,
    channel_time: Option<i64>,
    target: Option<String>,
    correspondent: Option<String>,
    direct_time: Option<i64>,
}

/// Where a row was said: a channel, or a direct message's two accounts in order.
type Place = (Option<String>, (Option<String>, Option<String>), i64);

impl Item {
    fn place(&self) -> Option<Place> {
        if let (Some(channel), Some(time)) = (&self.channel, self.channel_time) {
            return channel
                .starts_with('#')
                .then(|| (Some(channel.clone()), (None, None), time));
        }
        let (a, b, time) = (
            self.target.as_ref()?.to_lowercase(),
            self.correspondent.as_ref()?.to_lowercase(),
            self.direct_time?,
        );
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        Some((None, (Some(a), Some(b)), time))
    }
}

/// Takes out of the index every row Ergo's history no longer has, and says how many.
///
/// # Errors
///
/// When either database can't be read or the index written.
pub fn prune(connection: &mut Connection, history: &Path) -> rusqlite::Result<usize> {
    let pruned = with_history(connection, history, prune_gone)?;
    if pruned > 0 {
        checkpoint(connection)?;
    }
    Ok(pruned)
}

fn prune_gone(connection: &mut Connection) -> rusqlite::Result<usize> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS gone (id INTEGER PRIMARY KEY);
         DELETE FROM temp.gone;
         INSERT INTO temp.gone SELECT id FROM rows WHERE id NOT IN (SELECT id FROM hist.history);
         DELETE FROM grams WHERE rowid IN (SELECT id FROM temp.gone);",
    )?;
    let pruned = transaction.execute(
        "DELETE FROM rows WHERE id IN (SELECT id FROM temp.gone)",
        [],
    )?;
    transaction.commit()?;
    Ok(pruned)
}

/// Takes one message out of the index, before it's removed from Ergo's history, so a crash
/// between the two leaves it unfindable rather than findable.
///
/// # Errors
///
/// When the index can't be written.
pub fn forget(connection: &mut Connection, msgid: &str) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "DELETE FROM grams WHERE rowid IN (SELECT id FROM rows WHERE msgid = ?1)",
        [msgid],
    )?;
    transaction.execute("DELETE FROM rows WHERE msgid = ?1", [msgid])?;
    transaction.commit()?;
    checkpoint(connection)
}

/// Copies the log into the file and truncates it, so no removed row stays in the log.
fn checkpoint(connection: &Connection) -> rusqlite::Result<()> {
    let busy: i64 =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        warn!("the search index's log couldn't be emptied: a reader was in the way");
    }
    Ok(())
}

/// Merges FTS5's segments and rewrites the file without free pages.
fn compact(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute("INSERT INTO grams (grams) VALUES ('optimize')", [])?;
    connection.execute_batch("VACUUM")?;
    checkpoint(connection)
}

/// FTS5's query for text containing `text`, any case, or `None` when it's too short for the
/// index to find.
#[must_use]
pub fn phrase(text: &str) -> Option<String> {
    let text = text.trim();
    (text.chars().count() >= SHORTEST).then(|| format!("\"{}\"", text.replace('"', "\"\"")))
}

/// What [`keep_indexed`] does each time: catches up; prunes when rows went from the history
/// since it last looked; and, every [`TIDY`], prunes and compacts anyway.
pub struct Keeper {
    history: PathBuf,
    connection: Option<Connection>,
    gone: Option<i64>,
    tidied: Option<std::time::Instant>,
}

impl Keeper {
    /// A keeper of the index for the history at `history`.
    #[must_use]
    pub fn new(history: PathBuf) -> Self {
        Self {
            history,
            connection: None,
            gone: None,
            tidied: None,
        }
    }

    /// One round.
    ///
    /// # Errors
    ///
    /// When either database can't be read or the index written.
    pub fn tick(&mut self) -> rusqlite::Result<()> {
        if !self.history.exists() {
            return Ok(());
        }
        let connection = match &mut self.connection {
            Some(connection) => connection,
            empty => empty.insert(open(&path_for(&self.history))?),
        };
        let read = catch_up(connection, &self.history)?;
        if read > 0 {
            info!(read, "indexed for search");
        }
        let gone = crate::trim::gone(&crate::trim::open(&self.history)?)?;
        let due = self.tidied.is_none_or(|at| at.elapsed() >= TIDY);
        if due || gone > self.gone.unwrap_or(0) {
            let pruned = prune(connection, &self.history)?;
            if pruned > 0 {
                info!(
                    pruned,
                    "removed from the search index what went from the history"
                );
            }
        }
        if due {
            compact(connection)?;
            self.tidied = Some(std::time::Instant::now());
        }
        self.gone = Some(gone);
        Ok(())
    }
}

/// Keeps the index of the history at `history` up to date until the process ends.
pub async fn keep_indexed(history: PathBuf) {
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
            Ok(Err(error)) => warn!(%error, "couldn't keep the search index"),
            Err(error) => warn!(%error, "keeping the search index didn't finish"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trim::testing::{add, add_direct, history};

    const SECOND: i64 = 1_000_000_000;

    fn ids(connection: &Connection, text: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT r.msgid FROM grams g JOIN rows r ON r.id = g.rowid \
                 WHERE grams MATCH ?1 ORDER BY r.id",
            )
            .unwrap();
        statement
            .query_map([phrase(text).unwrap()], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn the_bundled_sqlite_has_fts5_with_trigrams_and_contentless_delete() {
        let dir = tempfile::tempdir().unwrap();
        open(&dir.path().join("zirc_search.db")).unwrap();
    }

    #[test]
    fn it_indexes_channels_and_direct_messages_once_each_and_finds_any_case_substrings() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(
            &path,
            "m1",
            "#team",
            10 * SECOND,
            "alice",
            "the Gateway timed out",
        );
        add(
            &path,
            "m2",
            "#team",
            20 * SECOND,
            "bob",
            "src/proxy.rs:42 panics",
        );
        add_direct(
            &path,
            "d1",
            30 * SECOND,
            "alice",
            "bob",
            "the gateway again?",
        );
        let mut index = open(&path_for(&path)).unwrap();
        assert!(!built(&index).unwrap());
        assert_eq!(catch_up(&mut index, &path).unwrap(), 3);
        assert!(built(&index).unwrap());
        assert_eq!(ids(&index, "GATEWAY"), ["m1", "d1"]);
        assert_eq!(ids(&index, "proxy.rs:4"), ["m2"]);
        // A direct message is one row, its two accounts in order.
        let pair: (Option<String>, String, String) = index
            .query_row(
                "SELECT channel, a, b FROM rows WHERE msgid = 'd1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(pair, (None, "alice".into(), "bob".into()));
        // Nothing new: nothing read.
        assert_eq!(catch_up(&mut index, &path).unwrap(), 0);
        add(&path, "m3", "#team", 40 * SECOND, "carol", "gateway fixed");
        assert_eq!(catch_up(&mut index, &path).unwrap(), 1);
        assert_eq!(ids(&index, "gateway"), ["m1", "d1", "m3"]);
        assert!(phrase("ab").is_none());
    }

    #[test]
    fn what_goes_from_the_history_goes_from_the_index_and_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(
            &path,
            "m1",
            "#team",
            10 * SECOND,
            "alice",
            "hunter2-secret-token",
        );
        add(&path, "m2", "#team", 20 * SECOND, "bob", "a secret plan");
        let index_path = path_for(&path);
        let mut index = open(&index_path).unwrap();
        catch_up(&mut index, &path).unwrap();
        // Ergo redacts m1 (or retention expires it): the index still has it until pruned.
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "DELETE FROM sequence WHERE history_id = (SELECT id FROM history WHERE msgid = 'm1');
                 DELETE FROM history WHERE msgid = 'm1';",
            )
            .unwrap();
        assert_eq!(ids(&index, "secret"), ["m1", "m2"]);
        assert_eq!(prune(&mut index, &path).unwrap(), 1);
        assert_eq!(ids(&index, "secret"), ["m2"]);
        // An owner's removal: out of the index before Ergo is asked.
        forget(&mut index, "m2").unwrap();
        assert!(ids(&index, "secret").is_empty());
        compact(&index).unwrap();
        drop(index);
        // The index never held the text, and pruning left none of its runs behind.
        let bytes = std::fs::read(&index_path).unwrap();
        let found = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(!found(b"hunter2"), "the index file has the text");
        assert!(!found(b"secret plan"), "the index file has the text");
    }

    #[test]
    fn an_index_of_another_format_is_built_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#team", 10 * SECOND, "alice", "morning all");
        let index_path = path_for(&path);
        let mut index = open(&index_path).unwrap();
        catch_up(&mut index, &path).unwrap();
        index
            .execute("UPDATE meta SET value = '0' WHERE key = 'version'", [])
            .unwrap();
        drop(index);
        let mut index = open(&index_path).unwrap();
        assert!(!built(&index).unwrap());
        assert_eq!(catch_up(&mut index, &path).unwrap(), 1);
        assert_eq!(ids(&index, "morning"), ["m1"]);
    }

    #[test]
    fn the_keeper_catches_up_and_prunes_when_rows_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#team", 10 * SECOND, "alice", "first words");
        let mut keeper = Keeper::new(path.clone());
        keeper.tick().unwrap();
        add(&path, "m2", "#team", 20 * SECOND, "bob", "second words");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "DELETE FROM sequence WHERE history_id = (SELECT id FROM history WHERE msgid = 'm1');
                 DELETE FROM history WHERE msgid = 'm1';",
            )
            .unwrap();
        keeper.tick().unwrap();
        let index = open(&path_for(&path)).unwrap();
        assert_eq!(ids(&index, "words"), ["m2"]);
    }
}
