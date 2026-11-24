//! What the pusher reads of Ergo's history, read-only: the rows after the last it read, each
//! with where it was said, and the message a reply or reaction names, to tell whose it was.

use rusqlite::{Connection, OptionalExtension, params};

use crate::exporter::{self, Said};

/// How many rows one read takes.
const BATCH: i64 = 500;

/// Where a row was said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// In a channel.
    Channel(String),
    /// To one person directly: their account, folded.
    Direct(String),
}

/// One row of Ergo's history.
pub struct Kept {
    /// Its message id.
    pub msgid: String,
    /// When, in nanoseconds since the Unix epoch.
    pub nanotime: i64,
    /// Where.
    pub place: Place,
    /// What, and by whom.
    pub said: Said,
}

/// The newest row's id, or 0 for an empty history.
///
/// # Errors
///
/// When the history can't be read.
pub fn newest(history: &Connection) -> rusqlite::Result<i64> {
    history.query_row("SELECT coalesce(max(id), 0) FROM history", [], |row| {
        row.get(0)
    })
}

/// What one read of the history found.
pub struct Read {
    /// The rows with a place, oldest first.
    pub kept: Vec<Kept>,
    /// The newest id read past, which is `last` when there was none.
    pub newest: i64,
    /// The row the read stopped before, as Ergo is still writing it: its message is kept, where
    /// it was said not yet.
    pub unplaced: Option<i64>,
}

/// The rows after `last`, oldest first, at most [`BATCH`] of them. A direct message is one row
/// for its recipient.
///
/// Ergo keeps a message in several statements, each its own transaction (its `sqlite`
/// package's `AddChannelItem` and `AddDirectMessage`): the message, then where it was said, a
/// channel's `sequence` or a direct message's copy under its sender's account and then under
/// its recipient's. Read between them, a row has no place yet, and read past it would never be
/// pushed; so the read stops before the first such row, unless it's `pass`, which the caller
/// has waited on long enough.
///
/// # Errors
///
/// When the history can't be read.
pub fn after(history: &Connection, last: i64, pass: Option<i64>) -> rusqlite::Result<Read> {
    // A channel's message is in `sequence`; a direct message in `conversations`, once under
    // each of its two accounts.
    let mut statement = history.prepare_cached(
        "WITH batch AS (SELECT id FROM history WHERE id > ?1 ORDER BY id LIMIT ?2) \
         SELECT h.id, h.msgid, h.data, s.target, s.nanotime, c.target, c.nanotime, \
         c.correspondent \
         FROM batch b JOIN history h ON h.id = b.id \
         LEFT JOIN sequence s ON s.history_id = h.id \
         LEFT JOIN conversations c ON c.history_id = h.id \
         ORDER BY h.id",
    )?;
    let mut rows = statement.query(params![last, BATCH])?;
    let mut kept = Vec::new();
    // Each id read, in order, and whether Ergo has finished writing it.
    let mut written: Vec<(i64, bool)> = Vec::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        if written.last().is_none_or(|(seen, _)| *seen != id) {
            written.push((id, false));
        }
        let data: Vec<u8> = row.get(2)?;
        let Some(said) = exporter::said(&data) else {
            // Not a message: nothing to wait for.
            mark(&mut written);
            continue;
        };
        let channel: Option<String> = row.get(3)?;
        let copy: Option<String> = row.get(5)?;
        let correspondent: Option<String> = row.get(7)?;
        // Said in a channel, to someone else, or to oneself, which has no second copy.
        if channel.is_some()
            || copy.as_ref().is_some_and(|account| {
                !account.eq_ignore_ascii_case(&said.account)
                    || correspondent
                        .as_ref()
                        .is_some_and(|nick| nick.eq_ignore_ascii_case(&said.nick))
            })
        {
            mark(&mut written);
        }
        let place = match (channel, copy) {
            (Some(channel), _) if channel.starts_with('#') => {
                Some((Place::Channel(channel), row.get::<_, i64>(4)?))
            }
            // The copy under the account it was sent to, not the sender's own.
            (None, Some(account)) if !account.eq_ignore_ascii_case(&said.account) => {
                Some((Place::Direct(account.to_lowercase()), row.get::<_, i64>(6)?))
            }
            _ => None,
        };
        if let Some((place, nanotime)) = place {
            kept.push((
                id,
                Kept {
                    msgid: row.get(1)?,
                    nanotime,
                    place,
                    said,
                },
            ));
        }
    }
    let unplaced = written
        .iter()
        .find(|(id, done)| !done && Some(*id) != pass)
        .map(|(id, _)| *id);
    let before = |id: i64| unplaced.is_none_or(|unplaced| id < unplaced);
    Ok(Read {
        kept: kept
            .into_iter()
            .filter(|(id, _)| before(*id))
            .map(|(_, kept)| kept)
            .collect(),
        newest: written
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| before(*id))
            .fold(last, i64::max),
        unplaced,
    })
}

/// Marks the id read last as finished.
fn mark(written: &mut [(i64, bool)]) {
    if let Some((_, done)) = written.last_mut() {
        *done = true;
    }
}

/// The message with the id `msgid`, if the history has it.
///
/// # Errors
///
/// When the history can't be read.
pub fn by_msgid(history: &Connection, msgid: &str) -> rusqlite::Result<Option<Said>> {
    let data: Option<Vec<u8>> = history
        .query_row(
            "SELECT data FROM history WHERE msgid = ?1 LIMIT 1",
            [msgid],
            |row| row.get(0),
        )
        .optional()?;
    Ok(data.and_then(|data| exporter::said(&data)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trim::testing::{add, add_copy, add_direct, add_message, history};

    #[test]
    fn reads_what_came_after_with_where_it_was_said() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#api", 10, "bob", "old");
        let connection = crate::trim::open(&path).unwrap();
        let last = newest(&connection).unwrap();
        add(&path, "m2", "#api", 20, "bob", "ada: look");
        add_direct(&path, "m3", 30, "bob", "ada", "hi");
        let read = after(&connection, last, None).unwrap();
        let places: Vec<(&str, &Place, &str)> = read
            .kept
            .iter()
            .map(|k| (k.msgid.as_str(), &k.place, k.said.text.as_str()))
            .collect();
        assert_eq!(
            places,
            [
                ("m2", &Place::Channel("#api".into()), "ada: look"),
                ("m3", &Place::Direct("ada".into()), "hi"),
            ]
        );
        assert_eq!((read.newest, read.unplaced), (3, None));
        assert!(
            after(&connection, read.newest, None)
                .unwrap()
                .kept
                .is_empty()
        );
        assert_eq!(by_msgid(&connection, "m1").unwrap().unwrap().nick, "bob");
        assert!(by_msgid(&connection, "nope").unwrap().is_none());
    }

    /// Read between Ergo's statements for a direct message, the read stops before it rather
    /// than read past it with nowhere to push it, until its recipient's copy is kept.
    #[test]
    fn stops_before_a_message_ergo_is_still_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let connection = crate::trim::open(&path).unwrap();
        let lunch = add_message(&path, "m1", "bob", "lunch?");
        let read = after(&connection, 0, None).unwrap();
        assert!(read.kept.is_empty());
        assert_eq!((read.newest, read.unplaced), (0, Some(lunch)));
        add_copy(&path, lunch, "bob", "ada");
        add(&path, "m2", "#api", 20, "bob", "after");
        let read = after(&connection, 0, None).unwrap();
        assert!(read.kept.is_empty(), "the sender's copy alone");
        assert_eq!((read.newest, read.unplaced), (0, Some(lunch)));
        add_copy(&path, lunch, "ada", "bob");
        let read = after(&connection, 0, None).unwrap();
        let msgids: Vec<&str> = read.kept.iter().map(|k| k.msgid.as_str()).collect();
        assert_eq!(msgids, ["m1", "m2"]);
        assert_eq!((read.newest, read.unplaced), (2, None));

        // One to oneself has one copy; one never finished is read past once waited on.
        let own = add_message(&path, "m3", "bob", "note to self");
        add_copy(&path, own, "bob", "bob");
        let lost = add_message(&path, "m4", "bob", "never placed");
        let read = after(&connection, 2, None).unwrap();
        assert_eq!((read.newest, read.unplaced), (own, Some(lost)));
        let read = after(&connection, own, Some(lost)).unwrap();
        assert_eq!((read.newest, read.unplaced), (lost, None));
    }
}
