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

/// The rows after `last`, oldest first, at most [`BATCH`] of them, and the newest id read,
/// which is `last` when there were none. A direct message is one row for its recipient.
///
/// # Errors
///
/// When the history can't be read.
pub fn after(history: &Connection, last: i64) -> rusqlite::Result<(Vec<Kept>, i64)> {
    // A channel's message is in `sequence`; a direct message in `conversations`, once under
    // each of its two accounts.
    let mut statement = history.prepare_cached(
        "WITH batch AS (SELECT id FROM history WHERE id > ?1 ORDER BY id LIMIT ?2) \
         SELECT h.id, h.msgid, h.data, s.target, s.nanotime, c.target, c.nanotime \
         FROM batch b JOIN history h ON h.id = b.id \
         LEFT JOIN sequence s ON s.history_id = h.id \
         LEFT JOIN conversations c ON c.history_id = h.id \
         ORDER BY h.id",
    )?;
    let mut rows = statement.query(params![last, BATCH])?;
    let (mut kept, mut newest) = (Vec::new(), last);
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        newest = newest.max(id);
        let data: Vec<u8> = row.get(2)?;
        let Some(said) = exporter::said(&data) else {
            continue;
        };
        let place = match (
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(5)?,
        ) {
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
            kept.push(Kept {
                msgid: row.get(1)?,
                nanotime,
                place,
                said,
            });
        }
    }
    Ok((kept, newest))
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
    use crate::trim::testing::{add, add_direct, history};

    #[test]
    fn reads_what_came_after_with_where_it_was_said() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#api", 10, "bob", "old");
        let connection = crate::trim::open(&path).unwrap();
        let last = newest(&connection).unwrap();
        add(&path, "m2", "#api", 20, "bob", "ada: look");
        add_direct(&path, "m3", 30, "bob", "ada", "hi");
        let (kept, read_to) = after(&connection, last).unwrap();
        let places: Vec<(&str, &Place, &str)> = kept
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
        assert_eq!(read_to, 3);
        assert!(after(&connection, read_to).unwrap().0.is_empty());
        assert_eq!(by_msgid(&connection, "m1").unwrap().unwrap().nick, "bob");
        assert!(by_msgid(&connection, "nope").unwrap().is_none());
    }
}
