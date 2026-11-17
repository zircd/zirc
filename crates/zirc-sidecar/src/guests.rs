//! An organisation's guests (`crate::gateway`), for zirc.dev's Guests list: who they are, and
//! taking one out of its channels or forgetting them.
//!
//! What's known of a guest is in Ergo's history: what they said in its public channels (under
//! the channel, with their nick), and their conversations with its staff (under each staff
//! member's account, with the guest as the correspondent, and under the guest's own account
//! once they have one). Forgetting a guest deletes all of it, through Ergo, which serves
//! CHATHISTORY from it, so nobody gets it back; `crate::trim` then scrubs it off the disk.
//! Fourteen days after a guest was last there, the sidecar forgets them by itself ([`KEPT`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use rusqlite::OptionalExtension;
use serde::Serialize;

/// How long after a guest was last there they're forgotten.
pub const KEPT: Duration = Duration::from_hours(14 * 24);

/// How often the sidecar looks for guests to forget.
pub const SWEEP: Duration = Duration::from_hours(1);

/// A guest as the Guests list shows them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guest {
    /// The guest's name.
    pub name: String,
    /// Whether they're connected now.
    pub connected: bool,
    /// When they last said something, or were connected while the sidecar watched (RFC 3339).
    pub last_seen: Option<String>,
    /// The public channels they've spoken in, or are in now.
    pub channels: BTreeSet<String>,
}

/// Whether `name` is a guest's.
#[must_use]
pub fn is_guest(name: &str) -> bool {
    name.strip_prefix(zirc_proto::GUEST_PREFIX)
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Every guest in `history`, with what it says of them: when each last spoke, and where.
///
/// # Errors
///
/// When the history can't be read.
pub fn in_history(history: &Path) -> rusqlite::Result<BTreeMap<String, (i64, BTreeSet<String>)>> {
    let mut guests: BTreeMap<String, (i64, BTreeSet<String>)> = BTreeMap::new();
    if !history.exists() {
        return Ok(guests);
    }
    let db = crate::trim::open(history)?;
    // What they said in channels: the item's nick is `nick!user@host`.
    let mut said = db.prepare(
        "SELECT lower(substr(json_extract(h.data, '$.Nick'), 1, \
                instr(json_extract(h.data, '$.Nick'), '!') - 1)) AS nick, s.target, max(s.nanotime) \
         FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.target LIKE '#%' AND json_extract(h.data, '$.Nick') LIKE 'guest-%' \
         GROUP BY nick, s.target",
    )?;
    let rows = said.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (nick, channel, at) = row?;
        if is_guest(&nick) {
            let entry = guests.entry(nick).or_default();
            entry.0 = entry.0.max(at);
            entry.1.insert(channel);
        }
    }
    // Their private lines; and for a guest staff wrote to who never answered, the
    // conversation's newest, so they're kept as long as it's going on.
    let mut talked = db.prepare(
        "SELECT CASE WHEN c.target LIKE 'guest-%' THEN c.target ELSE c.correspondent END, \
                max(CASE WHEN json_extract(h.data, '$.Nick') LIKE 'guest-%' THEN c.nanotime ELSE 0 END), \
                max(c.nanotime) \
         FROM conversations c JOIN history h ON h.id = c.history_id \
         WHERE c.target LIKE 'guest-%' OR c.correspondent LIKE 'guest-%' GROUP BY 1",
    )?;
    let rows = talked.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (name, own, newest) = row?;
        if is_guest(&name) {
            let entry = guests.entry(name).or_default();
            let said = entry.0.max(own);
            entry.0 = if said > 0 { said } else { newest };
        }
    }
    Ok(guests)
}

/// Every message `guest` is party to, as `(target, msgid)`: what they said in channels,
/// everything said in their rooms of their own ([`zirc_proto::guest_room`]), and both sides of
/// their conversations.
///
/// # Errors
///
/// When the history can't be read.
pub fn messages(history: &Path, guest: &str) -> rusqlite::Result<Vec<(String, String)>> {
    if !history.exists() {
        return Ok(vec![]);
    }
    let db = crate::trim::open(history)?;
    let mut found = BTreeMap::new();
    let mut said = db.prepare(
        "SELECT s.target, h.msgid FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.target LIKE '#%' AND (lower(json_extract(h.data, '$.Nick')) LIKE ?1 || '!%' \
                OR s.target LIKE '#%/' || ?1)",
    )?;
    for row in said.query_map([guest], |r| {
        Ok((r.get::<_, String>(1)?, r.get::<_, String>(0)?))
    })? {
        let (msgid, target) = row?;
        found.insert(msgid, target);
    }
    let mut talked = db.prepare(
        "SELECT h.msgid FROM conversations c JOIN history h ON h.id = c.history_id \
         WHERE c.target = ?1 OR c.correspondent = ?1",
    )?;
    for row in talked.query_map([guest], |r| r.get::<_, String>(0))? {
        found.entry(row?).or_insert_with(|| guest.to_owned());
    }
    Ok(found
        .into_iter()
        .map(|(msgid, target)| (target, msgid))
        .collect())
}

/// Removes who `guest` talked with (Ergo's `correspondents`, for `CHATHISTORY TARGETS`), once
/// the messages have gone.
///
/// # Errors
///
/// When the history can't be written.
pub fn drop_correspondents(history: &Path, guest: &str) -> rusqlite::Result<usize> {
    if !history.exists() {
        return Ok(0);
    }
    // Written, beside Ergo, as `crate::trim` scrubs: SQLite takes turns.
    let db = rusqlite::Connection::open(history)?;
    db.busy_timeout(Duration::from_secs(10))?;
    let present: Option<String> = db
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'correspondents'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if present.is_none() {
        return Ok(0);
    }
    db.execute(
        "DELETE FROM correspondents WHERE target = ?1 OR correspondent = ?1",
        [guest],
    )
}

/// The most unread lines a guest's reply email carries.
pub const UNREAD_LINES: usize = 20;
/// The most characters of each of those lines.
pub const UNREAD_CHARS: usize = 2000;

/// A line staff wrote to a guest who hasn't seen it, for zirc.dev's reply email.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unread {
    /// The line's message id.
    pub msgid: String,
    /// RFC 3339.
    pub time: String,
    /// The staff member's account.
    pub from: String,
    /// What they wrote, cut at [`UNREAD_CHARS`]; left out when the organisation keeps
    /// message text out of reply emails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// What people on staff wrote to `guest` after `since` (nanoseconds) and that isn't in
/// `mailed`, oldest first, at most [`UNREAD_LINES`]: from Ergo's history under the guest's
/// own account, so only a guest with one has any. Never what staff write each other, nor what
/// was removed, which Ergo's history no longer has.
///
/// # Errors
///
/// When the history can't be read.
pub fn unread(
    history: &Path,
    guest: &str,
    since: i64,
    mailed: &BTreeSet<String>,
    with_text: bool,
) -> rusqlite::Result<Vec<Unread>> {
    if !history.exists() {
        return Ok(vec![]);
    }
    let db = crate::trim::open(history)?;
    let mut rows = db.prepare(
        "SELECT c.nanotime, h.msgid, h.data FROM conversations c JOIN history h ON h.id = c.history_id \
         WHERE c.target = ?1 AND c.nanotime > ?2 ORDER BY c.nanotime, h.id",
    )?;
    let mut lines = Vec::new();
    let found = rows.query_map(rusqlite::params![guest, since], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get_ref(2)?.as_bytes()?.to_vec(),
        ))
    })?;
    for row in found {
        let (at, msgid, data) = row?;
        let Some(said) = crate::exporter::said(&data) else {
            continue;
        };
        let from = said.account.to_lowercase();
        if said.kind != crate::exporter::PRIVMSG
            || !crate::gateway::person(&from)
            || mailed.contains(&msgid)
        {
            continue;
        }
        let text = match zirc_proto::decode(&said.text).ok().flatten() {
            Some(message) => message.event.human_text(),
            None => said.text,
        };
        lines.push(Unread {
            msgid,
            time: Timestamp::from_nanosecond(i128::from(at))
                .map(|t| t.to_string())
                .unwrap_or_default(),
            from,
            text: with_text.then(|| text.chars().take(UNREAD_CHARS).collect()),
        });
        if lines.len() >= UNREAD_LINES {
            break;
        }
    }
    Ok(lines)
}

/// The lines already mailed to guests, by msgid, with when they were sent: kept beside
/// Ergo's history, for as long as a guest is.
pub struct Mailed {
    path: std::path::PathBuf,
    sent: BTreeMap<String, i64>,
}

impl Mailed {
    /// What was mailed, as noted beside Ergo's history, if anything.
    #[must_use]
    pub fn beside(history: &Path) -> Self {
        let path = history.with_file_name("zirc-mailed.json");
        let sent = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, sent }
    }

    /// The ids of the lines mailed.
    #[must_use]
    pub fn ids(&self) -> BTreeSet<String> {
        self.sent.keys().cloned().collect()
    }

    /// Notes `msgids` mailed at `now` (seconds since the epoch), forgets those older than a
    /// guest is kept, and saves.
    ///
    /// # Errors
    ///
    /// When it can't be saved.
    pub fn note(
        &mut self,
        msgids: impl IntoIterator<Item = String>,
        now: i64,
    ) -> std::io::Result<()> {
        for msgid in msgids {
            self.sent.insert(msgid, now);
        }
        let kept = i64::try_from(KEPT.as_secs()).unwrap_or(i64::MAX);
        self.sent.retain(|_, at| *at > now - kept);
        let bytes = serde_json::to_vec(&self.sent).map_err(std::io::Error::other)?;
        let partial = self.path.with_extension("json.partial");
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &self.path)
    }
}

/// The guests to forget at `now`: last there [`KEPT`] or longer ago, and not on now.
#[must_use]
pub fn due(guests: &[Guest], now: Timestamp) -> Vec<String> {
    guests
        .iter()
        .filter(|g| !g.connected)
        .filter(|g| {
            g.last_seen
                .as_deref()
                .and_then(|t| t.parse::<Timestamp>().ok())
                .is_none_or(|seen| now.duration_since(seen).unsigned_abs() >= KEPT)
        })
        .map(|g| g.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trim::testing::{add, add_direct, history};

    const SECOND: i64 = 1_000_000_000;

    #[test]
    fn a_guest_name_is_one_zirc_dev_gives() {
        assert!(is_guest("guest-7k2q"));
        assert!(!is_guest("guest-"));
        assert!(!is_guest("guest-7k2q-mbp"));
        assert!(!is_guest("mattias"));
        assert!(!is_guest("mattias-mbp"));
    }

    #[test]
    fn the_history_says_who_the_guests_are_and_what_they_re_party_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(
            &path,
            "c1",
            "#support",
            10 * SECOND,
            "guest-7k2q",
            "a question",
        );
        add(&path, "c2", "#support", 20 * SECOND, "mattias", "an answer");
        // Their room of their own: all of it is theirs, staff's lines too; another's isn't.
        add(
            &path,
            "r1",
            "#support/guest-7k2q",
            25 * SECOND,
            "mattias",
            "in your room",
        );
        add(
            &path,
            "r2",
            "#support/guest-9x8w",
            26 * SECOND,
            "mattias",
            "in theirs",
        );
        add_direct(
            &path,
            "d1",
            30 * SECOND,
            "mattias",
            "guest-7k2q",
            "privately",
        );
        add_direct(&path, "d2", 40 * SECOND, "guest-7k2q", "mattias", "thanks");
        add_direct(&path, "d3", 50 * SECOND, "mattias", "bob", "not theirs");
        // Written to, never answered.
        add_direct(&path, "d4", 60 * SECOND, "bob", "guest-9x8w", "hello?");
        let guests = in_history(&path).unwrap();
        assert_eq!(
            guests.keys().collect::<Vec<_>>(),
            ["guest-7k2q", "guest-9x8w"]
        );
        assert_eq!(guests["guest-7k2q"].0, 40 * SECOND);
        assert_eq!(guests["guest-9x8w"].0, 60 * SECOND);
        assert_eq!(
            guests["guest-7k2q"].1,
            BTreeSet::from(["#support".to_owned()])
        );
        let theirs: BTreeSet<String> = messages(&path, "guest-7k2q")
            .unwrap()
            .into_iter()
            .map(|(_, msgid)| msgid)
            .collect();
        assert_eq!(
            theirs,
            BTreeSet::from(["c1", "d1", "d2", "r1"].map(String::from))
        );
    }

    #[test]
    fn a_guest_s_unread_lines_are_staff_s_since_they_were_last_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add_direct(
            &path,
            "d1",
            10 * SECOND,
            "mattias",
            "guest-7k2q",
            "seen already",
        );
        add_direct(
            &path,
            "d2",
            30 * SECOND,
            "mattias",
            "guest-7k2q",
            "are you there?",
        );
        add_direct(
            &path,
            "d3",
            40 * SECOND,
            "mattias-mbp",
            "guest-7k2q",
            "an agent's",
        );
        add_direct(
            &path,
            "d4",
            50 * SECOND,
            "guest-7k2q",
            "mattias",
            "the guest's own",
        );
        add_direct(
            &path,
            "d5",
            60 * SECOND,
            "bob",
            "guest-7k2q",
            &"long ".repeat(500),
        );
        add_direct(&path, "d6", 70 * SECOND, "mattias", "bob", "between staff");
        let ids = |lines: Vec<Unread>| lines.into_iter().map(|l| l.msgid).collect::<Vec<_>>();
        let since = 20 * SECOND;
        let lines = unread(&path, "guest-7k2q", since, &BTreeSet::new(), true).unwrap();
        assert_eq!(lines[0].from, "mattias");
        assert_eq!(lines[0].text.as_deref(), Some("are you there?"));
        assert_eq!(
            lines[1].text.as_ref().unwrap().chars().count(),
            UNREAD_CHARS
        );
        assert_eq!(ids(lines), ["d2", "d5"]);
        // Mailed once, never again; and without text when the organisation says so.
        let mailed = BTreeSet::from(["d2".to_owned()]);
        let lines = unread(&path, "guest-7k2q", since, &mailed, false).unwrap();
        assert_eq!(lines[0].text, None);
        assert_eq!(ids(lines), ["d5"]);
    }

    #[test]
    fn guests_are_forgotten_fourteen_days_after_they_were_last_there() {
        let now: Timestamp = "2026-11-20T00:00:00Z".parse().unwrap();
        let guest = |name: &str, seen: Option<&str>, connected: bool| Guest {
            name: name.into(),
            connected,
            last_seen: seen.map(Into::into),
            channels: BTreeSet::new(),
        };
        let guests = [
            guest("guest-old", Some("2026-11-05T00:00:00Z"), false),
            guest("guest-new", Some("2026-11-10T00:00:00Z"), false),
            guest("guest-on", Some("2026-11-01T00:00:00Z"), true),
            guest("guest-unknown", None, false),
        ];
        assert_eq!(due(&guests, now), ["guest-old", "guest-unknown"]);
    }
}
