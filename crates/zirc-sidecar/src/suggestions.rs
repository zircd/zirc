//! Suggestions for the team's playbook ([`zirc_proto::suggestions`]): what the activity store
//! ([`crate::activity`]) shows went wrong for several sessions the same way, said in
//! `#agents`, saying its repository, as zirc's own account for the sessions to put into words and a person
//! to keep or mute.
//!
//! The first kind: a test target that failed the first landing of
//! [`zirc_proto::suggestions::SESSIONS`] different sessions within
//! [`zirc_proto::suggestions::DAYS`] ([`noticed`]), proposed as a gotcha for the paths most of
//! those sessions' branches changed.
//!
//! Where each suggestion stands is kept beside Ergo's history ([`KEPT`]), written whole and
//! renamed into place. What moves one on is read from the history itself, the rows after the
//! last read ([`Kept::heard`]): the server's own message, for its id; a reply to it, as the
//! lesson's words; and a person's reaction to the proposal. Only the store's metadata is read
//! to notice anything, and nothing of what anyone said is logged.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zirc_proto::Event;
use zirc_proto::suggestions::{
    DAYS, DRAFT_SECS, KEEP, MUTE, MUTED_DAYS, PATHS, SESSIONS, State, Suggestion,
};

use crate::exporter::{PRIVMSG, TAGMSG};
use crate::pusher::kept::{self as history, Place};

/// The file beside Ergo's history the suggestions are kept in.
pub const KEPT: &str = "zirc-suggestions.json";

/// How often the history is read for replies and reactions.
pub const EVERY: Duration = Duration::from_secs(30);

/// How often the activity store is read for something new to suggest.
pub const LOOK: Duration = Duration::from_mins(10);

/// How long a step said waits to be read back from the history before it's said again: it
/// wasn't heard, as when Ergo was restarting.
pub const AGAIN: u64 = 600;

const DAY: u64 = 24 * 3_600;

/// Something noticed: a target that failed several sessions' first landings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Noticed {
    /// The repository's scope, such as `#api` ([`zirc_chat::names::repo_scope`]).
    pub repo: String,
    /// The target.
    pub target: String,
    /// The sessions, oldest first.
    pub sessions: Vec<String>,
    /// The paths most of their branches changed.
    pub paths: Vec<String>,
}

/// The directory a changed `path` counts under: its parent, or itself at the root.
fn directory(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(parent, _)| parent)
}

/// The repository's scope a row kept under `channel` is about: the scope itself, or one of
/// the rooms repositories had under their channels before 20 Nov 2026.
fn repo_of(channel: &str) -> String {
    channel.split('/').next().unwrap_or(channel).to_lowercase()
}

fn text(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A repository's channel and a branch there.
type Branch = (String, String);

/// Each branch's first landing that ended, tries aside, among the `landings` said (each its
/// channel and data, oldest first): its build, its session and the targets it failed.
fn first_landings(landings: Vec<(String, Value)>) -> BTreeMap<Branch, (u64, String, Vec<String>)> {
    let mut first: BTreeMap<Branch, (u64, String, Vec<String>)> = BTreeMap::new();
    for (channel, data) in landings {
        let state = text(&data, "state").unwrap_or_default();
        let tried = data.get("try").and_then(Value::as_bool).unwrap_or(false);
        let build = text(&data, "build").and_then(|build| build.parse::<u64>().ok());
        let (Some(branch), Some(session), Some(build)) =
            (text(&data, "branch"), text(&data, "session"), build)
        else {
            continue;
        };
        if tried || !matches!(state.as_str(), "passed" | "failed") {
            continue;
        }
        let failed = data.get("failed").and_then(Value::as_array);
        let failed = failed
            .into_iter()
            .flatten()
            .filter_map(|target| target.as_str().map(str::to_owned));
        match first.get_mut(&(repo_of(&channel), branch.clone())) {
            Some(known) if known.0 < build => {}
            // The same build said again, such as with what failed: what it failed is added.
            Some(known) if known.0 == build => known.2.extend(failed),
            _ => {
                first.insert(
                    (repo_of(&channel), branch),
                    (build, session, failed.collect()),
                );
            }
        }
    }
    first
}

/// The directories each branch changed, from what was `said` of it: its commits and pushes.
fn changed(said: Vec<(String, Value)>) -> BTreeMap<Branch, BTreeSet<String>> {
    let mut changed: BTreeMap<Branch, BTreeSet<String>> = BTreeMap::new();
    for (channel, data) in said {
        let Some(branch) = text(&data, "branch") else {
            continue;
        };
        let paths = data.get("paths").and_then(Value::as_array);
        changed
            .entry((repo_of(&channel), branch))
            .or_default()
            .extend(
                paths
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|path| directory(path).to_owned()),
            );
    }
    changed
}

/// What the activity store shows since `since` (nanoseconds) worth suggesting: each target
/// that failed the first landing of [`SESSIONS`] sessions or more, whose branches changed
/// some directory in common. A branch's first landing is its lowest build that passed or
/// failed, tries aside.
///
/// # Errors
///
/// When the store can't be read.
pub fn noticed(activity: &Connection, since: i64) -> rusqlite::Result<Vec<Noticed>> {
    let rows = |kinds: &str, ci: bool| -> rusqlite::Result<Vec<(String, Value)>> {
        let mut statement = activity.prepare(&format!(
            "SELECT channel, data, account FROM rows WHERE kind IN ({kinds}) AND nanotime >= ?1 \
             AND channel IS NOT NULL AND data IS NOT NULL ORDER BY nanotime, id"
        ))?;
        let read = statement.query_map([since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        Ok(read
            .filter_map(Result::ok)
            // A landing is CI's word alone: rows kept before the store said so are passed over.
            .filter(|(_, _, account)| !ci || zirc_proto::ci::speaks_for_ci(account.as_deref()))
            .filter_map(|(channel, data, _)| Some((channel, serde_json::from_str(&data).ok()?)))
            .collect())
    };
    let first = first_landings(rows(&format!("'{}'", zirc_proto::ci::LANDING_KIND), true)?);
    let changed = changed(rows("'commit', 'push'", false)?);
    // Per repository and target: the sessions it failed, and their branches.
    let mut failures: BTreeMap<(String, String), (Vec<String>, Vec<String>)> = BTreeMap::new();
    let mut by_build: Vec<_> = first.into_iter().collect();
    by_build.sort_by_key(|(_, (build, ..))| *build);
    for ((repo, branch), (_, session, failed)) in by_build {
        for target in failed.into_iter().collect::<BTreeSet<_>>() {
            let (sessions, branches) = failures.entry((repo.clone(), target)).or_default();
            if !sessions.contains(&session) {
                sessions.push(session.clone());
            }
            branches.push(branch.clone());
        }
    }
    let mut found = Vec::new();
    for ((repo, target), (sessions, branches)) in failures {
        if sessions.len() < SESSIONS {
            continue;
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for branch in &branches {
            for dir in changed
                .get(&(repo.clone(), branch.clone()))
                .into_iter()
                .flatten()
            {
                *counts.entry(dir).or_default() += 1;
            }
        }
        // Those more than one of the branches changed, the most shared first, and none that
        // holds another of them: a gotcha for `web` would be told to whoever changes anything
        // there, when `web/e2e` says where it happened.
        let mut shared: Vec<(&str, usize)> = counts.into_iter().filter(|(_, n)| *n > 1).collect();
        shared.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let holds = |dir: &str| {
            shared.iter().any(|(other, _)| {
                other
                    .strip_prefix(dir)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
        };
        let paths: Vec<String> = shared
            .iter()
            .filter(|(dir, _)| !holds(dir))
            .take(PATHS)
            .map(|(dir, _)| (*dir).to_owned())
            .collect();
        if !paths.is_empty() {
            found.push(Noticed {
                repo,
                target,
                sessions,
                paths,
            });
        }
    }
    Ok(found)
}

/// One suggestion, as kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The scope of the repository it's about, which its lines in `#agents` say.
    pub repo: String,
    /// The suggestion, as its last step said it.
    pub suggestion: Suggestion,
    /// When it came to stand as it does, in seconds since the Unix epoch.
    pub at: u64,
    /// Its last step's message id, once the history has it: what a reply or a reaction names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// When that step was last said.
    #[serde(default)]
    pub said: u64,
}

impl Entry {
    /// Its step as it stands, to say.
    fn say(&self) -> Say {
        Say {
            channel: zirc_proto::AGENTS_CHANNEL.to_owned(),
            event: self.suggestion.event(),
            repo: Some(self.repo.trim_start_matches('#').to_owned()),
        }
    }
}

/// Something to say, as zirc's own account.
#[derive(Debug, Clone, PartialEq)]
pub struct Say {
    /// Where.
    pub channel: String,
    /// What.
    pub event: Event,
    /// The repository it's about, by name ([`zirc_proto::Message::repo`]), for a line in a
    /// room every repository shares, as a gotcha's.
    pub repo: Option<String>,
}

/// The suggestions, and where the history was read to.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// The last row of Ergo's history read; `None` before any was.
    #[serde(default)]
    pub last: Option<i64>,
    /// A row Ergo was still writing at the last round, read past at this one if it still is
    /// ([`history::after`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<i64>,
    /// The suggestions, by id.
    #[serde(default)]
    pub suggestions: BTreeMap<String, Entry>,
}

impl Kept {
    /// Moves the suggestion `id` on to `state`, with the lesson's `text` and whose it is or
    /// who decided, at `now`: what to say of it.
    fn step(
        &mut self,
        id: &str,
        state: State,
        text: Option<String>,
        by: Option<String>,
        now: u64,
    ) -> Vec<Say> {
        let Some(entry) = self.suggestions.get_mut(id) else {
            return Vec::new();
        };
        entry.suggestion.state = state;
        if text.is_some() {
            entry.suggestion.text = text;
        }
        // Kept, the lesson stays its writer's; who kept it is said, not kept.
        let said = Suggestion {
            by: by.clone().or_else(|| entry.suggestion.by.clone()),
            ..entry.suggestion.clone()
        };
        if state == State::Proposed {
            entry.suggestion.by = by;
        }
        entry.at = now;
        entry.said = now;
        entry.message = None;
        let mut says = vec![Say {
            channel: zirc_proto::AGENTS_CHANNEL.to_owned(),
            event: said.event(),
            repo: Some(entry.repo.trim_start_matches('#').to_owned()),
        }];
        if state == State::Kept {
            says.push(Say {
                channel: zirc_proto::gotchas::CHANNEL.to_owned(),
                event: entry.suggestion.gotcha_event(),
                repo: Some(entry.repo.trim_start_matches('#').to_owned()),
            });
        }
        says
    }

    /// Takes what was `noticed`, at `now`: the one not yet suggested that the most sessions
    /// met is asked about. One each time, so a channel hears them a look apart ([`LOOK`]),
    /// never all at once as the first look at a week would have it.
    pub fn notice(&mut self, mut noticed: Vec<Noticed>, now: u64) -> Vec<Say> {
        noticed.sort_by_key(|found| std::cmp::Reverse(found.sessions.len()));
        let mut says = Vec::new();
        for found in noticed {
            let id = Suggestion::id_of(&found.repo, &found.target);
            if self.suggestions.contains_key(&id) {
                continue;
            }
            let suggestion = Suggestion {
                id: id.clone(),
                state: State::Asked,
                target: found.target,
                sessions: found.sessions,
                paths: found.paths,
                text: None,
                by: None,
            };
            let entry = Entry {
                repo: found.repo,
                suggestion,
                at: now,
                message: None,
                said: now,
            };
            says.push(entry.say());
            self.suggestions.insert(id, entry);
            break;
        }
        says
    }

    /// What the time passing does, at `now`: one asked about for [`DRAFT_SECS`] with no reply
    /// is proposed as the plain facts, one muted for [`MUTED_DAYS`] is forgotten, to be
    /// suggested again if it's noticed again, and a question or proposal the history never
    /// showed ([`AGAIN`]) is said again.
    pub fn tick(&mut self, now: u64) -> Vec<Say> {
        self.suggestions.retain(|_, entry| {
            entry.suggestion.state != State::Muted || now < entry.at + u64::from(MUTED_DAYS) * DAY
        });
        let waited: Vec<String> = self
            .suggestions
            .iter()
            .filter(|(_, entry)| {
                entry.suggestion.state == State::Asked && now >= entry.at + DRAFT_SECS
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut says: Vec<Say> = waited
            .into_iter()
            .flat_map(|id| self.step(&id, State::Proposed, None, Some("zirc".into()), now))
            .collect();
        for entry in self.suggestions.values_mut() {
            let open = matches!(entry.suggestion.state, State::Asked | State::Proposed);
            if open && entry.message.is_none() && now >= entry.said + AGAIN {
                entry.said = now;
                says.push(entry.say());
            }
        }
        says
    }

    /// What a row of the history does, at `now`: `person` says whether an account is a
    /// person's own.
    pub(crate) fn heard(
        &mut self,
        row: &history::Kept,
        person: impl Fn(&str) -> bool,
        now: u64,
    ) -> Vec<Say> {
        let Place::Channel(_) = &row.place else {
            return Vec::new();
        };
        let account = row.said.account.to_lowercase();
        let message = (row.said.kind == PRIVMSG)
            .then(|| zirc_proto::decode(&row.said.text).ok().flatten())
            .flatten();
        // The server's own step, read back: its message's id.
        if account == zirc_proto::names::SYSTEM {
            if let Some(said) = message.as_ref().and_then(|m| Suggestion::of(&m.event))
                && let Some(entry) = self.suggestions.get_mut(&said.id)
                && entry.suggestion.state == said.state
            {
                entry.message = Some(row.msgid.clone());
            }
            return Vec::new();
        }
        let Some(replied) = row.said.tags.get("+draft/reply") else {
            return Vec::new();
        };
        let Some((id, state)) = self
            .suggestions
            .iter()
            // The server's ids are its own: one names one message, wherever it was said.
            .find(|(_, entry)| entry.message.as_ref() == Some(replied))
            .map(|(id, entry)| (id.clone(), entry.suggestion.state))
        else {
            return Vec::new();
        };
        match (state, row.said.kind) {
            // The first reply to the question, a session's or a person's: the lesson's words.
            (State::Asked, PRIVMSG) => {
                let (words, session) = match message {
                    Some(zirc_proto::Message {
                        event: Event::Text { body },
                        session,
                        ..
                    }) => (body, Some(session).filter(|s| !s.is_empty())),
                    Some(_) => return Vec::new(),
                    None => (row.said.text.clone(), None),
                };
                let words = words.lines().next().unwrap_or_default().trim().to_owned();
                if words.is_empty() {
                    return Vec::new();
                }
                self.step(
                    &id,
                    State::Proposed,
                    Some(words),
                    session.or(Some(account)),
                    now,
                )
            }
            // A person's answer to the proposal.
            (State::Proposed, TAGMSG) if person(&account) => {
                match row.said.tags.get("+draft/react").map(String::as_str) {
                    Some(KEEP) => self.step(&id, State::Kept, None, Some(account), now),
                    Some(MUTE) => self.step(&id, State::Muted, None, Some(account), now),
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }
}

/// The suggestions kept beside `history`, as last saved there.
#[must_use]
pub fn kept(history: &Path) -> Kept {
    std::fs::read(history.with_file_name(KEPT))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Keeps `kept` beside `history`.
///
/// # Errors
///
/// When it can't be written.
pub fn keep(history: &Path, kept: &Kept) -> std::io::Result<()> {
    crate::save_json(&history.with_file_name(KEPT), kept)
}

/// One round, at `now` (seconds): reads the history's new rows, lets the time pass and, when
/// it's time to `look`, the activity store; keeps the result and returns what to say. The
/// first time, it starts from the history's end.
///
/// # Errors
///
/// When the history or the store can't be read, or the result kept.
pub fn round(
    history_path: &Path,
    person: impl Fn(&str) -> bool,
    look: bool,
    now: u64,
) -> Result<Vec<Say>, Box<dyn std::error::Error + Send + Sync>> {
    if !history_path.exists() {
        return Ok(Vec::new());
    }
    let mut kept = kept(history_path);
    let before = serde_json::to_vec(&kept)?;
    let connection = crate::trim::open(history_path)?;
    let mut says = Vec::new();
    match kept.last {
        None => kept.last = Some(history::newest(&connection)?),
        Some(mut last) => {
            loop {
                let read = history::after(&connection, last, kept.waiting)?;
                for row in &read.kept {
                    says.extend(kept.heard(row, &person, now));
                }
                let moved = read.newest != last;
                last = read.newest;
                if read.unplaced.is_some() || !moved {
                    kept.waiting = read.unplaced;
                    break;
                }
            }
            kept.last = Some(last);
        }
    }
    says.extend(kept.tick(now));
    let store = crate::activity::path_for(history_path);
    if look && store.exists() {
        let activity = crate::activity::open(&store)?;
        let since = i64::try_from(now.saturating_sub(u64::from(DAYS) * DAY))
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000_000_000);
        says.extend(kept.notice(noticed(&activity, since)?, now));
    }
    if serde_json::to_vec(&kept)? != before {
        keep(history_path, &kept)?;
    }
    Ok(says)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use zirc_proto::Message;

    use super::*;
    use crate::activity::{Fact, insert};
    use crate::exporter::Said;

    const SECOND: i64 = 1_000_000_000;

    fn fact(kind: &str, data: Value) -> Fact {
        Fact {
            kind: kind.into(),
            data: Some(data),
            ..Fact::default()
        }
    }

    /// A landing's end CI said in `#api/landings`, its build numbered as its time, and a
    /// commit of its branch under `paths`.
    fn landing(
        store: &Connection,
        at: i64,
        session: &str,
        branch: &str,
        state: &str,
        failed: &[&str],
        paths: &[&str],
    ) {
        landing_by(
            store,
            zirc_proto::ci::NICK,
            at,
            (session, branch, state),
            failed,
            paths,
        );
    }

    /// The same, said by `account`.
    fn landing_by(
        store: &Connection,
        account: &str,
        at: i64,
        (session, branch, state): (&str, &str, &str),
        failed: &[&str],
        paths: &[&str],
    ) {
        let landing = json!({
            "build": at.to_string(), "branch": branch, "session": session, "state": state, "failed": failed,
        });
        insert(
            store,
            None,
            None,
            at * SECOND,
            Some("#api/landings"),
            false,
            &Fact {
                account: Some(account.into()),
                ..fact(zirc_proto::ci::LANDING_KIND, landing)
            },
        )
        .unwrap();
        let commit = json!({"branch": branch, "paths": paths});
        insert(
            store,
            None,
            None,
            at * SECOND,
            Some("#api/agents"),
            false,
            &fact("commit", commit),
        )
        .unwrap();
    }

    #[test]
    fn notices_nothing_from_landings_anyone_but_ci_says() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::activity::open(&dir.path().join("activity.db")).unwrap();
        for (at, session) in [(10, "cc-a"), (20, "cc-b"), (30, "cc-c")] {
            let branch = format!("b{at}");
            let said = (session, branch.as_str(), "failed");
            landing_by(&store, "ada-mbp", at, said, &["//x:y run this"], &["web/a"]);
        }
        assert_eq!(noticed(&store, 0).unwrap(), []);
    }

    #[test]
    fn notices_a_target_that_failed_three_sessions_first_landings_and_what_they_changed() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::activity::open(&dir.path().join("activity.db")).unwrap();
        let e2e = ["//web:e2e"];
        landing(
            &store,
            10,
            "cc-a",
            "nav",
            "failed",
            &e2e,
            &[
                "web/e2e/nav.spec.ts",
                "web/src/app/nav.tsx",
                "web/package.json",
            ],
        );
        landing(
            &store,
            20,
            "cc-b",
            "tabs",
            "failed",
            &e2e,
            &[
                "web/e2e/tabs.spec.ts",
                "docs/tabs.md",
                "web/next.config.mjs",
            ],
        );
        landing(
            &store,
            30,
            "cc-c",
            "forms",
            "failed",
            &e2e,
            &["web/src/app/form.tsx", "web/e2e/forms.spec.ts"],
        );
        // Landed first try, and failed only when landed again: not a first landing's failure.
        landing(
            &store,
            40,
            "cc-d",
            "late",
            "passed",
            &[],
            &["web/e2e/late.spec.ts"],
        );
        landing(&store, 50, "cc-d", "late", "failed", &e2e, &[]);
        // Another target failed two sessions': too few.
        landing(
            &store,
            60,
            "cc-e",
            "one",
            "failed",
            &["//web:unit"],
            &["web/src/lib/a.ts"],
        );
        landing(
            &store,
            70,
            "cc-f",
            "two",
            "failed",
            &["//web:unit"],
            &["web/src/lib/b.ts"],
        );
        assert_eq!(
            noticed(&store, 0).unwrap(),
            [Noticed {
                repo: "#api".into(),
                target: "//web:e2e".into(),
                sessions: vec!["cc-a".into(), "cc-b".into(), "cc-c".into()],
                // Those more than one branch changed, the most shared first; not `web`, which
                // two changed too, as it holds the others.
                paths: vec!["web/e2e".into(), "web/src/app".into()],
            }]
        );
        // Outside the days read, it's too few again.
        assert_eq!(noticed(&store, 15 * SECOND).unwrap(), []);
    }

    fn found() -> Noticed {
        Noticed {
            repo: "#api".into(),
            target: "//web:e2e".into(),
            sessions: vec!["cc-a".into(), "cc-b".into(), "cc-c".into()],
            paths: vec!["web/e2e".into()],
        }
    }

    fn row(
        msgid: &str,
        account: &str,
        kind: i64,
        text: String,
        tags: &[(&str, &str)],
    ) -> history::Kept {
        history::Kept {
            msgid: msgid.into(),
            nanotime: 0,
            place: Place::Channel("#agents".into()),
            said: Said {
                kind,
                nick: account.into(),
                mask: format!("{account}!u@h"),
                account: account.into(),
                text,
                tags: tags
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            },
        }
    }

    /// The server's own step, as the history gives it back with the id `msgid`.
    fn read_back(kept: &mut Kept, say: &Say, msgid: &str) {
        let body =
            zirc_proto::encode_up_to(&Message::new("server", say.event.clone()), 4096).unwrap();
        let own = row(msgid, zirc_proto::names::SYSTEM, PRIVMSG, body, &[]);
        assert!(kept.heard(&own, |_| false, 0).is_empty());
    }

    fn state_of(say: &Say) -> Suggestion {
        Suggestion::of(&say.event).unwrap()
    }

    #[test]
    fn a_sessions_reply_is_proposed_and_a_persons_thumb_keeps_it_as_a_gotcha() {
        let person = |account: &str| account == "ada";
        let mut kept = Kept::default();
        let asked = kept.notice(vec![found()], 100);
        assert_eq!(
            (
                asked.len(),
                asked[0].channel.as_str(),
                asked[0].repo.as_deref()
            ),
            (1, "#agents", Some("api"))
        );
        assert_eq!(state_of(&asked[0]).state, State::Asked);
        // Noticed again, it isn't asked about twice.
        assert!(kept.notice(vec![found()], 200).is_empty());
        read_back(&mut kept, &asked[0], "m1");

        let words = Message::new(
            "cc-b",
            Event::Text {
                body: "Run //web:e2e before landing here.\nmore".into(),
            },
        );
        let reply = row(
            "m2",
            "ada-mbp",
            PRIVMSG,
            zirc_proto::encode_up_to(&words, 4096).unwrap(),
            &[("+draft/reply", "m1")],
        );
        let proposed = kept.heard(&reply, person, 300);
        let proposal = state_of(&proposed[0]);
        assert_eq!(
            (
                proposal.state,
                proposal.text.as_deref(),
                proposal.by.as_deref()
            ),
            (
                State::Proposed,
                Some("Run //web:e2e before landing here."),
                Some("cc-b")
            )
        );
        read_back(&mut kept, &proposed[0], "m3");

        let react = |account: &str, emoji: &str| {
            row(
                "t",
                account,
                TAGMSG,
                String::new(),
                &[("+draft/reply", "m3"), ("+draft/react", emoji)],
            )
        };
        // A machine's reaction is no one's decision, nor is another emoji a person's.
        assert!(kept.heard(&react("ada-mbp", KEEP), person, 400).is_empty());
        assert!(kept.heard(&react("ada", "🎉"), person, 400).is_empty());
        let says = kept.heard(&react("ada", KEEP), person, 500);
        assert_eq!(says.len(), 2);
        assert_eq!(state_of(&says[0]).state, State::Kept);
        assert!(
            state_of(&says[0])
                .says()
                .starts_with("ada kept the gotcha for web/e2e: Run //web:e2e")
        );
        // The gotcha, in the gotchas room saying its repository, as any is said.
        assert_eq!(
            (says[1].channel.as_str(), says[1].repo.as_deref()),
            ("#gotchas", Some("api"))
        );
        assert_eq!(
            zirc_proto::gotchas::lesson(&says[1].event),
            Some((
                vec!["web/e2e".to_owned()],
                "Run //web:e2e before landing here.".to_owned()
            ))
        );
        // Decided, it's never suggested again.
        assert!(kept.heard(&react("ada", MUTE), person, 600).is_empty());
        assert!(kept.notice(vec![found()], 700).is_empty());
    }

    #[test]
    fn several_noticed_at_once_are_asked_about_one_each_look_the_most_met_first() {
        let smoke = Noticed {
            target: "//web:smoke".into(),
            sessions: vec!["cc-a".into(), "cc-b".into(), "cc-c".into(), "cc-d".into()],
            ..found()
        };
        let mut kept = Kept::default();
        let first = kept.notice(vec![found(), smoke.clone()], 0);
        assert_eq!(first.len(), 1);
        assert_eq!(state_of(&first[0]).target, "//web:smoke");
        let second = kept.notice(vec![found(), smoke.clone()], 600);
        assert_eq!(second.len(), 1);
        assert_eq!(state_of(&second[0]).target, "//web:e2e");
        assert!(kept.notice(vec![found(), smoke], 1_200).is_empty());
    }

    #[test]
    fn unanswered_for_an_hour_the_plain_facts_are_proposed_and_muted_it_rests_seven_days() {
        let person = |account: &str| account == "ada";
        let mut kept = Kept::default();
        let asked = kept.notice(vec![found()], 0);
        // Never read back, as when the server was restarting: said again, once.
        assert!(kept.tick(AGAIN - 1).is_empty());
        assert_eq!(kept.tick(AGAIN), asked);
        assert!(kept.tick(AGAIN + 1).is_empty());
        read_back(&mut kept, &asked[0], "m1");
        assert!(kept.tick(DRAFT_SECS - 1).is_empty());
        let proposed = kept.tick(DRAFT_SECS);
        let proposal = state_of(&proposed[0]);
        assert_eq!(
            (proposal.state, proposal.by.as_deref()),
            (State::Proposed, Some("zirc"))
        );
        assert!(proposal.says().contains(&proposal.facts()));
        read_back(&mut kept, &proposed[0], "m2");
        // A reply now is too late to be its words.
        let late = Message::new(
            "cc-a",
            Event::Text {
                body: "late".into(),
            },
        );
        let reply = row(
            "m3",
            "ada-mbp",
            PRIVMSG,
            zirc_proto::encode_up_to(&late, 4096).unwrap(),
            &[("+draft/reply", "m2")],
        );
        assert!(kept.heard(&reply, person, DRAFT_SECS + 1).is_empty());

        let no = row(
            "t",
            "ada",
            TAGMSG,
            String::new(),
            &[("+draft/reply", "m2"), ("+draft/react", MUTE)],
        );
        let muted = kept.heard(&no, person, DRAFT_SECS + 2);
        assert_eq!((muted.len(), state_of(&muted[0]).state), (1, State::Muted));
        let rested = DRAFT_SECS + 2 + u64::from(MUTED_DAYS) * DAY;
        assert!(kept.tick(rested - 1).is_empty());
        assert!(kept.notice(vec![found()], rested - 1).is_empty());
        assert!(kept.tick(rested).is_empty());
        assert_eq!(kept.notice(vec![found()], rested).len(), 1);
    }

    #[test]
    fn a_round_starts_from_the_historys_end_and_keeps_where_it_stands() {
        use crate::trim::testing::{add, add_item, history as a_history};
        let dir = tempfile::tempdir().unwrap();
        let path = a_history(dir.path());
        add(&path, "old", "#api", 1, "bob", "before it ran");
        let store = crate::activity::open(&crate::activity::path_for(&path)).unwrap();
        for (i, session) in ["cc-a", "cc-b", "cc-c"].iter().enumerate() {
            let at = 1_000 + i64::try_from(i).unwrap();
            landing(
                &store,
                at,
                session,
                session,
                "failed",
                &["//web:e2e"],
                &["web/e2e/x.spec.ts"],
            );
        }
        drop(store);
        let says = round(&path, |_| false, true, 2_000).unwrap();
        assert_eq!(says.len(), 1);
        let asked = state_of(&says[0]);
        // Its own message, then a reply to it, read from the history on the next round.
        let body =
            zirc_proto::encode_up_to(&Message::new("server", says[0].event.clone()), 4096).unwrap();
        add(&path, "m1", "#api", 2, zirc_proto::names::SYSTEM, &body);
        add_item(
            &path,
            "m2",
            "#api",
            3,
            &json!({
                "Type": 1, "Nick": "ada!u@h", "AccountName": "ada",
                "Message": {"Message": "Run it first.", "Msgid": "m2"},
                "Tags": {"+draft/reply": "m1"},
            }),
        );
        let says = round(&path, |_| false, false, 2_100).unwrap();
        let proposal = state_of(&says[0]);
        assert_eq!(
            (proposal.id, proposal.state, proposal.text.as_deref()),
            (asked.id, State::Proposed, Some("Run it first."))
        );
        assert_eq!(kept(&path).suggestions.len(), 1);
        assert!(round(&path, |_| false, true, 2_200).unwrap().is_empty());
    }
}
