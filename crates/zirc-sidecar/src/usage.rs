//! What a tenant's sidecar counts of its server's use, for zirc's product analytics: how many
//! people and agents it has, how many were active in the last day and week, how many are
//! connected now, and how many channel messages each kind sent in the last day. Counts only:
//! never an account, a nick, a channel or a message.
//!
//! The sidecar counts, at `GET /v1/usage` on its account endpoint, which only the controller
//! may call, and which sends the counts on.
//!
//! People and agents are told apart by the account a message was sent from, as Ergo stored
//! it, and that account's kind as zirc.dev knows it: a person's handle can't have a `-` (web's
//! `HANDLE`, and a constraint on its `memberships` table), and every machine's account is
//! `<handle>-<machine>`, so an account with a `-` is a machine, whose messages are its agents'.
//!
//! A message is what a person would call one: a channel `PRIVMSG` of plain text, or a zirc
//! post (zirc's `text` event: taking, done, a question). zircd's automatic events (who's
//! working on what, commits, pushes, intents, custom events) and `NOTICE`s aren't counted,
//! nor are direct messages. Who's active is who sent such a message, from Ergo's history, or
//! had a client connected, which the sidecar notes every few minutes in `zirc-seen.json`
//! beside the history, on the tenant's own disk.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::exporter::{self, PRIVMSG};

/// People and agents, each counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Split {
    /// People.
    pub people: u64,
    /// Agents.
    pub agents: u64,
}

impl Split {
    fn add(&mut self, account: &str, n: u64) {
        if is_agent(account) {
            self.agents += n;
        } else {
            self.people += n;
        }
    }

    fn of<'a>(accounts: impl IntoIterator<Item = &'a String>) -> Self {
        let mut split = Self::default();
        accounts.into_iter().for_each(|a| split.add(a, 1));
        split
    }
}

/// What the sidecar counts of its server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Its accounts.
    pub accounts: Split,
    /// Accounts that sent a channel message or had a client connected, in the last day.
    pub active_day: Split,
    /// The same in the last week.
    pub active_week: Split,
    /// Accounts with a client connected now.
    pub connected: Split,
    /// The clients connected now, an account's desktop and phone each counting.
    pub sessions: Split,
    /// Channel messages sent in the last day.
    pub messages_day: Split,
    /// The first channel message its history has, if any. Older sidecars don't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<First>,
}

/// When the first message was sent (nanoseconds since the epoch), and whether an agent sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct First {
    /// When it was sent, in nanoseconds since the Unix epoch.
    pub at: i64,
    /// Whether an agent sent it.
    pub by_agent: bool,
}

/// An agent's machine rather than a person ([`crate::machines::is_agent`]).
pub(crate) fn is_agent(account: &str) -> bool {
    crate::machines::is_agent(account)
}

const DAY: Duration = Duration::from_hours(24);
const WEEK: Duration = Duration::from_hours(24 * 7);

/// How long the sidecar remembers when an account was last connected: as long as a person's
/// account stays always-on unused (`shape::ALWAYS_ON_EXPIRATION`), for the Machines page.
const REMEMBERED: Duration = Duration::from_hours(24 * 90);

/// How often the sidecar notes who's connected.
pub const SAMPLE: Duration = Duration::from_mins(5);

/// When each account last had a client connected, kept on the tenant's disk so a restart
/// doesn't forget the week.
pub struct Seen {
    path: PathBuf,
    last: BTreeMap<String, i64>,
}

impl Seen {
    /// What was noted beside Ergo's history, if anything.
    #[must_use]
    pub fn beside(history: &Path) -> Self {
        let path = history.with_file_name("zirc-seen.json");
        let last = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, last }
    }

    /// Notes the accounts connected at `now` (seconds since the epoch), forgets those not
    /// seen for [`REMEMBERED`], and saves.
    pub fn note<'a>(&mut self, connected: impl IntoIterator<Item = &'a String>, now: i64) {
        for account in connected {
            self.last.insert(account.clone(), now);
        }
        let remembered = i64::try_from(REMEMBERED.as_secs()).unwrap_or(i64::MAX);
        self.last.retain(|_, seen| *seen > now - remembered);
        let saved = serde_json::to_vec(&self.last)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let partial = self.path.with_extension("json.partial");
                std::fs::write(&partial, bytes)?;
                std::fs::rename(&partial, &self.path)
            });
        if let Err(error) = saved {
            warn!(%error, path = %self.path.display(), "couldn't save who was connected");
        }
    }

    /// When each account was last seen connected (seconds since the epoch).
    pub(crate) fn last(&self) -> BTreeMap<String, i64> {
        self.last.clone()
    }

    /// Accounts seen connected since `since` (seconds since the epoch).
    pub(crate) fn since(&self, since: i64) -> impl Iterator<Item = &String> {
        self.last
            .iter()
            .filter(move |(_, seen)| **seen >= since)
            .map(|(account, _)| account)
    }
}

/// Who sent channel messages, from Ergo's history.
#[derive(Default)]
struct Sent {
    week: BTreeSet<String>,
    day: BTreeSet<String>,
    messages_day: Split,
}

/// Whether a history item's message is one a person would call a message, and whose account
/// sent it: plain text, or a zirc post.
fn message(data: &[u8]) -> Option<String> {
    let said = exporter::said(data)?;
    if said.kind != PRIVMSG {
        return None;
    }
    match zirc_proto::decode(&said.text) {
        Ok(None) => {}
        Ok(Some(message)) if matches!(message.event, zirc_proto::Event::Text { .. }) => {}
        _ => return None,
    }
    // Someone without an account, in through the gate, isn't counted.
    let account = said.account.to_lowercase();
    (!account.is_empty() && account != "*").then_some(account)
}

/// From Ergo's history, the accounts that sent a channel message since `week` and since
/// `day`, and how many messages each kind sent since `day` (nanoseconds since the epoch).
/// Direct messages aren't read.
fn sent(history: &Path, week: i64, day: i64) -> rusqlite::Result<Sent> {
    let mut sent = Sent::default();
    if !history.exists() {
        return Ok(sent);
    }
    let connection = crate::trim::open(history)?;
    let mut statement = connection.prepare(&format!(
        "SELECT s.nanotime, h.data FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.nanotime >= ?1 AND s.target LIKE '#%' \
         AND json_extract(h.data, '$.Type') = {PRIVMSG}"
    ))?;
    let mut rows = statement.query([week])?;
    while let Some(row) = rows.next()? {
        let nanotime: i64 = row.get(0)?;
        let Some(account) = message(row.get_ref(1)?.as_bytes()?) else {
            continue;
        };
        if nanotime >= day {
            sent.messages_day.add(&account, 1);
            sent.day.insert(account.clone());
        }
        sent.week.insert(account);
    }
    Ok(sent)
}

/// The first message in Ergo's history, as [`message`] counts one.
fn first(history: &Path) -> rusqlite::Result<Option<First>> {
    if !history.exists() {
        return Ok(None);
    }
    let connection = crate::trim::open(history)?;
    let mut statement = connection.prepare(&format!(
        "SELECT s.nanotime, h.data FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.target LIKE '#%' AND json_extract(h.data, '$.Type') = {PRIVMSG} \
         ORDER BY s.nanotime"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        if let Some(account) = message(row.get_ref(1)?.as_bytes()?) {
            return Ok(Some(First {
                at: row.get(0)?,
                by_agent: is_agent(&account),
            }));
        }
    }
    Ok(None)
}

/// The server's usage at `now`: from who's connected (`sessions`, each account with its
/// connected clients), who was seen connected before, and the history.
#[must_use]
pub fn usage(sessions: &[(String, i64)], seen: &Seen, history: &Path, now: Timestamp) -> Usage {
    let ago = |span: Duration| now.checked_sub(span).unwrap_or(Timestamp::UNIX_EPOCH);
    let (week, day) = (ago(WEEK), ago(DAY));
    let nanos = |t: Timestamp| i64::try_from(t.as_nanosecond()).unwrap_or(i64::MAX);
    let sent = sent(history, nanos(week), nanos(day)).unwrap_or_else(|error| {
        warn!(%error, "couldn't read Ergo's history for usage");
        Sent::default()
    });
    let accounts: BTreeSet<String> = sessions.iter().map(|(a, _)| a.clone()).collect();
    let connected: BTreeSet<String> = sessions
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(a, _)| a.clone())
        .collect();
    let active = |sent: &BTreeSet<String>, since: Timestamp| {
        let active: BTreeSet<&String> = sent
            .iter()
            .chain(seen.since(since.as_second()))
            .chain(&connected)
            // Only accounts that are still there.
            .filter(|a| accounts.contains(*a))
            .collect();
        Split::of(active)
    };
    let mut clients = Split::default();
    for (account, n) in sessions {
        clients.add(account, u64::try_from(*n).unwrap_or(0));
    }
    Usage {
        accounts: Split::of(&accounts),
        active_day: active(&sent.day, day),
        active_week: active(&sent.week, week),
        connected: Split::of(&connected),
        sessions: clients,
        messages_day: sent.messages_day,
        first_message: first(history).unwrap_or_else(|error| {
            warn!(%error, "couldn't read Ergo's history for the first message");
            None
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_SECONDS: i64 = 24 * 3600;

    #[test]
    fn people_and_agents_are_told_apart_by_what_the_server_was_told() {
        crate::machines::the_tests_machines();
        let accounts = ["mattias", "mattias-mbp", "ada", "ada-ci"].map(String::from);
        assert_eq!(
            Split::of(&accounts),
            Split {
                people: 2,
                agents: 2
            }
        );
    }

    #[test]
    fn seen_remembers_ninety_days_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("ergo_history.db");
        let mut seen = Seen::beside(&history);
        seen.note(&["old".to_owned()], 0);
        seen.note(&["month".to_owned()], 60 * DAY_SECONDS);
        seen.note(&["ada".to_owned()], 90 * DAY_SECONDS);
        // Read back from the disk: `old` was last seen 90 days before, and is forgotten.
        let seen = Seen::beside(&history);
        assert_eq!(seen.since(0).collect::<Vec<_>>(), ["ada", "month"]);
        assert_eq!(seen.last()["month"], 60 * DAY_SECONDS);
    }

    #[test]
    fn usage_counts_connections_and_who_was_seen() {
        crate::machines::the_tests_machines();
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("ergo_history.db");
        let now = Timestamp::from_second(30 * DAY_SECONDS).unwrap();
        let mut seen = Seen::beside(&history);
        // Three days ago: active in the week, not the day.
        seen.note(&["bob".to_owned()], now.as_second() - 3 * DAY_SECONDS);
        let sessions = [
            ("ada".to_owned(), 2),
            ("ada-ci".to_owned(), 1),
            ("bob".to_owned(), 0),
            ("bob-mbp".to_owned(), 0),
        ];
        let split = |people, agents| Split { people, agents };
        assert_eq!(
            usage(&sessions, &seen, &history, now),
            Usage {
                accounts: split(2, 2),
                active_day: split(1, 1),
                active_week: split(2, 1),
                connected: split(1, 1),
                sessions: split(2, 1),
                messages_day: Split::default(),
                first_message: None,
            }
        );
    }

    #[test]
    fn messages_are_what_people_and_agents_say_not_zircd_s_events() {
        use crate::trim::testing::{add, add_item, history};
        crate::machines::the_tests_machines();
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let now = Timestamp::from_second(30 * DAY_SECONDS).unwrap();
        let at = |seconds_ago: i64| (now.as_second() - seconds_ago) * 1_000_000_000;
        let zirc =
            |event| zirc_proto::encode(&zirc_proto::Message::new("cc-1531f8", event)).unwrap();
        let post = zirc(zirc_proto::Event::Text {
            body: "taking: the usage counts".into(),
        });
        let working = zirc(zirc_proto::Event::Working {
            paths: vec!["src/usage.rs".into()],
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        });
        // A person, from the desktop app: a plain message, and a multiline one.
        add(&path, "m1", "#team", at(60), "alice", "morning");
        add_item(
            &path,
            "m2",
            "#team",
            at(50),
            &serde_json::json!({
                "Type": 1, "Nick": "alice!u@h", "AccountName": "alice",
                "Message": {"Message": "", "Split": [{"Message": "one"}, {"Message": "two"}]},
            }),
        );
        // An agent on alice's machine: a post counts, zircd's events don't.
        add(&path, "m3", "#team", at(40), "alice-mbp", &post);
        add(&path, "m4", "#team", at(30), "alice-mbp", &working);
        add(&path, "m5", "#team", at(20), "alice-mbp", &working);
        // Not messages: a NOTICE, a direct message, and someone without an account.
        add_item(
            &path,
            "m6",
            "#team",
            at(10),
            &serde_json::json!({
                "Type": 2, "Nick": "bot!u@h", "AccountName": "bot",
                "Message": {"Message": "the build passed"},
            }),
        );
        add(&path, "m7", "alice-mbp", at(10), "alice", "thanks");
        add(&path, "m8", "#team", at(10), "*", "without an account");
        // Three days ago: active in the week only.
        add(&path, "m9", "#team", at(3 * DAY_SECONDS), "bob", "hello");

        let sessions: Vec<(String, i64)> = ["alice", "alice-mbp", "bob", "bot"]
            .iter()
            .map(|a| ((*a).to_owned(), 0))
            .collect();
        let usage = usage(&sessions, &Seen::beside(&path), &path, now);
        let split = |people, agents| Split { people, agents };
        assert_eq!(usage.messages_day, split(2, 1));
        assert_eq!(usage.active_day, split(1, 1));
        assert_eq!(usage.active_week, split(2, 1));
        // The first message: bob's, three days ago, not zircd's events or one without an account.
        assert_eq!(
            usage.first_message,
            Some(First {
                at: at(3 * DAY_SECONDS),
                by_agent: false
            })
        );
    }
}
