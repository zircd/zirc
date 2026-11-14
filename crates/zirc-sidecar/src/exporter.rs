//! Prometheus metrics for one tenant's Ergo, from a sidecar in its pod: who's connected, from
//! Ergo's HTTP API on localhost, and, from the history Ergo keeps in SQLite on the disk they
//! share, how many messages its channels get and which zirc machines and sessions are active.
//!
//! Ergo's API has neither, so they come from its history database, read-only. Its schema is
//! Ergo's own (version 2 as of v2.19); on any other version the exporter leaves them out
//! rather than guess. Direct messages are never read.
//!
//! These go to Google (Managed Prometheus), so they're counts and nothing more: no channel's
//! name, nor anything else about a conversation, is ever a label. Per-channel figures are the
//! account endpoint's to give, from the pod (`GET /v1/channels`, [`channels`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::get;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use tracing::warn;

use crate::ergo::Api;

/// The content type of `OpenMetrics` text, which the sidecar and the controller both serve.
pub const CONTENT: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// The history schema the message counts are written for.
pub(crate) const HISTORY_VERSION: &str = "2";

/// Ergo's history item types for a PRIVMSG and a NOTICE.
pub(crate) const MESSAGE_TYPES: &str = "1, 2";
pub(crate) const PRIVMSG: i64 = 1;

/// A kept TAGMSG's item type, as Ergo numbers them.
pub(crate) const TAGMSG: i64 = 8;

/// A zirc machine or session is active if it said something in a channel this recently.
pub const ACTIVE: Duration = Duration::from_hours(1);

#[derive(Clone, Debug, clap::Args)]
#[group(id = "metrics")]
/// The sidecar's options for the metrics endpoint and Ergo's API, flattened beside the others.
pub struct Options {
    /// Where to serve `/metrics`.
    #[arg(long, default_value = "0.0.0.0:9100")]
    pub listen: std::net::SocketAddr,

    /// Ergo's API, on localhost.
    #[arg(long, default_value = "http://127.0.0.1:8089")]
    pub ergo_api: String,

    /// The API's bearer token.
    #[arg(long, env = "ERGO_API_TOKEN", hide_env_values = true)]
    pub token: String,

    /// Ergo's history database.
    #[arg(long, default_value = "/ircd/ergo_history.db")]
    pub history: PathBuf,
}

impl Options {
    /// Ergo's API, as these options reach it.
    #[must_use]
    pub fn api(&self) -> Api {
        Api::new(self.ergo_api.clone(), self.token.clone())
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct Kind {
    kind: String,
}

struct Exporter {
    api: Api,
    history: Mutex<History>,
    up: Gauge,
    users: Family<Kind, Gauge>,
    max_users: Gauge,
    channels: Gauge,
    channel_messages: Counter,
    sessions: Gauge,
    machines: Gauge,
    registry: Registry,
}

/// Serves `/metrics` until the process ends.
///
/// # Errors
///
/// When it can't listen.
pub async fn run(options: Options) -> std::io::Result<()> {
    let listen = options.listen;
    let exporter = Arc::new(Exporter::new(&options));
    let app = Router::new().route(
        "/metrics",
        get(move || {
            let exporter = exporter.clone();
            async move { ([(CONTENT_TYPE, CONTENT)], exporter.scrape().await).into_response() }
        }),
    );
    axum::serve(tokio::net::TcpListener::bind(listen).await?, app).await
}

impl Exporter {
    fn new(options: &Options) -> Self {
        let mut root = Registry::default();
        let (sessions, machines) = (Gauge::default(), Gauge::default());
        root.register(
            "zirc_sessions_active",
            "zirc sessions (agents or people) that sent a zirc message to a channel in the last hour",
            sessions.clone(),
        );
        root.register(
            "zirc_machines_active",
            "zirc machines (zircd connections) that sent a zirc message to a channel in the last hour",
            machines.clone(),
        );
        let registry = root.sub_registry_with_prefix("ergo");
        let (up, users, max_users, channels) = (
            Gauge::default(),
            Family::default(),
            Gauge::default(),
            Gauge::default(),
        );
        let channel_messages = Counter::default();
        registry.register("up", "Whether Ergo's API answered", up.clone());
        registry.register(
            "users",
            "Connected users, by kind (total, invisible, operators, unknown)",
            users.clone(),
        );
        registry.register(
            "users_max",
            "The most users connected at once since Ergo started",
            max_users.clone(),
        );
        registry.register("channels", "Channels that exist", channels.clone());
        registry.register(
            "channel_messages",
            "Messages (PRIVMSG and NOTICE) sent to channels, all of them together, since the exporter started",
            channel_messages.clone(),
        );
        Self {
            history: Mutex::new(History::new(options.history.clone())),
            api: options.api(),
            up,
            users,
            max_users,
            channels,
            channel_messages,
            sessions,
            machines,
            registry: root,
        }
    }

    async fn scrape(&self) -> String {
        match self
            .api
            .call::<Status>("status", serde_json::json!({}))
            .await
        {
            Ok(status) => {
                self.up.set(1);
                for (kind, count) in [
                    ("total", status.users.total),
                    ("invisible", status.users.invisible),
                    ("operators", status.users.operators),
                    ("unknown", status.users.unknown),
                ] {
                    self.users
                        .get_or_create(&Kind { kind: kind.into() })
                        .set(count);
                }
                self.max_users.set(status.users.max);
                self.channels.set(status.channels);
            }
            Err(error) => {
                warn!(%error, "Ergo's API didn't answer");
                self.up.set(0);
            }
        }

        let (counts, active) = {
            let mut history = self.history.lock().unwrap();
            let counts = history.new_messages();
            (counts, history.active(now()))
        };
        self.sessions
            .set(i64::try_from(active.sessions).unwrap_or(i64::MAX));
        self.machines
            .set(i64::try_from(active.machines).unwrap_or(i64::MAX));
        // All channels' together: which channel said what never leaves the pod.
        self.channel_messages.inc_by(counts.values().sum());

        let mut text = String::new();
        encode(&mut text, &self.registry).expect("writing to a String succeeds");
        text
    }
}

#[derive(Deserialize)]
struct Status {
    users: Users,
    channels: i64,
}

#[derive(Deserialize)]
struct Users {
    total: i64,
    invisible: i64,
    operators: i64,
    unknown: i64,
    max: i64,
}

#[derive(Deserialize)]
struct List {
    channels: Vec<ListedChannel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedChannel {
    name: String,
    user_count: i64,
}

/// A channel as its members' dashboard shows it (`GET /v1/channels`): served from the pod to
/// zirc.dev, and never a metric.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelFigures {
    /// The channel's name.
    pub name: String,
    /// How many are in it now.
    pub users: i64,
    /// How many messages it had in the last hour.
    pub messages_last_hour: u64,
}

/// The server's channels: their users from Ergo's API, and the messages (PRIVMSG and NOTICE)
/// each got in the last hour from its history; busiest first.
///
/// # Errors
///
/// When Ergo's API doesn't answer.
pub async fn channels(api: &Api, history: PathBuf) -> Result<Vec<ChannelFigures>, reqwest::Error> {
    let list: List = api.call("list", serde_json::json!({})).await?;
    let since = now().saturating_sub(i64::try_from(ACTIVE.as_nanos()).unwrap_or(i64::MAX));
    let sent = tokio::task::spawn_blocking(move || messages_since(&history, since))
        .await
        .ok()
        .and_then(|read| {
            read.inspect_err(|error| warn!(%error, "couldn't read Ergo's history"))
                .ok()
        })
        .unwrap_or_default();
    Ok(figures(list.channels, &sent))
}

fn figures(listed: Vec<ListedChannel>, sent: &BTreeMap<String, u64>) -> Vec<ChannelFigures> {
    let mut channels: Vec<_> = listed
        .into_iter()
        .map(|channel| {
            let name = channel.name.to_lowercase();
            ChannelFigures {
                messages_last_hour: sent.get(&name).copied().unwrap_or(0),
                users: channel.user_count,
                name,
            }
        })
        .collect();
    channels.sort_by(|a, b| b.users.cmp(&a.users).then_with(|| a.name.cmp(&b.name)));
    channels
}

/// Ergo's history, as [`open_history`] finds it.
pub(crate) enum Opened {
    /// Open, read-only, with the schema this knows.
    Open(Connection),
    /// Not there yet.
    Missing,
    /// There, with a schema this doesn't know: its `db.version`.
    Unknown(Option<String>),
}

/// Ergo's history at `path`, read-only, when its schema is [`HISTORY_VERSION`]: what every
/// reader of it here opens it with.
pub(crate) fn open_history(path: &std::path::Path) -> rusqlite::Result<Opened> {
    if !path.exists() {
        return Ok(Opened::Missing);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let version: Option<String> = connection
        .query_row(
            "SELECT value FROM metadata WHERE key_name = 'db.version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(if version.as_deref() == Some(HISTORY_VERSION) {
        Opened::Open(connection)
    } else {
        Opened::Unknown(version)
    })
}

/// Messages (PRIVMSG and NOTICE) to each channel at or after `since` (nanoseconds since the
/// epoch), from Ergo's history read-only; none where there's no history or its schema isn't
/// one this knows. Never direct messages.
fn messages_since(path: &std::path::Path, since: i64) -> rusqlite::Result<BTreeMap<String, u64>> {
    let Opened::Open(connection) = open_history(path)? else {
        return Ok(BTreeMap::new());
    };
    let mut statement = connection.prepare(&format!(
        "SELECT lower(s.target), count(*) FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.nanotime >= ?1 AND s.target LIKE '#%' \
         AND json_extract(h.data, '$.Type') IN ({MESSAGE_TYPES}) GROUP BY lower(s.target)"
    ))?;
    statement
        .query_map([since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .map(|row| row.map(|(channel, n)| (channel, u64::try_from(n).unwrap_or(0))))
        .collect()
}

/// Now, in Ergo's history's nanoseconds since the epoch.
fn now() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since.as_nanos()).unwrap_or(i64::MAX)
}

/// The zirc machines and sessions active lately.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Active {
    /// Sessions seen lately.
    pub sessions: usize,
    /// Machines seen lately.
    pub machines: usize,
}

/// Ergo's history, read from where the last read left off.
pub struct History {
    path: PathBuf,
    connection: Option<Connection>,
    /// The newest history item already counted.
    cursor: i64,
    /// The schema isn't one the exporter knows.
    unsupported: bool,
    /// When each zirc session, by its machine's nick, last sent a zirc message to a channel.
    senders: HashMap<(String, String), i64>,
}

impl History {
    /// Reads the history database at `path`, from its start.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            connection: None,
            cursor: 0,
            unsupported: false,
            senders: HashMap::new(),
        }
    }

    /// Messages to each channel since the last call. The first call only finds where the
    /// history ends, so counting starts with the exporter; it does read the last hour back,
    /// for who's active.
    pub fn new_messages(&mut self) -> BTreeMap<String, u64> {
        match self.read() {
            Ok(counts) => counts,
            Err(error) => {
                warn!(%error, path = %self.path.display(), "couldn't read Ergo's history");
                self.connection = None;
                BTreeMap::new()
            }
        }
    }

    /// The zirc sessions and machines that sent a zirc message to a channel in the hour
    /// before `now` (nanoseconds since the epoch), as of the last read.
    pub fn active(&mut self, now: i64) -> Active {
        let since = now.saturating_sub(i64::try_from(ACTIVE.as_nanos()).unwrap_or(i64::MAX));
        self.senders.retain(|_, last| *last >= since);
        Active {
            sessions: self.senders.len(),
            machines: self
                .senders
                .keys()
                .map(|(nick, _)| nick)
                .collect::<HashSet<_>>()
                .len(),
        }
    }

    fn read(&mut self) -> rusqlite::Result<BTreeMap<String, u64>> {
        if self.unsupported {
            return Ok(BTreeMap::new());
        }
        let first = self.connection.is_none();
        if first {
            let connection = match open_history(&self.path)? {
                Opened::Open(connection) => connection,
                Opened::Missing => return Ok(BTreeMap::new()),
                Opened::Unknown(version) => {
                    warn!(
                        ?version,
                        "Ergo's history has a schema this exporter doesn't know; not reading it"
                    );
                    self.unsupported = true;
                    return Ok(BTreeMap::new());
                }
            };
            self.connection = Some(connection);
        }
        let connection = self.connection.as_ref().expect("opened above");
        // Up to where the history ends now, so what Ergo writes meanwhile waits for next time.
        let end: i64 =
            connection.query_row("SELECT coalesce(max(id), 0) FROM history", [], |row| {
                row.get(0)
            })?;
        let since = now().saturating_sub(i64::try_from(ACTIVE.as_nanos()).unwrap_or(i64::MAX));
        // The first read starts an hour back, and only notes who was active.
        let (from, newer_than) = if first {
            (0, since)
        } else {
            (self.cursor, i64::MIN)
        };
        let mut statement = connection.prepare_cached(&format!(
            "SELECT s.target, s.nanotime, h.data FROM sequence s JOIN history h ON h.id = s.history_id \
             WHERE s.history_id > ?1 AND s.history_id <= ?2 AND s.nanotime >= ?3 \
             AND s.target LIKE '#%' AND json_extract(h.data, '$.Type') IN ({MESSAGE_TYPES})"
        ))?;
        let mut counts = BTreeMap::<String, u64>::new();
        let mut rows = statement.query([from, end, newer_than])?;
        while let Some(row) = rows.next()? {
            let (target, nanotime): (String, i64) = (row.get(0)?, row.get(1)?);
            // Each item is JSON, which Ergo stores as a BLOB.
            let data = row.get_ref(2)?.as_bytes()?.to_vec();
            if !first {
                *counts.entry(target).or_default() += 1;
            }
            if let Some(sender) = zirc_sender(&data) {
                let last = self.senders.entry(sender).or_insert(nanotime);
                *last = (*last).max(nanotime);
            }
        }
        self.cursor = end;
        Ok(counts)
    }
}

/// A message in Ergo's history: its type, who sent it and what it says, a multiline one's
/// lines joined.
pub(crate) struct Said {
    pub kind: i64,
    /// The sender's nick, without the rest of its mask.
    pub nick: String,
    /// The sender's account, empty or `*` without one.
    pub account: String,
    pub text: String,
    /// Its message tags, as a TAGMSG's are kept (Ergo's tagmsg-storage whitelist).
    pub tags: std::collections::BTreeMap<String, String>,
}

/// A history item's message, or `None` when it isn't one this reads.
pub(crate) fn said(data: &[u8]) -> Option<Said> {
    #[derive(Deserialize)]
    struct Item {
        #[serde(rename = "Type")]
        kind: i64,
        #[serde(rename = "Nick")]
        nick: String,
        #[serde(rename = "AccountName", default)]
        account: String,
        #[serde(rename = "Message")]
        message: Body,
        #[serde(rename = "Tags", default)]
        tags: Option<std::collections::BTreeMap<String, String>>,
    }
    // Ergo's SplitMessage: a multiline message (draft/multiline) has its lines in `Split`, and
    // `Message` empty; a line that `Concat`s carries on from the one before without a break.
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "Message")]
        text: String,
        #[serde(rename = "Split", default)]
        lines: Option<Vec<Line>>,
    }
    #[derive(Deserialize)]
    struct Line {
        #[serde(rename = "Message")]
        text: String,
        #[serde(rename = "Concat", default)]
        concat: bool,
    }
    let item: Item = serde_json::from_slice(data).ok()?;
    let text = match item.message.lines {
        Some(lines) if !lines.is_empty() => {
            let mut text = String::new();
            for (i, line) in lines.iter().enumerate() {
                if i > 0 && !line.concat {
                    text.push('\n');
                }
                text.push_str(&line.text);
            }
            text
        }
        _ => item.message.text,
    };
    Some(Said {
        kind: item.kind,
        nick: item.nick.split('!').next().unwrap_or_default().to_owned(),
        account: item.account,
        text,
        tags: item.tags.unwrap_or_default(),
    })
}

/// The machine's nick and the session a history item came from, if it's a zirc message.
fn zirc_sender(data: &[u8]) -> Option<(String, String)> {
    let said = said(data)?;
    if said.kind != PRIVMSG {
        return None;
    }
    let message = zirc_proto::decode(&said.text).ok()??;
    Some((said.nick, message.session))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multiline_zirc_message_has_its_sender() {
        let event = zirc_proto::Event::Intent {
            summary: "the controller".into(),
            paths: vec![],
            expires: None,
            lasts: None,
        };
        let body = zirc_proto::encode(&zirc_proto::Message::new("cc-1531f8", event)).unwrap();
        // As Ergo stores a draft/multiline batch: the text in Split, cut anywhere and
        // concatenated back, and Message empty.
        let (a, b) = body.split_at(body.len() / 2);
        let data = serde_json::json!({
            "Type": PRIVMSG,
            "Nick": "mattias-mbp!~u@host",
            "Message": {"Message": "", "Msgid": "x", "Time": "2026-10-31T00:00:00Z",
                        "Split": [{"Message": a, "Concat": false}, {"Message": b, "Concat": true}]},
        });
        assert_eq!(
            zirc_sender(data.to_string().as_bytes()),
            Some(("mattias-mbp".into(), "cc-1531f8".into()))
        );
    }

    /// A history database as Ergo v2.19 makes it, with a few items in it.
    fn database(path: &std::path::Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE metadata (key_name TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO metadata VALUES ('db.version', '2'), ('db.minorversion', '2');
                 CREATE TABLE history (id INTEGER PRIMARY KEY AUTOINCREMENT, data BLOB NOT NULL, msgid TEXT NOT NULL);
                 CREATE TABLE sequence (history_id INTEGER NOT NULL PRIMARY KEY, target TEXT NOT NULL, nanotime INTEGER NOT NULL);",
            )
            .unwrap();
        connection
    }

    fn add(connection: &Connection, target: &str, kind: u8) {
        add_from(connection, target, kind, "someone", "hello", now());
    }

    /// An item as Ergo writes it, from `nick` with `text`, at `nanotime`.
    fn add_from(
        connection: &Connection,
        target: &str,
        kind: u8,
        nick: &str,
        text: &str,
        nanotime: i64,
    ) {
        let data = serde_json::json!({
            "Type": kind,
            "Nick": format!("{nick}!~u@host"),
            "AccountName": "*",
            "Message": {"Message": text, "Msgid": "x", "Split": null, "Time": "2026-10-31T00:00:00Z"},
        });
        connection
            .execute(
                "INSERT INTO history (data, msgid) VALUES (?1, 'x')",
                // As Ergo stores it: a BLOB.
                [data.to_string().into_bytes()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sequence VALUES (last_insert_rowid(), ?1, ?2)",
                rusqlite::params![target, nanotime],
            )
            .unwrap();
    }

    #[test]
    fn counts_new_channel_messages_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ergo_history.db");
        let mut history = History::new(path.clone());
        assert!(history.new_messages().is_empty(), "no database yet");

        let ergo = database(&path);
        add(&ergo, "#team", 1);
        assert!(
            history.new_messages().is_empty(),
            "what was there before isn't counted"
        );

        add(&ergo, "#team", 1); // PRIVMSG
        add(&ergo, "#team", 2); // NOTICE
        add(&ergo, "#team", 3); // JOIN
        add(&ergo, "#ops", 1);
        add(&ergo, "alice", 1); // a direct message
        assert_eq!(
            history.new_messages(),
            BTreeMap::from([("#ops".to_owned(), 1), ("#team".to_owned(), 2)])
        );
        assert!(
            history.new_messages().is_empty(),
            "nothing is counted twice"
        );
    }

    #[test]
    fn leaves_an_unknown_schema_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ergo_history.db");
        let ergo = database(&path);
        ergo.execute(
            "UPDATE metadata SET value = '3' WHERE key_name = 'db.version'",
            [],
        )
        .unwrap();
        let mut history = History::new(path);
        history.new_messages();
        add(&ergo, "#team", 1);
        assert!(history.new_messages().is_empty());
    }

    fn zirc(session: &str) -> String {
        let event = zirc_proto::Event::Intent {
            summary: "the controller".into(),
            paths: vec![],
            expires: None,
            lasts: None,
        };
        zirc_proto::encode(&zirc_proto::Message::new(session, event)).unwrap()
    }

    #[test]
    fn counts_active_zirc_machines_and_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ergo_history.db");
        let ergo = database(&path);
        let hour = i64::try_from(ACTIVE.as_nanos()).unwrap();
        let earlier = now() - 2 * hour;
        // Before the exporter started: an hour old or more doesn't count.
        add_from(&ergo, "#team", 1, "laptop", &zirc("cc-old"), earlier);
        add_from(
            &ergo,
            "#team",
            1,
            "laptop",
            &zirc("cc-recent"),
            now() - hour / 2,
        );
        let mut history = History::new(path);
        history.new_messages();
        assert_eq!(
            history.active(now()),
            Active {
                sessions: 1,
                machines: 1
            }
        );

        add_from(&ergo, "#team", 1, "laptop", &zirc("cc-other"), now());
        add_from(&ergo, "#ops", 1, "desktop", &zirc("cc-recent"), now());
        add_from(
            &ergo,
            "#team",
            1,
            "person",
            "plain words aren't zirc's",
            now(),
        );
        add_from(&ergo, "#team", 2, "laptop", &zirc("cc-notice"), now()); // a NOTICE
        add_from(&ergo, "alice", 1, "laptop", &zirc("cc-dm"), now()); // a direct message
        let counts = history.new_messages();
        assert_eq!(counts["#team"], 3);
        assert_eq!(
            history.active(now()),
            Active {
                sessions: 3,
                machines: 2
            }
        );

        // An hour on, they've all gone quiet.
        assert_eq!(history.active(now() + 2 * hour), Active::default());
    }

    #[test]
    fn channels_have_their_users_and_last_hours_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ergo_history.db");
        let ergo = database(&path);
        let hour = i64::try_from(ACTIVE.as_nanos()).unwrap();
        add(&ergo, "#team", 1);
        add(&ergo, "#team", 2); // a NOTICE counts
        add(&ergo, "#team", 3); // a JOIN doesn't
        add_from(&ergo, "#team", 1, "someone", "old", now() - 2 * hour);
        add(&ergo, "#ops", 1);
        add(&ergo, "alice", 1); // a direct message is never read
        let sent = messages_since(&path, now() - hour).unwrap();
        assert_eq!(
            sent,
            BTreeMap::from([("#ops".to_owned(), 1), ("#team".to_owned(), 2)])
        );
        let listed = |name: &str, user_count| ListedChannel {
            name: name.into(),
            user_count,
        };
        assert_eq!(
            figures(
                vec![listed("#Ops", 3), listed("#team", 3), listed("#quiet", 5)],
                &sent
            ),
            vec![
                ChannelFigures {
                    name: "#quiet".into(),
                    users: 5,
                    messages_last_hour: 0
                },
                ChannelFigures {
                    name: "#ops".into(),
                    users: 3,
                    messages_last_hour: 1
                },
                ChannelFigures {
                    name: "#team".into(),
                    users: 3,
                    messages_last_hour: 2
                },
            ]
        );
        assert!(
            messages_since(&dir.path().join("none.db"), 0)
                .unwrap()
                .is_empty()
        );
    }

    /// What the exporter gives Google says nothing about any conversation: no label names a
    /// channel, and no channel's name appears anywhere in it.
    #[tokio::test]
    async fn no_metric_names_a_channel() {
        use axum::routing::post;
        let fake = Router::new()
            .route(
                "/v1/status",
                post(|| async {
                    axum::Json(serde_json::json!({
                        "users": {"total": 3, "invisible": 0, "operators": 1, "unknown": 0, "max": 4},
                        "channels": 2,
                    }))
                }),
            )
            .route(
                "/v1/list",
                post(|| async {
                    axum::Json(serde_json::json!({
                        "channels": [{"name": "#secret-plans", "userCount": 2}, {"name": "#team", "userCount": 3}],
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, fake).await });

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ergo_history.db");
        let ergo = database(&path);
        let exporter = Exporter::new(&Options {
            listen: "127.0.0.1:0".parse().unwrap(),
            ergo_api: format!("http://{address}"),
            token: "t".into(),
            history: path,
        });
        exporter.scrape().await;
        add(&ergo, "#secret-plans", 1);
        add(&ergo, "#team", 1);
        add(&ergo, "#team", 2);
        let text = exporter.scrape().await;

        assert!(text.contains("ergo_up 1"), "{text}");
        assert!(text.contains("ergo_channels 2"), "{text}");
        assert!(text.contains("ergo_channel_messages_total 3"), "{text}");
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            assert!(!line.contains("channel="), "a channel label: {line}");
            assert!(
                !line.contains("secret-plans") && !line.contains("team"),
                "a channel's name: {line}"
            );
        }
        // The exporter's own words aside (# HELP, # TYPE), its text names no channel either.
        assert!(
            !text.contains("#secret-plans") && !text.contains("#team"),
            "{text}"
        );
    }
}
