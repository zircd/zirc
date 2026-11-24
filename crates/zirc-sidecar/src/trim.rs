//! Removing single messages from a tenant's history, such as a secret someone pasted into a
//! channel, and making sure they're gone from Ergo's disk too.
//!
//! Ergo removes a message with `REDACT` (`draft/message-redaction`): its author, the channel's
//! ops, or the operator the sidecar logs in as, for the organisation's owners. Clients stop
//! getting it back through CHATHISTORY at once, and those that take the capability drop it
//! from what they show. But Ergo's SQLite only marks the row's space free: the message's bytes
//! stay in `ergo_history.db` and its write-ahead log until they happen to be overwritten.
//! [`keep_scrubbed`] notices that rows went (removed, or past the history's retention) and
//! rewrites the database without them, while Ergo runs, as `VACUUM` does.
//!
//! What this can't reach: the server's daily disk snapshots, kept seven days. Cloud Logging
//! never has the message: Ergo is configured never to log userinput or useroutput, and the
//! operator's log sinks drop IRC lines besides.

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use tracing::{info, warn};

use crate::exporter::{HISTORY_VERSION, MESSAGE_TYPES};

/// How often [`keep_scrubbed`] looks for rows that went.
pub const EVERY: Duration = Duration::from_mins(5);

/// How long the scrub waits for Ergo to finish writing, and Ergo for the scrub.
const BUSY: Duration = Duration::from_secs(10);

/// A message in a channel's history, as owners see it to choose what to remove.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    /// The message's id.
    pub msgid: String,
    /// The channel it was in.
    pub channel: String,
    /// RFC 3339.
    pub time: String,
    /// The nick it was sent under.
    pub nick: String,
    /// The account it was sent from, if any.
    pub account: Option<String>,
    /// What people read: a zirc message's text, without its framing.
    pub text: String,
}

/// Up to `limit` of the newest channel messages containing `query` (ASCII letters in any
/// case), in `channel` if given, newest first. Never direct messages.
///
/// # Errors
///
/// When the history can't be read, or has a schema this doesn't know.
pub fn find(
    path: &Path,
    channel: Option<&str>,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<Message>> {
    let connection = open(path)?;
    let pattern = format!(
        "%{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut statement = connection.prepare(&format!(
        "SELECT h.msgid, s.target, s.nanotime, h.data FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.target LIKE '#%' AND (?1 IS NULL OR s.target = lower(?1)) \
         AND json_extract(h.data, '$.Type') IN ({MESSAGE_TYPES}) \
         AND json_extract(h.data, '$.Message.Message') LIKE ?2 ESCAPE '\\' \
         ORDER BY s.nanotime DESC LIMIT ?3"
    ))?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = statement.query_map(rusqlite::params![channel, pattern, limit], |row| {
        let data = row.get_ref(3)?.as_bytes()?.to_vec();
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            data,
        ))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let (msgid, channel, nanotime, data) = row?;
        let Ok(item) = serde_json::from_slice::<Item>(&data) else {
            continue;
        };
        found.push(Message {
            msgid,
            channel,
            time: Timestamp::from_nanosecond(i128::from(nanotime))
                .map(|t| t.to_string())
                .unwrap_or_default(),
            nick: item.nick,
            account: Some(item.account).filter(|a| a != "*" && !a.is_empty()),
            text: readable(&item.message.message),
        });
    }
    Ok(found)
}

/// Every id Ergo's history keeps under `channel`, of any kind: what deleting the channel's
/// history deletes ([`crate::ergo::Operator::delete_messages`]).
///
/// # Errors
///
/// When the history can't be read, or has a schema this doesn't know.
pub fn msgids_in(path: &Path, channel: &str) -> rusqlite::Result<Vec<String>> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT h.msgid FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.target = lower(?1) AND h.msgid != '' ORDER BY s.nanotime",
    )?;
    let ids = statement.query_map([channel], |row| row.get::<_, String>(0))?;
    ids.collect()
}

/// The channel message `msgid` was sent to, if it's a channel message in the history.
///
/// # Errors
///
/// When the history can't be read, or has a schema this doesn't know.
pub fn channel_of(path: &Path, msgid: &str) -> rusqlite::Result<Option<String>> {
    open(path)?
        .query_row(
            "SELECT s.target FROM history h JOIN sequence s ON s.history_id = h.id \
             WHERE h.msgid = ?1 AND s.target LIKE '#%'",
            [msgid],
            |row| row.get(0),
        )
        .optional()
}

/// Rewrites the history without what was removed from it, and empties its write-ahead log,
/// so no copy of a removed message is left in either. Ergo waits meanwhile.
///
/// # Errors
///
/// When the database can't be opened or rewritten.
pub fn scrub(path: &Path) -> rusqlite::Result<()> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(BUSY)?;
    // Ergo's pages into the database first, as VACUUM leaves the log for what it writes.
    checkpoint(&connection)?;
    connection.execute_batch("VACUUM")?;
    checkpoint(&connection)
}

/// Copies the log into the database and truncates it to nothing.
fn checkpoint(connection: &Connection) -> rusqlite::Result<()> {
    let busy: i64 =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        warn!("Ergo's history log couldn't be emptied: a reader was in the way");
    }
    Ok(())
}

/// Scrubs the history whenever rows went from it since the last look.
pub struct Scrubber {
    path: PathBuf,
    /// The ids Ergo had given out but no longer has, as of the last look: kept beside the
    /// history ([`mark_of`]), so a sidecar starting again doesn't rewrite it all when nothing
    /// went meanwhile, as every rollout did, and Ergo's writes waited on it.
    gone: Option<i64>,
}

/// Where a scrubber keeps how many rows had gone at its last look, beside the history.
fn mark_of(path: &Path) -> PathBuf {
    path.with_extension("scrubbed")
}

impl Scrubber {
    /// A scrubber of the history database at `path`, as the last one there left it.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        let gone = std::fs::read_to_string(mark_of(&path))
            .ok()
            .and_then(|mark| mark.trim().parse().ok());
        Self { path, gone }
    }

    /// Scrubs if rows went since the last call, or on the first call if any ever did, and says
    /// whether it did.
    ///
    /// # Errors
    ///
    /// When the history can't be read or scrubbed.
    pub fn tick(&mut self) -> rusqlite::Result<bool> {
        if !self.path.exists() {
            return Ok(false);
        }
        let gone = gone(&open(&self.path)?)?;
        let scrub = gone > self.gone.unwrap_or(0);
        if scrub {
            self::scrub(&self.path)?;
        }
        if self.gone != Some(gone)
            && let Err(error) = std::fs::write(mark_of(&self.path), gone.to_string())
        {
            warn!(%error, "couldn't keep the scrub's mark: a restart scrubs again");
        }
        self.gone = Some(gone);
        Ok(scrub)
    }
}

/// History ids given out whose rows are gone. Ergo's ids only grow (AUTOINCREMENT), so this
/// grows with every row removed, whichever it was.
pub(crate) fn gone(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT coalesce((SELECT seq FROM sqlite_sequence WHERE name = 'history'), 0) \
         - (SELECT count(*) FROM history)",
        [],
        |row| row.get(0),
    )
}

/// Runs a [`Scrubber`] every [`EVERY`] until the process ends.
pub async fn keep_scrubbed(path: PathBuf) {
    let scrubber = std::sync::Arc::new(std::sync::Mutex::new(Scrubber::new(path)));
    let mut every = tokio::time::interval(EVERY);
    loop {
        every.tick().await;
        let scrubber = scrubber.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            scrubber
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .tick()
        })
        .await;
        match outcome {
            Ok(Ok(true)) => info!("history scrubbed of what was removed"),
            Ok(Ok(false)) => {}
            Ok(Err(error)) => warn!(%error, "couldn't scrub Ergo's history"),
            Err(error) => warn!(%error, "the scrub didn't finish"),
        }
    }
}

/// The history, read-only, if its schema is the one this knows.
pub(crate) fn open(path: &Path) -> rusqlite::Result<Connection> {
    match crate::exporter::open_history(path)? {
        crate::exporter::Opened::Open(connection) => {
            connection.busy_timeout(BUSY)?;
            Ok(connection)
        }
        crate::exporter::Opened::Missing => Err(rusqlite::Error::InvalidPath(path.to_owned())),
        crate::exporter::Opened::Unknown(version) => Err(rusqlite::Error::InvalidParameterName(
            format!("Ergo's history has schema {version:?}, not {HISTORY_VERSION}"),
        )),
    }
}

/// The parts of Ergo's history item this reads.
#[derive(serde::Deserialize)]
struct Item {
    #[serde(rename = "Nick")]
    nick: String,
    #[serde(rename = "AccountName", default)]
    account: String,
    #[serde(rename = "Message")]
    message: Body,
}

#[derive(serde::Deserialize)]
struct Body {
    #[serde(rename = "Message")]
    message: String,
}

/// A message's text as people read it, as zirc-proto reads one.
fn readable(message: &str) -> String {
    zirc_proto::readable(message)
}

#[cfg(test)]
mod tests {
    use super::testing::{add, history};
    use super::*;

    #[test]
    fn zirc_messages_read_as_their_text() {
        assert_eq!(readable("plain words"), "plain words");
        assert_eq!(readable("\x01ACTION ZIRC eyJ9 :the text\x01"), "the text");
        assert_eq!(readable("\x01ACTION waves\x01"), "* waves");
    }

    #[test]
    fn owners_find_channel_messages_but_never_direct_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(
            &path,
            "m1",
            "#api",
            1_000_000_000,
            "alice",
            "key is SECRET-1",
        );
        add(
            &path,
            "m2",
            "#ops",
            2_000_000_000,
            "bob",
            "\x01ACTION ZIRC e30 :secret-2 here\x01",
        );
        add(
            &path,
            "m3",
            "bob",
            3_000_000_000,
            "alice",
            "a secret in a DM",
        );
        add(&path, "m4", "#api", 4_000_000_000, "carol", "100%_sure");

        let found = find(&path, None, "secret", 10).unwrap();
        assert_eq!(
            found.iter().map(|m| m.msgid.as_str()).collect::<Vec<_>>(),
            ["m2", "m1"],
            "newest first, and no DM"
        );
        assert_eq!(found[0].text, "secret-2 here");
        assert_eq!(found[0].channel, "#ops");
        assert_eq!(found[1].time, "1970-01-01T00:00:01Z");
        assert_eq!(found[1].account.as_deref(), Some("alice"));

        assert_eq!(find(&path, Some("#API"), "secret", 10).unwrap().len(), 1);
        assert_eq!(
            find(&path, None, "%_", 10).unwrap()[0].msgid,
            "m4",
            "% and _ are literal"
        );
        assert_eq!(find(&path, None, "", 1).unwrap()[0].msgid, "m4");

        assert_eq!(channel_of(&path, "m1").unwrap().as_deref(), Some("#api"));
        assert_eq!(
            channel_of(&path, "m3").unwrap(),
            None,
            "a DM isn't a channel's"
        );
        assert_eq!(channel_of(&path, "nope").unwrap(), None);
    }

    #[test]
    fn with_nothing_removed_every_message_stays_however_old() {
        // History kept for ever (Ergo's expire-time 0): rows go only when someone removes one,
        // so the scrub rewrites nothing and the oldest message stays.
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "first", "#api", 0, "alice", "the very first message");
        add(
            &path,
            "later",
            "#api",
            9_000_000_000_000_000_000,
            "bob",
            "years later",
        );
        let mut scrubber = Scrubber::new(path.clone());
        for _ in 0..3 {
            assert!(!scrubber.tick().unwrap());
        }
        assert_eq!(find(&path, None, "very first", 5).unwrap().len(), 1);
        assert_eq!(find(&path, None, "years later", 5).unwrap().len(), 1);
    }

    #[test]
    fn a_removed_message_is_scrubbed_from_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        for i in 0..300 {
            add(
                &path,
                &format!("f{i}"),
                "#api",
                i,
                "alice",
                &format!("filler {i} {}", "x".repeat(200)),
            );
        }
        add(&path, "s", "#api", 1000, "bob", "hunter2-SECRET");
        for i in 0..300 {
            add(
                &path,
                &format!("g{i}"),
                "#api",
                2000 + i,
                "alice",
                &format!("later {i} {}", "y".repeat(200)),
            );
        }
        // A connection held open throughout, as Ergo's is.
        let ergo = Connection::open(&path).unwrap();
        let on_disk = |needle: &[u8]| {
            std::fs::read_dir(dir.path()).unwrap().any(|entry| {
                let bytes = std::fs::read(entry.unwrap().path()).unwrap();
                bytes.windows(needle.len()).any(|w| w == needle)
            })
        };

        let mut scrubber = Scrubber::new(path.clone());
        assert!(!scrubber.tick().unwrap(), "nothing went yet");

        ergo.execute_batch(
            "DELETE FROM sequence WHERE history_id = (SELECT id FROM history WHERE msgid = 's');
             DELETE FROM history WHERE msgid = 's';",
        )
        .unwrap();
        assert!(
            on_disk(b"hunter2-SECRET"),
            "deleting alone leaves the bytes"
        );

        assert!(scrubber.tick().unwrap());
        assert!(!on_disk(b"hunter2-SECRET"));
        assert!(!scrubber.tick().unwrap(), "nothing more went");

        // Ergo carries on writing afterwards.
        add(&path, "n", "#api", 9000, "bob", "after the scrub");
        assert_eq!(find(&path, None, "after the scrub", 5).unwrap().len(), 1);

        // A sidecar starting again picks up where this one left off: nothing went since.
        assert!(!Scrubber::new(path.clone()).tick().unwrap());
        ergo.execute_batch("DELETE FROM history WHERE msgid = 'n';")
            .unwrap();
        assert!(Scrubber::new(path.clone()).tick().unwrap(), "but this did");
    }
}

/// Ergo's history, made up for tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};

    use rusqlite::Connection;

    /// A history with Ergo's schema, as far as this reads it.
    pub(crate) fn history(dir: &Path) -> PathBuf {
        let path = dir.join("ergo_history.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE metadata (key_name TEXT PRIMARY KEY, value TEXT);
                 INSERT INTO metadata VALUES ('db.version', '2');
                 CREATE TABLE history (id INTEGER PRIMARY KEY AUTOINCREMENT, data BLOB NOT NULL, msgid TEXT NOT NULL);
                 CREATE TABLE sequence (history_id INTEGER NOT NULL PRIMARY KEY, target TEXT NOT NULL, nanotime INTEGER NOT NULL);
                 CREATE TABLE conversations (id INTEGER PRIMARY KEY AUTOINCREMENT, target TEXT NOT NULL, correspondent TEXT NOT NULL, nanotime INTEGER NOT NULL, history_id INTEGER NOT NULL);",
            )
            .unwrap();
        path
    }

    pub(crate) fn add(
        path: &Path,
        msgid: &str,
        target: &str,
        nanotime: i64,
        nick: &str,
        text: &str,
    ) {
        let data = serde_json::json!({
            "Type": 1, "Nick": format!("{nick}!u@h"), "AccountName": nick,
            "Message": {"Message": text, "Msgid": msgid},
        });
        add_item(path, msgid, target, nanotime, &data);
    }

    /// Any history item, as Ergo stores it.
    pub(crate) fn add_item(
        path: &Path,
        msgid: &str,
        target: &str,
        nanotime: i64,
        data: &serde_json::Value,
    ) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "INSERT INTO history (data, msgid) VALUES (?1, ?2)",
                rusqlite::params![data.to_string().into_bytes(), msgid],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sequence VALUES (last_insert_rowid(), ?1, ?2)",
                rusqlite::params![target, nanotime],
            )
            .unwrap();
    }

    /// A message's row alone, as Ergo's first statement for one keeps it: where it was said
    /// follows ([`add_copy`]). Its id.
    pub(crate) fn add_message(path: &Path, msgid: &str, from: &str, text: &str) -> i64 {
        let data = serde_json::json!({
            "Type": 1, "Nick": format!("{from}!u@h"), "AccountName": from,
            "Message": {"Message": text, "Msgid": msgid},
        });
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "INSERT INTO history (data, msgid) VALUES (?1, ?2)",
                rusqlite::params![data.to_string().into_bytes(), msgid],
            )
            .unwrap();
        connection.last_insert_rowid()
    }

    /// One copy of the direct message `id`, under `account`, with `correspondent`.
    pub(crate) fn add_copy(path: &Path, id: i64, account: &str, correspondent: &str) {
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO conversations (target, correspondent, nanotime, history_id) \
                 VALUES (?1, ?2, 1, ?3)",
                rusqlite::params![account, correspondent, id],
            )
            .unwrap();
    }

    /// A direct message from `from` to `to`, kept for both, as Ergo keeps one.
    pub(crate) fn add_direct(
        path: &Path,
        msgid: &str,
        nanotime: i64,
        from: &str,
        to: &str,
        text: &str,
    ) {
        let data = serde_json::json!({
            "Type": 1, "Nick": format!("{from}!u@h"), "AccountName": from,
            "Message": {"Message": text, "Msgid": msgid},
        });
        add_direct_item(path, msgid, nanotime, from, to, &data);
    }

    /// A direct item (a message, or a kept TAGMSG) from `from` to `to`, kept for both.
    pub(crate) fn add_direct_item(
        path: &Path,
        msgid: &str,
        nanotime: i64,
        from: &str,
        to: &str,
        data: &serde_json::Value,
    ) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "INSERT INTO history (data, msgid) VALUES (?1, ?2)",
                rusqlite::params![data.to_string().into_bytes(), msgid],
            )
            .unwrap();
        let id = connection.last_insert_rowid();
        for (target, correspondent) in [(from, to), (to, from)] {
            connection
                .execute(
                    "INSERT INTO conversations (target, correspondent, nanotime, history_id) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![target, correspondent, nanotime, id],
                )
                .unwrap();
        }
    }
}
