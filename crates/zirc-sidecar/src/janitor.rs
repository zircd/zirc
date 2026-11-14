//! The janitor of unused threads: which of a tenant's channels it may clean, and when.
//!
//! The rules are the channels session's spec, which the user agreed to on 5 Nov ("let's go
//! ahead with your recommendations", then "hmm, no just inform, no cancel button", and that
//! a thread registered by the server's own account is nobody's choice). A channel is cleaned
//! only when every one of these holds, and anything uncertain keeps it ([`keep`]):
//!
//! 1. It's a thread, `#<parent>/<name>`, and not a standing room ([`zirc_proto::is_thread`]).
//! 2. It isn't registered with `ChanServ`, or its founder is the server's own account
//!    ([`crate::teams::FOUNDER`]); any other founder keeps it.
//! 3. Nothing readable was ever said in it: a message or notice, but those of the services
//!    (`HistServ`'s playback, `ChanServ`, `NickServ`) and sessions' title cards
//!    ([`zirc_proto::worth_reading`]).
//! 4. Its history is whole: its oldest row is newer than the retention less a day, so none
//!    of what was said in it can have expired.
//! 5. It's been quiet a day: its newest row, of any kind, is over [`QUIET`] old.
//! 6. Nobody is in it: no member connected and not away.
//!
//! It runs at most once an hour. A run that finds channels to clean says which in `#ops` and
//! when, [`NOTICE`] ahead, and the run after that time cleans those still to be cleaned then
//! ([`Janitor::step`]); at most [`CAP`] a run. It does nothing, and says why, when more than
//! [`MOST`] are candidates or the history looks wrong ([`Refusal`]). It keeps nothing on the
//! server: the history is its evidence and the channel list its state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use zirc_irc::Line;

use crate::ergo::{Api, Operator};
use crate::exporter::MESSAGE_TYPES;
use crate::teams::{self, FOUNDER, Session};

/// The most channels one run cleans: a bug can't empty a server.
pub const CAP: usize = 25;

/// More candidates than this, and the janitor suspects itself, and does nothing.
pub const MOST: usize = 200;

/// How long a thread has been quiet, at least, before it's cleaned.
pub const QUIET: Duration = Duration::from_hours(24);

/// How long ahead the janitor says what it will clean.
pub const NOTICE: Duration = Duration::from_mins(60);

/// History whose newest row is older than this looks stale: Ergo may not be writing it.
pub const STALE: Duration = Duration::from_hours(7 * 24);

/// The services, whose lines in a channel's history aren't anyone saying anything.
const SERVICES: &[&str] = &["histserv", "chanserv", "nickserv"];

/// What the history says of one channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rows {
    /// Its oldest and newest rows of any kind, in nanoseconds since the Unix epoch.
    pub oldest: i64,
    /// Its newest row of any kind, in nanoseconds since the Unix epoch.
    pub newest: i64,
    /// Whether anything readable was said in it ([`read`]'s rule 3). Kept for threads only.
    pub readable: bool,
}

/// What the history says of every channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// Each channel's rows, by its lowercase name.
    pub channels: BTreeMap<String, Rows>,
    /// The newest row of any channel.
    pub newest: Option<i64>,
}

/// A history item, as far as the janitor reads one.
#[derive(Deserialize)]
struct Item {
    #[serde(rename = "Nick", default)]
    nick: String,
    #[serde(rename = "Message", default)]
    message: Said,
}

#[derive(Deserialize, Default)]
struct Said {
    #[serde(rename = "Message", default)]
    message: String,
}

/// Every channel's rows in the history at `path`.
///
/// # Errors
///
/// When the history can't be read, or has a schema this doesn't know.
pub fn read(path: &Path) -> rusqlite::Result<History> {
    let connection = crate::trim::open(path)?;
    let mut history = History::default();
    let mut statement = connection.prepare(
        "SELECT target, min(nanotime), max(nanotime) FROM sequence \
         WHERE target LIKE '#%' GROUP BY target",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (channel, oldest, newest) = row?;
        history.newest = Some(history.newest.map_or(newest, |n| n.max(newest)));
        history.channels.insert(
            channel.to_lowercase(),
            Rows {
                oldest,
                newest,
                readable: false,
            },
        );
    }
    // Rule 3, for threads alone: their messages and notices, each read as people would.
    let mut statement = connection.prepare(&format!(
        "SELECT s.target, h.data FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.target LIKE '#%/%' AND json_extract(h.data, '$.Type') IN ({MESSAGE_TYPES})"
    ))?;
    let messages = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get_ref(1)?.as_bytes()?.to_vec(),
        ))
    })?;
    for message in messages {
        let (channel, data) = message?;
        let Some(rows) = history.channels.get_mut(&channel.to_lowercase()) else {
            continue;
        };
        if !rows.readable {
            rows.readable = readable(&data);
        }
    }
    Ok(history)
}

/// Whether a stored message is one a person would read. One that can't be read counts.
fn readable(data: &[u8]) -> bool {
    let Ok(item) = serde_json::from_slice::<Item>(data) else {
        return true;
    };
    let nick = item
        .nick
        .split('!')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    !SERVICES.contains(&nick.as_str()) && zirc_proto::worth_reading(&item.message.message)
}

/// What Ergo says of a channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Its `ChanServ` founder, or `None` when it isn't registered.
    pub founder: Option<String>,
    /// Whether a member is connected and not away.
    pub present: bool,
}

/// What Ergo says of each thread on the server ([`Facts`]), as its founder and operator; the
/// other channels aren't asked about, as rule 1 keeps them anyway. A thread Ergo couldn't say
/// is left out, and so kept ([`Keep::Unknown`]).
///
/// # Errors
///
/// When it can't list the channels, reach Ergo or log in.
pub async fn facts(api: &Api, operator: &Operator) -> Result<BTreeMap<String, Facts>, String> {
    let threads: Vec<String> = teams::channels(api)
        .await?
        .into_iter()
        .filter(|name| zirc_proto::is_thread(name))
        .collect();
    let mut facts = BTreeMap::new();
    if threads.is_empty() {
        return Ok(facts);
    }
    let (client, mut incoming, _) = teams::log_in(api, operator).await?;
    let mut session = Session {
        client,
        incoming: &mut incoming,
    };
    for name in threads {
        if let Ok(founder) = founder(&mut session, &name).await
            && let Ok(members) = members(&mut session, &name).await
        {
            let present = members.iter().any(|(_, here)| *here);
            facts.insert(name, Facts { founder, present });
        }
    }
    let _ = session.client.quit("done");
    Ok(facts)
}

/// Cleans each of `channels`, threads [`candidates`] chose, one at a time: unregisters it,
/// should the server's own account have registered it, takes out whoever's left in it (away,
/// as rule 6 has it), and checks it's gone from the server's list. Each channel's outcome,
/// `None` when it's gone. It checks again what it can as it goes, and leaves a channel be
/// that's no longer a thread, is registered to someone, or has someone in it.
///
/// # Errors
///
/// When it can't reach Ergo or log in.
pub async fn clean(
    api: &Api,
    operator: &Operator,
    channels: &[String],
) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut outcome = BTreeMap::new();
    if channels.is_empty() {
        return Ok(outcome);
    }
    let (client, mut incoming, _) = teams::log_in(api, operator).await?;
    let mut session = Session {
        client,
        incoming: &mut incoming,
    };
    for name in channels {
        let result = clean_one(&mut session, name).await;
        outcome.insert(name.clone(), result.err());
    }
    let _ = session.client.quit("done");
    let left = teams::channels(api).await?;
    for (name, error) in &mut outcome {
        if error.is_none() && left.contains(name) {
            *error = Some("still on the server".into());
        }
    }
    Ok(outcome)
}

async fn clean_one(session: &mut Session<'_>, name: &str) -> Result<(), String> {
    if !zirc_proto::is_thread(name) || name.contains([' ', ',', '\x07']) {
        return Err("not a thread".into());
    }
    let founder = founder(session, name).await?;
    if founder
        .as_deref()
        .is_some_and(|founder| !founder.eq_ignore_ascii_case(FOUNDER))
    {
        return Err(format!("registered to {}", founder.unwrap_or_default()));
    }
    let members = members(session, name).await?;
    if members.iter().any(|(_, here)| *here) {
        return Err("someone's in it".into());
    }
    // In it, with ops, to take out those who are away, and so that unregistering doesn't
    // leave it behind empty; leaving it then ends it.
    let inside = !members.is_empty();
    if inside {
        session.send(Line::new("JOIN", [name]))?;
        session.joined(name).await?;
        session.send(Line::new("SAMODE", [name, "+o", FOUNDER]))?;
        for (nick, _) in &members {
            session.send(Line::new("KICK", [name, nick.as_str(), "an unused thread"]))?;
        }
    }
    if founder.is_some() {
        unregister(session, name).await?;
    }
    if inside {
        session.send(Line::new("PART", [name]))?;
    }
    Ok(())
}

/// A channel's `ChanServ` founder, or `None` when it isn't registered.
async fn founder(session: &mut Session<'_>, name: &str) -> Result<Option<String>, String> {
    let info = session.chanserv(&format!("INFO {name}")).await?;
    if info.contains("is not registered") {
        return Ok(None);
    }
    if !info.contains("is registered") {
        return Err(format!("ChanServ: {info}"));
    }
    // Then its founder, and when it was registered.
    let founder = session.chanserv_notice().await?;
    session.chanserv_notice().await?;
    founder
        .strip_prefix("Founder: ")
        .map(|founder| Some(founder.trim().to_owned()))
        .ok_or(format!("ChanServ: {founder}"))
}

/// A channel's members but the founder, each with whether they're here (not away).
async fn members(session: &mut Session<'_>, name: &str) -> Result<Vec<(String, bool)>, String> {
    // WHOX: each member's nick and flags, `H` for here or `G` for gone (away).
    session.send(Line::new("WHO", [name, "%nf"]))?;
    let mut members = Vec::new();
    loop {
        let line = session
            .until(|l| matches!(l.command.as_str(), "354" | "315"), |_| false)
            .await?;
        if line.command == "315" {
            return Ok(members);
        }
        if let [_, nick, flags] = line.params.as_slice()
            && !nick.eq_ignore_ascii_case(FOUNDER)
        {
            members.push((nick.clone(), !flags.starts_with('G')));
        }
    }
}

/// Unregisters a channel the founder holds: `ChanServ` asks for the code it gives, to be sure
/// (`To confirm, run this command: /CS UNREGISTER <channel> <code>`), then says it's done.
async fn unregister(session: &mut Session<'_>, name: &str) -> Result<(), String> {
    session.send(Line::new(
        "PRIVMSG",
        ["ChanServ", &format!("UNREGISTER {name}")],
    ))?;
    let mut code = None;
    for _ in 0..2 {
        if let Some(found) = confirmation(&plain(&session.chanserv_notice().await?), name) {
            code = Some(found);
            break;
        }
    }
    let code = code.ok_or("ChanServ gave no code to unregister it")?;
    let reply = plain(
        &session
            .chanserv(&format!("UNREGISTER {name} {code}"))
            .await?,
    );
    if reply.contains("is now unregistered") {
        Ok(())
    } else {
        Err(format!("ChanServ: {reply}"))
    }
}

/// The code in `ChanServ`'s ask to confirm unregistering `channel`, as it gave it.
fn confirmation(notice: &str, channel: &str) -> Option<String> {
    let at = notice
        .to_lowercase()
        .find(&format!("unregister {} ", channel.to_lowercase()))?;
    let rest = notice.get(at + "unregister ".len() + channel.len() + 1..)?;
    rest.split_whitespace().next().map(ToOwned::to_owned)
}

/// Text without IRC's formatting codes (bold, colours and the like).
fn plain(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Why a channel is kept: the first rule it fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Keep {
    /// Not a thread, or a standing room (rule 1).
    NotThread,
    /// Registered to someone (rule 2).
    Registered,
    /// Someone's in it (rule 6).
    Present,
    /// What Ergo or the history says of it isn't known.
    Unknown,
    /// Something readable was said in it (rule 3).
    Readable,
    /// Its history may have expired in part (rule 4).
    Trimmed,
    /// Made, or used, within [`QUIET`] (rule 5).
    Recent,
}

impl Keep {
    /// The rule's words, for the line a run says.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::NotThread => "not a thread",
            Self::Registered => "registered",
            Self::Present => "someone in it",
            Self::Unknown => "not known",
            Self::Readable => "used",
            Self::Trimmed => "older than its history",
            Self::Recent => "recent",
        }
    }
}

/// Why `channel` is kept, or `None` when it may be cleaned: its `facts` from Ergo and `rows`
/// from the history, at `now` (nanoseconds since the Unix epoch), with the history kept for
/// `retention`.
#[must_use]
pub fn keep(
    channel: &str,
    facts: Option<&Facts>,
    rows: Option<&Rows>,
    now: i64,
    retention: Duration,
) -> Option<Keep> {
    if !zirc_proto::is_thread(channel) {
        return Some(Keep::NotThread);
    }
    let Some(facts) = facts else {
        return Some(Keep::Unknown);
    };
    if facts
        .founder
        .as_deref()
        .is_some_and(|founder| !founder.eq_ignore_ascii_case(crate::teams::FOUNDER))
    {
        return Some(Keep::Registered);
    }
    if facts.present {
        return Some(Keep::Present);
    }
    // A channel with no history at all has no age to go by.
    let Some(rows) = rows else {
        return Some(Keep::Unknown);
    };
    if rows.readable {
        return Some(Keep::Readable);
    }
    let whole = nanos(retention.saturating_sub(Duration::from_hours(24)));
    if rows.oldest < now.saturating_sub(whole) {
        return Some(Keep::Trimmed);
    }
    if rows.newest > now.saturating_sub(nanos(QUIET)) {
        return Some(Keep::Recent);
    }
    None
}

fn nanos(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

/// Why a run does nothing at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// More candidates than [`MOST`]: a rule may be wrong.
    TooMany(usize),
    /// The history has no channel rows.
    EmptyHistory,
    /// The history's newest row is older than [`STALE`].
    StaleHistory,
    /// The history couldn't be read.
    Unreadable(String),
    /// Ergo didn't list its channels, or say what they are.
    NoList(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooMany(n) => write!(
                f,
                "{n} threads look unused, more than {MOST}: a rule may be wrong"
            ),
            Self::EmptyHistory => write!(f, "the history has no channels in it"),
            Self::StaleHistory => write!(f, "the history hasn't been written for a week"),
            Self::Unreadable(why) => write!(f, "the history couldn't be read: {why}"),
            Self::NoList(why) => write!(f, "Ergo didn't say its channels: {why}"),
        }
    }
}

/// Whether `history` looks right to judge by, at `now`.
#[must_use]
pub fn sound(history: &History, now: i64) -> Option<Refusal> {
    match history.newest {
        None => Some(Refusal::EmptyHistory),
        Some(newest) if newest < now.saturating_sub(nanos(STALE)) => Some(Refusal::StaleHistory),
        Some(_) => None,
    }
}

/// The channels that may be cleaned, sorted, and how many each rule kept.
#[must_use]
pub fn candidates(
    facts: &BTreeMap<String, Facts>,
    history: &History,
    now: i64,
    retention: Duration,
) -> (Vec<String>, BTreeMap<Keep, usize>) {
    let mut clean = Vec::new();
    let mut kept = BTreeMap::new();
    for (channel, facts) in facts {
        let channel = channel.to_lowercase();
        match keep(
            &channel,
            Some(facts),
            history.channels.get(&channel),
            now,
            retention,
        ) {
            None => clean.push(channel),
            Some(why) => *kept.entry(why).or_insert(0) += 1,
        }
    }
    clean.sort();
    (clean, kept)
}

/// What a run does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to say or do.
    Nothing,
    /// Say in `#ops` that these will be cleaned at `at` (nanoseconds since the Unix epoch);
    /// `more` is how many beyond [`CAP`] wait for a later run.
    Announce {
        /// The channels, at most [`CAP`].
        channels: Vec<String>,
        /// When they will be cleaned.
        at: i64,
        /// How many more wait for a later run.
        more: usize,
    },
    /// Clean these, announced a [`NOTICE`] ago and candidates still; `dropped` were
    /// announced and are kept, no longer candidates.
    Clean {
        /// The channels to clean.
        channels: Vec<String>,
        /// Those announced and kept after all.
        dropped: Vec<String>,
    },
    /// Do nothing, and say why.
    Refuse(Refusal),
}

/// The janitor between runs: what it said it would clean, and when. In memory alone: a
/// restarted sidecar says it again before it cleans anything.
#[derive(Debug, Default)]
pub struct Janitor {
    announced: Option<(i64, BTreeSet<String>)>,
}

impl Janitor {
    /// The run at `now` with these `candidates`, or a refusal: announces, then cleans those
    /// still candidates once [`NOTICE`] has passed.
    pub fn step(&mut self, candidates: Result<Vec<String>, Refusal>, now: i64) -> Step {
        let candidates = match candidates {
            Ok(candidates) if candidates.len() > MOST => Err(Refusal::TooMany(candidates.len())),
            other => other,
        };
        let candidates = match candidates {
            Ok(candidates) => candidates,
            Err(refusal) => {
                self.announced = None;
                return Step::Refuse(refusal);
            }
        };
        match self.announced.take() {
            Some((at, announced)) if now >= at => {
                let still: BTreeSet<&String> = candidates.iter().collect();
                let (channels, dropped): (Vec<String>, Vec<String>) =
                    announced.into_iter().partition(|c| still.contains(c));
                if channels.is_empty() && dropped.is_empty() {
                    return Step::Nothing;
                }
                Step::Clean { channels, dropped }
            }
            Some(waiting) => {
                self.announced = Some(waiting);
                Step::Nothing
            }
            None if candidates.is_empty() => Step::Nothing,
            None => {
                let at = now.saturating_add(nanos(NOTICE));
                let more = candidates.len().saturating_sub(CAP);
                let channels: Vec<String> = candidates.into_iter().take(CAP).collect();
                self.announced = Some((at, channels.iter().cloned().collect()));
                Step::Announce { channels, at, more }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trim::testing::{add, add_item, history};

    const HOUR: i64 = 3_600_000_000_000;
    const DAY: i64 = 24 * HOUR;
    const NOW: i64 = 100 * DAY;
    const YEAR: Duration = Duration::from_hours(365 * 24);

    fn quiet() -> Rows {
        Rows {
            oldest: NOW - 3 * DAY,
            newest: NOW - 2 * DAY,
            readable: false,
        }
    }

    fn nobody() -> Facts {
        Facts::default()
    }

    fn verdict(channel: &str, facts: &Facts, rows: &Rows) -> Option<Keep> {
        keep(channel, Some(facts), Some(rows), NOW, YEAR)
    }

    const THREAD: &str = "#monorepo/web-c26b2cc";

    #[test]
    fn the_unregister_code_is_read_as_chanserv_gave_it() {
        let notice = "To confirm, run this command: /CS UNREGISTER #proj/ours aB3dE9";
        assert_eq!(
            confirmation(notice, "#proj/ours").as_deref(),
            Some("aB3dE9")
        );
        assert_eq!(
            confirmation(notice, "#proj/OURS").as_deref(),
            Some("aB3dE9")
        );
        assert_eq!(confirmation(notice, "#proj/other"), None);
        assert_eq!(
            confirmation("Warning: unregistering this channel", "#proj/ours"),
            None
        );
    }

    #[test]
    fn an_unregistered_quiet_unused_empty_thread_is_cleaned() {
        assert_eq!(verdict(THREAD, &nobody(), &quiet()), None);
    }

    #[test]
    fn one_registered_by_the_server_itself_is_cleaned_too() {
        let ours = Facts {
            founder: Some("ZIRC".into()),
            ..nobody()
        };
        assert_eq!(verdict(THREAD, &ours, &quiet()), None);
    }

    #[test]
    fn channels_and_standing_rooms_are_kept() {
        for channel in [
            "#monorepo",
            "#general",
            "#monorepo/agents",
            "#monorepo/landings",
        ] {
            assert_eq!(
                verdict(channel, &nobody(), &quiet()),
                Some(Keep::NotThread),
                "{channel}"
            );
        }
    }

    #[test]
    fn one_registered_by_anyone_else_is_kept() {
        let theirs = Facts {
            founder: Some("mattias".into()),
            ..nobody()
        };
        assert_eq!(verdict(THREAD, &theirs, &quiet()), Some(Keep::Registered));
    }

    #[test]
    fn one_with_someone_in_it_is_kept() {
        let present = Facts {
            present: true,
            ..nobody()
        };
        assert_eq!(verdict(THREAD, &present, &quiet()), Some(Keep::Present));
    }

    #[test]
    fn one_something_readable_was_said_in_is_kept() {
        let used = Rows {
            readable: true,
            ..quiet()
        };
        assert_eq!(verdict(THREAD, &nobody(), &used), Some(Keep::Readable));
    }

    #[test]
    fn one_older_than_its_history_is_kept() {
        let old = Rows {
            oldest: NOW - 365 * DAY + HOUR,
            ..quiet()
        };
        assert_eq!(verdict(THREAD, &nobody(), &old), Some(Keep::Trimmed));
    }

    #[test]
    fn with_history_kept_for_ever_none_is_older_than_it() {
        // Ergo's expire-time 0, the default: nothing expires, so no history is partial.
        let ancient = Rows {
            oldest: 0,
            ..quiet()
        };
        assert_eq!(
            keep(THREAD, Some(&nobody()), Some(&ancient), NOW, Duration::MAX),
            None
        );
    }

    #[test]
    fn one_made_or_used_in_the_last_day_is_kept() {
        let made = Rows {
            oldest: NOW - 23 * HOUR,
            newest: NOW - 23 * HOUR,
            ..quiet()
        };
        let joined = Rows {
            newest: NOW - HOUR,
            ..quiet()
        };
        assert_eq!(verdict(THREAD, &nobody(), &made), Some(Keep::Recent));
        assert_eq!(verdict(THREAD, &nobody(), &joined), Some(Keep::Recent));
    }

    #[test]
    fn anything_not_known_keeps_it() {
        assert_eq!(
            keep(THREAD, None, Some(&quiet()), NOW, YEAR),
            Some(Keep::Unknown)
        );
        assert_eq!(
            keep(THREAD, Some(&nobody()), None, NOW, YEAR),
            Some(Keep::Unknown)
        );
    }

    fn item(nick: &str, kind: i64, text: &str) -> serde_json::Value {
        serde_json::json!({"Type": kind, "Nick": format!("{nick}!u@h"), "Message": {"Message": text}})
    }

    #[test]
    fn the_history_counts_messages_but_not_services_or_title_cards() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let card = zirc_proto::encode(&zirc_proto::Message::new(
            "cc-1",
            zirc_proto::Event::Custom {
                kind: zirc_proto::session::KIND.into(),
                data: serde_json::Value::Null,
                text: "cc-1 works on web".into(),
            },
        ))
        .unwrap();
        // Only a join, HistServ's playback and a title card.
        add_item(&path, "a1", "#r/quiet", 10, &item("m", 3, ""));
        add_item(
            &path,
            "a2",
            "#r/quiet",
            11,
            &item("HistServ", 1, "m joined"),
        );
        add_item(&path, "a3", "#r/quiet", 12, &item("m", 1, &card));
        // A word said.
        add(&path, "b1", "#r/used", 20, "m", "hello");
        // A notice counts too.
        add_item(&path, "c1", "#r/noticed", 30, &item("m", 2, "fyi"));
        // A top-level channel's messages aren't read for rule 3.
        add(&path, "d1", "#r", 40, "m", "hi");
        let read = read(&path).unwrap();
        assert_eq!(
            read.channels["#r/quiet"],
            Rows {
                oldest: 10,
                newest: 12,
                readable: false
            }
        );
        assert!(read.channels["#r/used"].readable);
        assert!(read.channels["#r/noticed"].readable);
        assert!(!read.channels["#r"].readable);
        assert_eq!(read.newest, Some(40));
    }

    #[test]
    fn a_message_that_cant_be_read_counts_as_readable() {
        assert!(readable(b"not json"));
    }

    #[test]
    fn an_empty_or_stale_history_is_refused() {
        assert_eq!(sound(&History::default(), NOW), Some(Refusal::EmptyHistory));
        let stale = History {
            newest: Some(NOW - 8 * DAY),
            ..History::default()
        };
        assert_eq!(sound(&stale, NOW), Some(Refusal::StaleHistory));
        let fresh = History {
            newest: Some(NOW - HOUR),
            ..History::default()
        };
        assert_eq!(sound(&fresh, NOW), None);
    }

    #[test]
    fn candidates_say_how_many_each_rule_kept() {
        let facts = BTreeMap::from([
            (THREAD.to_string(), nobody()),
            ("#monorepo".to_string(), nobody()),
            (
                "#monorepo/b".to_string(),
                Facts {
                    present: true,
                    ..nobody()
                },
            ),
        ]);
        let history = History {
            channels: BTreeMap::from([
                (THREAD.to_string(), quiet()),
                ("#monorepo/b".to_string(), quiet()),
            ]),
            newest: Some(NOW - HOUR),
        };
        let (clean, kept) = candidates(&facts, &history, NOW, YEAR);
        assert_eq!(clean, vec![THREAD.to_string()]);
        assert_eq!(
            kept,
            BTreeMap::from([(Keep::NotThread, 1), (Keep::Present, 1)])
        );
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("#r/t{i:03}")).collect()
    }

    #[test]
    fn it_announces_then_cleans_what_is_still_a_candidate_an_hour_later() {
        let mut janitor = Janitor::default();
        let first = janitor.step(Ok(names(2)), NOW);
        assert_eq!(
            first,
            Step::Announce {
                channels: names(2),
                at: NOW + HOUR,
                more: 0
            }
        );
        // Not yet.
        assert_eq!(janitor.step(Ok(names(2)), NOW + HOUR / 2), Step::Nothing);
        // An hour on, one has someone in it now: only the other goes.
        assert_eq!(
            janitor.step(Ok(vec!["#r/t000".into()]), NOW + HOUR),
            Step::Clean {
                channels: vec!["#r/t000".into()],
                dropped: vec!["#r/t001".into()]
            }
        );
        // And the next run announces afresh before cleaning anything else.
        assert!(matches!(
            janitor.step(Ok(vec!["#r/t001".into()]), NOW + 2 * HOUR),
            Step::Announce { .. }
        ));
    }

    #[test]
    fn a_channel_that_appears_after_the_notice_waits_for_its_own() {
        let mut janitor = Janitor::default();
        janitor.step(Ok(vec!["#r/t000".into()]), NOW);
        let later = janitor.step(Ok(vec!["#r/t000".into(), "#r/t001".into()]), NOW + HOUR);
        assert_eq!(
            later,
            Step::Clean {
                channels: vec!["#r/t000".into()],
                dropped: vec![]
            }
        );
    }

    #[test]
    fn a_run_cleans_at_most_the_cap() {
        let mut janitor = Janitor::default();
        let Step::Announce { channels, more, .. } = janitor.step(Ok(names(CAP + 5)), NOW) else {
            panic!("no announcement");
        };
        assert_eq!(channels.len(), CAP);
        assert_eq!(more, 5);
        let Step::Clean { channels, .. } = janitor.step(Ok(names(CAP + 5)), NOW + HOUR) else {
            panic!("no clean");
        };
        assert_eq!(channels.len(), CAP);
    }

    #[test]
    fn too_many_candidates_or_a_refusal_does_nothing_and_forgets_the_notice() {
        let mut janitor = Janitor::default();
        assert_eq!(
            janitor.step(Ok(names(MOST + 1)), NOW),
            Step::Refuse(Refusal::TooMany(MOST + 1))
        );
        janitor.step(Ok(names(1)), NOW);
        assert_eq!(
            janitor.step(Err(Refusal::NoList("down".into())), NOW + HOUR),
            Step::Refuse(Refusal::NoList("down".into()))
        );
        // The notice is gone: the next run announces again rather than cleaning.
        assert!(matches!(
            janitor.step(Ok(names(1)), NOW + 2 * HOUR),
            Step::Announce { .. }
        ));
    }
}
