//! A count, kept on this machine, of how sessions' watches woke and whether the session
//! acted on it: by day and reason ([`Why`]), how many wakes, and how many the session
//! followed within [`WAKE_ACTED_SECS`] by posting, committing, pushing or asking zircd for
//! something. Only counts: no session, channel, message or path. Without it, which wakes
//! are useful can't be measured: the chat analysis of 4 Nov 2026 had to leave it out.
//!
//! What reaches one session within [`TOGETHER`] is one wake, as `zirc watch --once` prints
//! the first line and what comes with it. An intent zircd said for a session by itself
//! (`auto_intents`) is counted apart from one a session said, as `intent` with
//! [`WakeCount::automatic`].
//!
//! Each wake, and then what the session did about it, also goes to the tenant's recorder
//! (`zirc_sidecar::recorder`) as debug activity records ([`zirc_proto::activity::Wake`],
//! [`zirc_proto::activity::WakeOutcome`]): sent to its nick alone, never kept in history,
//! and lost when it isn't there. Metadata only: the line's kind, level and id, never its
//! words. So does each overlap and conflict ahead zircd tells a session of
//! ([`zirc_proto::activity::Notice`]): what it amounts to, never its paths.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use zirc_core::ipc::{Request, Response, WAKE_ACTED_SECS, WakeCount, Why};
use zirc_core::paths;
use zirc_proto::Event;
use zirc_proto::activity::{self, Acted, Noticed, Record};
use zirc_proto::telemetry::OverlapKind;

use super::{Daemon, now, read_saved};

/// Lines within this many seconds of a session's last wake are part of it.
const TOGETHER: u64 = 5;

/// How many days of counts are kept.
const DAYS: usize = 60;

/// The counts, saved, and the wakes not yet acted on, which aren't.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Wakes {
    /// By day (`2026-11-05`), then by reason as it's written (`mention`): woke, acted.
    days: BTreeMap<String, BTreeMap<String, (u64, u64)>>,
    /// By session, its recent wakes: when, the day and the reason, and whether it's been
    /// acted on.
    #[serde(skip)]
    recent: BTreeMap<String, VecDeque<Wake>>,
    /// How many wakes there have been since zircd started, for their ids.
    #[serde(skip)]
    seq: u64,
}

/// One wake of a session's watch.
#[derive(Debug)]
struct Wake {
    id: String,
    at: u64,
    day: String,
    why: String,
    acted: bool,
}

/// What woke a session, as its record says: the line's kind, level and server id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Line {
    pub kind: String,
    pub level: Option<zirc_proto::Level>,
    pub msgid: Option<String>,
}

impl Line {
    /// What `response` was, for the record of a wake on it.
    pub(super) fn of(response: &Response) -> Self {
        match response {
            Response::Delivery(delivery) => Self {
                kind: match &delivery.message.event {
                    Event::Custom { kind, .. } => kind.clone(),
                    event => serde_json::to_value(event)
                        .ok()
                        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned))
                        .unwrap_or_default(),
                },
                level: delivery.message.level(),
                msgid: delivery.id.clone(),
            },
            // A direct message has an id too, so what a session was woken by can be looked
            // up afterwards by whoever may read it; and its kind says whether it was another
            // agent's query, answer or note, which a person's own words are none of.
            Response::Direct(direct) => Self {
                kind: if direct.query.is_some() {
                    "query"
                } else if direct.answer.is_some() {
                    "answer"
                } else if direct.note {
                    "note"
                } else {
                    "direct"
                }
                .into(),
                level: None,
                msgid: direct.id.clone(),
            },
            other => Self {
                kind: serde_json::to_value(other)
                    .ok()
                    .and_then(|v| v.get("kind").and_then(|t| t.as_str()).map(str::to_owned))
                    .unwrap_or_else(|| "other".into()),
                ..Self::default()
            },
        }
    }
}

impl Wakes {
    /// Counts `why` waking `session` at `now`, unless it's part of the wake just before;
    /// `automatic` when what woke it was zircd's own intent. Returns the wake's id when it was
    /// counted.
    fn woke(&mut self, session: &str, why: Why, automatic: bool, now: u64) -> Option<String> {
        let recent = self.recent.entry(session.to_owned()).or_default();
        if recent
            .back()
            .is_some_and(|last| now.saturating_sub(last.at) < TOGETHER)
        {
            return None;
        }
        self.seq += 1;
        let id = format!("w{now:x}{:x}", self.seq);
        let day = day_of(now);
        let why = if automatic {
            format!("{}{AUTOMATIC}", reason(why))
        } else {
            reason(why)
        };
        self.days
            .entry(day.clone())
            .or_default()
            .entry(why.clone())
            .or_default()
            .0 += 1;
        recent.push_back(Wake {
            id: id.clone(),
            at: now,
            day,
            why,
            acted: false,
        });
        while self.days.len() > DAYS {
            self.days.pop_first();
        }
        Some(id)
    }

    /// Counts `session` acting at `now` for each of its wakes in the time before. Returns
    /// each wake it acted on, with how many seconds after.
    fn acted(&mut self, session: &str, now: u64) -> Vec<(String, u64)> {
        let Some(recent) = self.recent.get_mut(session) else {
            return Vec::new();
        };
        let mut acted = Vec::new();
        for wake in recent.iter_mut() {
            if wake.acted || now.saturating_sub(wake.at) > WAKE_ACTED_SECS {
                continue;
            }
            wake.acted = true;
            acted.push((wake.id.clone(), now.saturating_sub(wake.at)));
            if let Some(count) = self
                .days
                .get_mut(&wake.day)
                .and_then(|day| day.get_mut(&wake.why))
            {
                count.1 += 1;
            }
        }
        acted
    }

    /// Lets go of every wake older than the time a session has to act on it, and says which
    /// of them, by session, it never acted on.
    fn expire(&mut self, now: u64) -> Vec<(String, String)> {
        let mut missed = Vec::new();
        for (session, recent) in &mut self.recent {
            while recent
                .front()
                .is_some_and(|wake| now.saturating_sub(wake.at) > WAKE_ACTED_SECS)
            {
                if let Some(wake) = recent.pop_front()
                    && !wake.acted
                {
                    missed.push((session.clone(), wake.id));
                }
            }
        }
        self.recent.retain(|_, recent| !recent.is_empty());
        missed
    }

    /// The counts of the last `days` days to `now`, oldest first.
    fn counts(&self, days: u32, now: u64) -> Vec<WakeCount> {
        let since = day_of(now.saturating_sub(u64::from(days.saturating_sub(1)) * 86_400));
        self.days
            .range(since..)
            .flat_map(|(day, reasons)| {
                reasons.iter().filter_map(|(why, &(woke, acted))| {
                    let (why, automatic) = match why.strip_suffix(AUTOMATIC) {
                        Some(why) => (why, true),
                        None => (why.as_str(), false),
                    };
                    Some(WakeCount {
                        day: day.clone(),
                        why: serde_json::from_value(serde_json::Value::String(why.to_owned()))
                            .ok()?,
                        automatic,
                        woke,
                        acted,
                    })
                })
            })
            .collect()
    }
}

/// How a reason is written when what woke the session was zircd's own intent.
const AUTOMATIC: &str = "/auto";

/// Whether `response` is an intent zircd said for a session by itself, as `auto_intents`
/// words it.
pub(super) fn automatic(response: &Response) -> bool {
    matches!(response, Response::Delivery(delivery)
        if matches!(&delivery.message.event, Event::Intent { summary, .. }
            if summary.ends_with("(said by zircd)")))
}

/// The session `request` is an action of, and how it acted, if it's one: saying something,
/// or asking zircd to do something for it. Reading, watching and status aren't.
pub(super) fn acting(request: &Request) -> Option<(&str, Acted)> {
    match request {
        Request::Post { session, .. }
        | Request::Dm { session, .. }
        | Request::Ask { session, .. }
        | Request::Note { session, .. }
        | Request::AskOwner { session, .. }
        | Request::Reply { session, .. } => Some((session, Acted::Posted)),
        Request::Intend { session, .. } => Some((session, Acted::Intended)),
        Request::React { session, .. } => Some((session, Acted::Reacted)),
        Request::Emit { session, .. }
        | Request::Workflow { session, .. }
        | Request::SetState { session, .. }
        | Request::Lease { session, .. } => Some((session, Acted::Asked)),
        Request::Decide { session, .. } => session.as_deref().map(|s| (s, Acted::Asked)),
        _ => None,
    }
}

/// How a session acted, when what its machine saw it do was a commit or a push of its own.
pub(super) fn acted_by(event: &Event) -> Option<Acted> {
    match event {
        Event::Commit { .. } => Some(Acted::Committed),
        Event::Push { .. } => Some(Acted::Pushed),
        _ => None,
    }
}

/// The UTC day `at` falls on, `2026-11-05`.
fn day_of(at: u64) -> String {
    zirc_core::time::tag(at).chars().take(10).collect()
}

/// `why` as it's written: `mention`.
pub(super) fn reason(why: Why) -> String {
    match serde_json::to_value(why) {
        Ok(serde_json::Value::String(why)) => why,
        _ => format!("{why:?}"),
    }
}

impl Daemon {
    /// Takes in the counts saved before zircd last stopped.
    pub(super) fn restore_wakes(&mut self) {
        if let Some(wakes) = read_saved::<Wakes>(&paths::wakes(&self.org.state)) {
            self.wakes = wakes;
        }
    }

    fn save_wakes(&self) {
        let json = serde_json::to_string(&self.wakes).expect("wakes serialize");
        if let Err(err) = paths::write_private(&paths::wakes(&self.org.state), &json) {
            tracing::warn!("couldn't save the wake counts: {err}");
        }
    }

    /// Counts `why` waking `session`'s watch on `line`, and tells the recorder.
    pub(super) fn woke(&mut self, session: &str, why: Why, automatic: bool, line: Line) {
        let Some(id) = self.wakes.woke(session, why, automatic, now()) else {
            return;
        };
        self.save_wakes();
        let record = activity::Wake {
            id,
            why: reason(why),
            kind: line.kind,
            level: line.level,
            line: line.msgid,
            automatic,
        };
        self.record(session, record.event());
    }

    /// Counts `session` acting, `how`, for the wakes before it, and tells the recorder.
    pub(super) fn session_acted(&mut self, session: &str, how: Acted) {
        let acted = self.wakes.acted(session, now());
        if acted.is_empty() {
            return;
        }
        self.save_wakes();
        for (wake, after) in acted {
            let outcome = activity::WakeOutcome {
                wake,
                acted: how,
                after: Some(after),
            };
            self.record(session, outcome.event());
        }
    }

    /// Tells the recorder of each wake whose session never acted on it in time.
    pub(super) fn wakes_tick(&mut self) {
        for (session, wake) in self.wakes.expire(now()) {
            let outcome = activity::WakeOutcome {
                wake,
                acted: Acted::Nothing,
                after: None,
            };
            self.record(&session, outcome.event());
        }
    }

    /// Sends `event`, an activity record of `session`'s, to the tenant's recorder alone, as a
    /// debug `TAGMSG`: best effort, as it isn't there on every server. Never from a guest's
    /// terminal, which sends nothing outside its guest's room (the gateway refuses it).
    fn record(&self, session: &str, event: Event) {
        let Some(client) = self.client.as_ref().filter(|_| self.org.guest.is_none()) else {
            return;
        };
        let mut message = self.signed(session, event);
        message.task = self.tasks.on(session).cloned();
        if let Some(line) = zirc_core::presence::working_line(activity::RECORDER, &message)
            && let Err(err) = client.send(line)
        {
            tracing::debug!("couldn't tell the recorder: {err}");
        }
    }

    /// Tells the recorder of `alert`, an overlap or a conflict ahead that zircd just told
    /// its watchers of, once for each of this machine's sessions it's between: what it
    /// amounts to (`overlap`, for an overlap), whether it `woke` them and whether it
    /// `escalated`, never its paths. Two sessions here are one record, the first's, naming
    /// the other; nothing when no session is a side of it, as with the person's own trees.
    pub(super) fn noticed(
        &self,
        alert: &Response,
        overlap: Option<OverlapKind>,
        woke: bool,
        escalated: bool,
    ) {
        let (what, sides): (_, Vec<(String, Option<String>)>) = match alert {
            Response::LocalOverlap(local) => {
                let [first, second] = local
                    .worktrees
                    .each_ref()
                    .map(|worktree| self.session_of(worktree).map(str::to_owned));
                let side = match (first, second) {
                    (Some(session), with) => Some((session, with)),
                    (None, Some(session)) => Some((session, None)),
                    (None, None) => None,
                };
                (Noticed::Overlap, side.into_iter().collect())
            }
            Response::Overlap(remote) => {
                let sides = self
                    .local(&remote.channel)
                    .into_iter()
                    .filter(|(_, _, paths)| remote.paths.iter().any(|path| paths.contains(path)))
                    .filter_map(|(_, session, _)| Some((session?, Some(remote.nick.clone()))))
                    .collect();
                (Noticed::Overlap, sides)
            }
            Response::Conflict(conflict) => {
                let side = self
                    .session_of(&conflict.worktree)
                    .map(|session| (session.to_owned(), Some(conflict.nick.clone())));
                (Noticed::ConflictAhead, side.into_iter().collect())
            }
            _ => return,
        };
        for (session, with) in sides {
            let notice = activity::Notice {
                what,
                overlap,
                local: matches!(alert, Response::LocalOverlap(_)),
                woke,
                escalated,
                with,
            };
            self.record(&session, notice.event());
        }
    }

    /// The counts of the last `days` days.
    pub(super) fn wake_counts(&self, days: u32) -> Vec<WakeCount> {
        self.wakes.counts(days, now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-05 10:00 UTC.
    const AT: u64 = 1_793_872_800;

    #[test]
    fn a_direct_message_is_recorded_by_its_id_and_what_it_was() {
        let direct = |extra: serde_json::Value| {
            let mut said = serde_json::json!({"from": "ada", "text": "rebase please", "id": "m1"});
            said.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            Line::of(&Response::Direct(serde_json::from_value(said).unwrap()))
        };
        let line = |kind: &str| Line {
            kind: kind.into(),
            level: None,
            msgid: Some("m1".into()),
        };
        assert_eq!(direct(serde_json::json!({})), line("direct"));
        assert_eq!(direct(serde_json::json!({"query": "q1"})), line("query"));
        assert_eq!(direct(serde_json::json!({"answer": "q1"})), line("answer"));
        assert_eq!(direct(serde_json::json!({"note": true})), line("note"));
    }

    #[test]
    fn counts_a_wake_once_and_whether_it_was_acted_on_in_time() {
        let mut wakes = Wakes::default();
        let first = wakes.woke("cc-1", Why::Mention, false, AT).unwrap();
        // What comes with it is the same wake.
        assert!(wakes.woke("cc-1", Why::Topic, false, AT + 2).is_none());
        let missed = wakes.woke("cc-2", Why::Topic, false, AT + 2).unwrap();
        assert_ne!(first, missed, "each wake has its own id");
        assert_eq!(wakes.acted("cc-1", AT + 60), vec![(first, 60)]);
        // Acting again doesn't count it twice.
        assert!(wakes.acted("cc-1", AT + 70).is_empty());
        assert!(wakes.woke("cc-1", Why::Mention, false, AT + 100).is_some());
        // Too late to be acting on it.
        assert!(wakes.acted("cc-2", AT + 2 + WAKE_ACTED_SECS + 1).is_empty());
        // zircd's own intents apart from sessions'.
        let automatic = wakes.woke("cc-3", Why::Intent, true, AT + 200).unwrap();
        assert_eq!(
            wakes.counts(1, AT + 3600),
            vec![
                WakeCount {
                    day: "2026-11-05".into(),
                    why: Why::Intent,
                    automatic: true,
                    woke: 1,
                    acted: 0,
                },
                WakeCount {
                    day: "2026-11-05".into(),
                    why: Why::Mention,
                    automatic: false,
                    woke: 2,
                    acted: 1,
                },
                WakeCount {
                    day: "2026-11-05".into(),
                    why: Why::Topic,
                    automatic: false,
                    woke: 1,
                    acted: 0,
                },
            ]
        );
        // Past the time to act, the wakes never acted on are said, once each.
        let mut expired = wakes.expire(AT + 200 + WAKE_ACTED_SECS + 1);
        expired.sort();
        assert_eq!(expired.len(), 3, "{expired:?}");
        assert!(expired.contains(&("cc-2".into(), missed)));
        assert!(expired.contains(&("cc-3".into(), automatic)));
        assert!(wakes.expire(AT + 200 + WAKE_ACTED_SECS + 2).is_empty());
        // Saved, the counts come back; the wakes waiting for an action don't.
        let saved: Wakes = serde_json::from_str(&serde_json::to_string(&wakes).unwrap()).unwrap();
        assert_eq!(saved.counts(1, AT + 3600), wakes.counts(1, AT + 3600));
        assert!(saved.recent.is_empty());
        assert!(wakes.counts(1, AT + 2 * 86_400).is_empty());
    }
}
