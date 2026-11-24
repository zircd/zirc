//! What the pusher keeps, in its own SQLite file beside Ergo's history ([`path_for`]), on the
//! tenant's disk: each phone's registration (its endpoint, keys and how much each of its
//! conversations notifies), the pusher's VAPID key, and how far into the history it has read.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

/// How long a write waits for another.
const BUSY: Duration = Duration::from_secs(10);

/// One phone the pusher pushes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// The account it's for, folded.
    pub account: String,
    /// The endpoint zirc.dev made for it.
    pub endpoint: String,
    /// Its public key, base64url.
    pub p256dh: String,
    /// Its auth secret, base64url.
    pub auth: String,
    /// How much each conversation notifies on it: zirc-chat's `NotifySettings`, as JSON.
    pub settings: String,
}

/// The store beside the history at `history`: `zirc_push.db` in the same directory.
#[must_use]
pub fn path_for(history: &Path) -> PathBuf {
    history.with_file_name("zirc_push.db")
}

/// The store at `path`, made if it isn't there.
///
/// # Errors
///
/// When it can't be opened or made.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(BUSY)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS subscriptions (
             endpoint TEXT PRIMARY KEY,
             account TEXT NOT NULL,
             p256dh TEXT NOT NULL,
             auth TEXT NOT NULL,
             settings TEXT NOT NULL,
             renewed INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS subscriptions_account ON subscriptions (account);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value BLOB NOT NULL);",
    )?;
    Ok(connection)
}

/// Keeps `subscription`, renewed at `now` (seconds since the Unix epoch), in place of any
/// for its endpoint: a phone signed in to another account moves its endpoint over.
///
/// # Errors
///
/// When the store can't be written.
pub fn register(
    connection: &Connection,
    subscription: &Subscription,
    now: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO subscriptions (endpoint, account, p256dh, auth, settings, renewed) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            subscription.endpoint,
            subscription.account,
            subscription.p256dh,
            subscription.auth,
            subscription.settings,
            now
        ],
    )?;
    Ok(())
}

/// Keeps `subscription` as [`register`] does, once what wasn't renewed in
/// [`zirc_proto::push::EXPIRY_DAYS`] before `now` is gone: how a phone's registration is
/// kept, whether the phone said it or zirc.dev did for it.
///
/// # Errors
///
/// When the store can't be written.
pub fn keep(
    connection: &Connection,
    subscription: &Subscription,
    now: i64,
) -> rusqlite::Result<()> {
    expire(connection, now - zirc_proto::push::EXPIRY_DAYS * 86_400)?;
    register(connection, subscription, now)
}

/// Forgets every phone of `account`, as when it's erased.
///
/// # Errors
///
/// When the store can't be written.
pub fn forget_account(connection: &Connection, account: &str) -> rusqlite::Result<usize> {
    connection.execute("DELETE FROM subscriptions WHERE account = ?1", [account])
}

/// Forgets the endpoint, but only `account`'s: no one unregisters another's phone.
///
/// # Errors
///
/// When the store can't be written.
pub fn unregister(connection: &Connection, account: &str, endpoint: &str) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM subscriptions WHERE endpoint = ?1 AND account = ?2",
        params![endpoint, account],
    )?;
    Ok(())
}

/// Forgets the endpoint, as when its push service says it's gone.
///
/// # Errors
///
/// When the store can't be written.
pub fn forget(connection: &Connection, endpoint: &str) -> rusqlite::Result<()> {
    connection.execute("DELETE FROM subscriptions WHERE endpoint = ?1", [endpoint])?;
    Ok(())
}

/// Forgets what wasn't renewed since `before`, and says how many.
///
/// # Errors
///
/// When the store can't be written.
pub fn expire(connection: &Connection, before: i64) -> rusqlite::Result<usize> {
    connection.execute("DELETE FROM subscriptions WHERE renewed < ?1", [before])
}

/// Every subscription, by account.
///
/// # Errors
///
/// When the store can't be read.
pub fn all(connection: &Connection) -> rusqlite::Result<Vec<Subscription>> {
    let mut statement = connection.prepare(
        "SELECT account, endpoint, p256dh, auth, settings FROM subscriptions ORDER BY account, endpoint",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Subscription {
            account: row.get(0)?,
            endpoint: row.get(1)?,
            p256dh: row.get(2)?,
            auth: row.get(3)?,
            settings: row.get(4)?,
        })
    })?;
    rows.collect()
}

fn meta(connection: &Connection, key: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    connection
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
}

fn set_meta(connection: &Connection, key: &str, value: &[u8]) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

/// The newest history row read, if the pusher has read any.
///
/// # Errors
///
/// When the store can't be read.
pub fn last(connection: &Connection) -> rusqlite::Result<Option<i64>> {
    Ok(meta(connection, "last")?
        .and_then(|value| String::from_utf8(value).ok())
        .and_then(|value| value.parse().ok()))
}

/// Keeps `id` as the newest history row read.
///
/// # Errors
///
/// When the store can't be written.
pub fn set_last(connection: &Connection, id: i64) -> rusqlite::Result<()> {
    set_meta(connection, "last", id.to_string().as_bytes())
}

/// The pusher's VAPID key as PKCS#8, made with `make` and kept the first time.
///
/// # Errors
///
/// When the store can't be read or written, or `make` fails.
pub fn vapid<E: From<rusqlite::Error>>(
    connection: &Connection,
    make: impl FnOnce() -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, E> {
    if let Some(kept) = meta(connection, "vapid")? {
        return Ok(kept);
    }
    // The first one kept, should two be made at once.
    let fresh = make()?;
    connection.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('vapid', ?1)",
        [&fresh],
    )?;
    Ok(meta(connection, "vapid")?.unwrap_or(fresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(account: &str, endpoint: &str) -> Subscription {
        Subscription {
            account: account.into(),
            endpoint: endpoint.into(),
            p256dh: "BP".into(),
            auth: "au".into(),
            settings: "{}".into(),
        }
    }

    #[test]
    fn keeps_renews_and_forgets_phones() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("zirc_push.db")).unwrap();
        register(&store, &phone("ada", "https://e/1"), 100).unwrap();
        register(&store, &phone("bob", "https://e/2"), 100).unwrap();
        // The same phone, signed in as bob now.
        register(&store, &phone("bob", "https://e/1"), 200).unwrap();
        assert_eq!(
            all(&store).unwrap(),
            [phone("bob", "https://e/1"), phone("bob", "https://e/2")]
        );
        unregister(&store, "ada", "https://e/2").unwrap();
        assert_eq!(all(&store).unwrap().len(), 2, "not ada's to unregister");
        assert_eq!(expire(&store, 150).unwrap(), 1);
        assert_eq!(all(&store).unwrap(), [phone("bob", "https://e/1")]);
        forget(&store, "https://e/1").unwrap();
        assert!(all(&store).unwrap().is_empty());
    }

    #[test]
    fn keeps_where_it_read_to_and_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("zirc_push.db")).unwrap();
        assert_eq!(last(&store).unwrap(), None);
        set_last(&store, 42).unwrap();
        assert_eq!(last(&store).unwrap(), Some(42));
        let made = vapid::<rusqlite::Error>(&store, || Ok(vec![1, 2])).unwrap();
        let kept = vapid::<rusqlite::Error>(&store, || Ok(vec![3])).unwrap();
        assert_eq!((made, kept), (vec![1, 2], vec![1, 2]));
    }
}
