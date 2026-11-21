//! What the sidecar's own SQLite stores share (the activity store, the search index): each is
//! made, or made again for a new format, in place and under a write lock, never by deleting
//! its files.
//!
//! Deleting them raced every other connection to the store in this process, as the server's
//! parts each open it as they start. One that opened the old file then opened the new one's
//! `-shm` as a file of its own, as SQLite tells files apart by their inode, and POSIX locks
//! never conflict within a process: it found itself alone and truncated the `-shm` under the
//! other's mapping, which died of a Bus error (zirc-server's media test, builds 4158 to 4684).

use rusqlite::{Connection, OptionalExtension};

/// The version in the store's `meta` table, `None` while it has none (a new file).
fn version(connection: &Connection) -> Option<String> {
    connection
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
            row.get(0)
        })
        .optional()
        .unwrap_or(None)
}

/// `connection`'s store, at `current`: when it's at another version, or none, `make` brings it
/// there, given the version found, inside one write transaction, so a second connection
/// opening meanwhile waits for it and finds it made.
///
/// # Errors
///
/// When the transaction or `make` fails; nothing it did is kept.
pub(crate) fn made(
    connection: Connection,
    current: &str,
    make: impl FnOnce(&Connection, Option<&str>) -> rusqlite::Result<()>,
) -> rusqlite::Result<Connection> {
    if version(&connection).as_deref() == Some(current) {
        return Ok(connection);
    }
    connection.execute_batch("BEGIN IMMEDIATE;")?;
    let done = match version(&connection) {
        Some(found) if found == current => Ok(()),
        found => make(&connection, found.as_deref()),
    };
    connection.execute_batch(if done.is_ok() { "COMMIT;" } else { "ROLLBACK;" })?;
    done.map(|()| connection)
}

/// Drops every table the store has, for one of another format to be made in its place:
/// virtual tables first, whose own tables go with them, then the rest with their indexes.
///
/// # Errors
///
/// When the schema can't be read or a table dropped.
pub(crate) fn clear(connection: &Connection) -> rusqlite::Result<()> {
    for virtual_tables in [true, false] {
        let names = connection
            .prepare(
                "SELECT name FROM sqlite_schema WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%' AND (sql LIKE 'CREATE VIRTUAL TABLE%') = ?1",
            )?
            .query_map([virtual_tables], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for name in names {
            connection.execute_batch(&format!("DROP TABLE \"{}\";", name.replace('"', "\"\"")))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    fn open(path: &std::path::Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=WAL;")
            .unwrap();
        connection
    }

    fn make_v2(connection: &Connection, _found: Option<&str>) -> rusqlite::Result<()> {
        clear(connection)?;
        connection.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE VIRTUAL TABLE grams USING fts5(text, tokenize='trigram');
             INSERT INTO meta VALUES ('version', '2');",
        )
    }

    #[test]
    fn a_store_of_another_format_is_made_again_in_its_own_file_under_an_open_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let other = open(&path);
        other
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE old (text TEXT); INSERT INTO meta VALUES ('version', '1');",
            )
            .unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();

        made(open(&path), "2", make_v2).unwrap();

        // The same file, which the connection open all along reads at its new format.
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(version(&other).as_deref(), Some("2"));
        let tables: Vec<String> = other
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name = 'old'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(tables.is_empty(), "the old format's table was kept");
    }

    #[test]
    fn connections_opening_a_new_store_at_once_make_it_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        // In WAL already: eight connections switching a new file to it at once may be told
        // it's busy without waiting, which isn't what's tested here.
        drop(open(&path));
        let makes = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    made(open(&path), "2", |connection, found| {
                        makes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        make_v2(connection, found)
                    })
                    .unwrap();
                });
            }
        });
        assert_eq!(makes.into_inner(), 1);
        assert_eq!(version(&open(&path)).as_deref(), Some("2"));
    }

    #[test]
    fn the_stores_are_made_in_the_file_a_connection_has_open() {
        let dir = tempfile::tempdir().unwrap();
        for (path, opened) in [
            (
                dir.path().join("zirc_activity.db"),
                crate::activity::open as fn(&std::path::Path) -> rusqlite::Result<Connection>,
            ),
            (dir.path().join("zirc_search.db"), crate::index::open),
        ] {
            // Another part of the server, as it starts, with the file already open.
            let other = open(&path);
            let inode = std::fs::metadata(&path).unwrap().ino();

            opened(&path).unwrap();

            assert_eq!(
                std::fs::metadata(&path).unwrap().ino(),
                inode,
                "{path:?} was made anew"
            );
            assert!(
                version(&other).is_some(),
                "{path:?} isn't made in the open file"
            );
        }
    }
}
