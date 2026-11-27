//! Incidents: the wakes a coworker had about one thing, drawn as one card in its conversation
//! (the user's choice, 27 Nov 2026, mock `FixerAfter`): the same alert, the same warning or
//! flaky target in CI, or the same issue, however often it woke the coworker, as one card with where it
//! came from (`ALERTS`, `CI`), a short title and detail worked out of the tool's words, how
//! often and when, whether it's still open, and what the coworker said it did about it.
//!
//! A coworker says what it did about a wake in one line tied to it ([`Outcome`]): `zirc role
//! say --about <id>`, which its sidecar says from the coworker's nick in the agents' room,
//! and which closes the item with `--done`. While it has said one, the sidecar wakes it no
//! more for that item while it's open, and says each reminder quietly ([`crate::woke::Woke::quiet`]),
//! which the card counts.
//!
//! Every word a card says is decided here; the apps draw the cards zirc-chat makes of them.

use serde::{Deserialize, Serialize};

use crate::Event;
use crate::card::State;
use crate::role_report::{End, Subject};
use crate::woke::{Tone, Woke};

/// The kind of an outcome's event.
pub const OUTCOME_KIND: &str = "x-zirc/outcome";

/// The most characters a card's title keeps.
pub const TITLE_MAX: usize = 60;

/// The most characters a card's detail keeps.
pub const DETAIL_MAX: usize = 100;

/// The most characters an outcome keeps: one line.
pub const OUTCOME_MAX: usize = 300;

/// What a card's button says while its wakes are shown, to hide them.
pub const HIDE: &str = "Hide the wakes";

/// What a card says of an incident still open.
pub const STILL_OPEN: &str = "Still open";

/// What a coworker said it did about an incident: what an [`Event::Custom`] of
/// [`OUTCOME_KIND`] carries, said from the coworker's nick, its text the line itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Outcome {
    /// The job whose item it's about, such as `fixer`.
    pub role: String,
    /// The incident's key ([`key`]): an item's id, as the wake's line showed it in brackets.
    pub about: String,
    /// Whether it's done: the item closes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
}

impl Outcome {
    /// Its event, `text` being the line.
    ///
    /// # Panics
    /// Never: an outcome always serialises.
    #[must_use]
    pub fn event(&self, text: &str) -> Event {
        Event::Custom {
            kind: OUTCOME_KIND.to_owned(),
            data: serde_json::to_value(self).expect("an outcome serialises"),
            text: crate::clip_line(text, OUTCOME_MAX),
        }
    }

    /// The outcome `event` says, if it's one, with its line.
    #[must_use]
    pub fn of(event: &Event) -> Option<(Self, String)> {
        match event {
            Event::Custom { kind, data, text } if kind == OUTCOME_KIND => {
                let outcome: Self = serde_json::from_value(data.clone()).ok()?;
                Some((outcome, text.clone()))
            }
            _ => None,
        }
    }
}

/// Whether `kind` is an outcome's: it wakes no watch, and a room keeps it to Everything.
#[must_use]
pub fn is_kind(kind: &str) -> bool {
    kind == OUTCOME_KIND
}

/// The incident `woke` is about, by key: the item a report opened
/// ([`crate::role_report::OpenItem::id`]), whatever reported it: an alert, a warning or a
/// flaky test in CI, a tool's error or issue. `None` for any other wake, such as a build's,
/// which stays a line of its own.
#[must_use]
pub fn key(woke: &Woke) -> Option<String> {
    woke.report.as_ref().map(|reported| reported.id.clone())
}

/// What a card heads an incident with, from its first wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Where it came from, as its chip says it: `ALERTS`, `CI`, `SENTRY`.
    pub source: String,
    /// The chip's colour.
    pub tone: Tone,
    /// What it is, short: `web pod is failing`.
    pub title: String,
    /// What the tool said, cut to its gist: `Build option android_platforms has changed,
    /// discarding analysis cache`.
    pub detail: Option<String>,
    /// What names it, short, in monospace: `web-7ff4…`, `#412`.
    pub reference: Option<String>,
}

/// What a card heads `woke`'s incident with, `None` for a wake that's no incident ([`key`]).
#[must_use]
pub fn head(woke: &Woke) -> Option<Head> {
    let tone = woke.tone();
    let reported = woke.report.as_ref()?;
    let report = &reported.report;
    let source = report
        .tool
        .map_or_else(|| report.kind.found_by(), |tool| tool.name())
        .to_uppercase();
    let summary = gist(&report.summary);
    let details = report.details.as_deref().map(gist);
    let (title, detail, reference) = match report.kind {
        Subject::Container => {
            let name = report.key.rsplit('/').next().unwrap_or(&report.key);
            let (pod, reason) = report
                .summary
                .split_once("'s pod ")
                .and_then(|(_, rest)| rest.split_once(": "))
                .map_or((None, report.summary.as_str()), |(pod, reason)| {
                    (Some(short_pod(pod)), reason)
                });
            let title = match reason.trim() {
                "OOMKilled" => format!("{name} pod ran out of memory"),
                "NotReady" => format!("{name} isn't ready"),
                _ => format!("{name} pod is failing"),
            };
            (title, details.or_else(|| Some(gist(reason))), pod)
        }
        Subject::Warning => {
            let title = match &report.build {
                Some(build) => format!("New warning in build {build}"),
                None => "New warning".to_owned(),
            };
            (title, Some(summary), None)
        }
        Subject::Flaky => {
            let detail = report
                .build
                .as_ref()
                .map(|build| format!("In build {build}"));
            ("Flaky test".to_owned(), detail, Some(report.key.clone()))
        }
        Subject::Error | Subject::Issue | Subject::Incident => {
            (summary, details, report.reference.clone())
        }
    };
    Some(Head {
        source,
        tone,
        title: crate::clip(&capitalised(&title), TITLE_MAX),
        detail: detail
            .filter(|detail| !detail.is_empty())
            .map(|detail| crate::clip(&capitalised(&detail), DETAIL_MAX)),
        reference,
    })
}

/// The incidents a wake says closed, by key: a landing's items (`Fixes: report <id>`).
#[must_use]
pub fn closes(woke: &Woke) -> &[String] {
    if woke.landing && woke.state == Some(State::Passed) {
        &woke.items
    } else {
        &[]
    }
}

/// `text` cut to its gist, as a card says what a tool said: a level's prefix (`WARNING:`),
/// web addresses and an aside naming one gone, a Bazel flag or label by its last name
/// (`--@@rules_android+//flags:android_platforms` → `android_platforms`) and a pod's
/// generated name shortened (`web-7ff48bd49f-qgfqb` → `web-7ff4…`), on one line.
#[must_use]
pub fn gist(text: &str) -> String {
    let mut text = crate::clip_line(text, usize::MAX);
    for prefix in ["warning:", "error:", "info:", "debug:", "notice:"] {
        if text
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        {
            text = text[prefix.len()..].trim_start().to_owned();
        }
    }
    // An aside that points somewhere, `(this can be expensive, see https://…)`, closed or cut.
    while let Some(open) = text.find('(') {
        let close = text[open..]
            .find(')')
            .map_or(text.len(), |at| open + at + 1);
        if !text[open..close].contains("://") {
            break;
        }
        text.replace_range(open..close, "");
    }
    let words: Vec<String> = text
        .split_whitespace()
        .filter(|word| !word.contains("://"))
        .map(|word| {
            let (core, tail) = split_punctuation(word);
            format!("{}{tail}", short_word(core))
        })
        .collect();
    words
        .join(" ")
        .replace(" .", ".")
        .replace(" ,", ",")
        .replace(" ;", ";")
        .trim_end_matches([',', ';', ':', ' '])
        .to_owned()
}

/// `word` and the punctuation it ends with, apart.
fn split_punctuation(word: &str) -> (&str, &str) {
    let core = word.trim_end_matches([',', ';', '.', ':', ')']);
    (core, &word[core.len()..])
}

/// A Bazel flag or label by its last name, a pod's name shortened, anything else as it is.
fn short_word(word: &str) -> String {
    if word.contains("//") && word.starts_with("--") {
        let name = word.rsplit([':', '/']).next().unwrap_or(word);
        return name.to_owned();
    }
    short_pod(word)
}

/// A pod's name as a card shows it: its workload's name and the first four of its
/// replica set's hash, `web-7ff4…`; any other name as it is.
#[must_use]
pub fn short_pod(name: &str) -> String {
    let parts: Vec<&str> = name.rsplitn(3, '-').collect();
    if let [suffix, hash, base] = parts[..]
        && suffix.len() == 5
        && (8..=10).contains(&hash.len())
        && [suffix, hash].iter().all(|part| {
            part.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
        && hash.bytes().any(|b| b.is_ascii_digit())
        && !base.is_empty()
    {
        return format!("{base}-{}…", &hash[..4]);
    }
    name.to_owned()
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `n` times as a card says it: `once`, `twice`, `4 times`.
#[must_use]
pub fn times(n: usize) -> String {
    match n {
        1 => "once".to_owned(),
        2 => "twice".to_owned(),
        n => format!("{n} times"),
    }
}

/// One wake of an incident, or a quiet repeat of it, as its card keeps it for showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// When, in milliseconds since 1970.
    pub at_ms: i64,
    /// The wake's words whole, as its line said them.
    pub words: String,
    /// Whether it was said without waking anyone ([`Woke::quiet`]).
    pub quiet: bool,
}

/// One incident: its wakes about one thing, how it ended and what its coworker said of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    /// Its key ([`key`]).
    pub key: String,
    /// What heads its card, from its first wake.
    pub head: Head,
    /// Its wakes, oldest first.
    pub seen: Vec<Seen>,
    /// How it ended, once it did.
    pub end: Option<End>,
    /// The coworker's latest word on it, and when.
    pub outcome: Option<(i64, String)>,
}

impl Incident {
    /// When its first wake was.
    #[must_use]
    pub fn first_ms(&self) -> i64 {
        self.seen.first().map_or(0, |seen| seen.at_ms)
    }

    /// What its card says of how it stands, `who` being the coworker as the reader knows
    /// it and `clock` how the reader's clock says a time: `Still open`, `Still open · woke
    /// fixer 4 times, last 20:41`, `Fixed · woke fixer twice, last 20:41`, `Still open · 5
    /// times, woke fixer once, last 21:30`. How it ended goes unsaid where the coworker's
    /// line says it in its pill ([`Incident::pill`]), so `None` for one ended that woke once.
    #[must_use]
    pub fn status(&self, who: &str, clock: impl Fn(i64) -> String) -> Option<String> {
        let state = match self.end {
            None => Some(STILL_OPEN.to_owned()),
            Some(_) if self.outcome.is_some() => None,
            Some(end) => Some(capitalised(end.words())),
        };
        let all = self.seen.len();
        let woke = self.seen.iter().filter(|seen| !seen.quiet).count();
        let last = self
            .seen
            .last()
            .map(|seen| clock(seen.at_ms))
            .unwrap_or_default();
        let counts = match (all, woke) {
            (0 | 1, _) => None,
            (all, woke) if all == woke => Some(format!("woke {who} {}, last {last}", times(woke))),
            (all, woke) => Some(format!(
                "{all} times, woke {who} {}, last {last}",
                times(woke)
            )),
        };
        let said: Vec<String> = [state, counts].into_iter().flatten().collect();
        (!said.is_empty()).then(|| said.join(" · "))
    }

    /// What its button says to show its wakes: `Show the wake`, `Show the 4 wakes`.
    #[must_use]
    pub fn show(&self) -> String {
        match self.seen.len() {
            1 => "Show the wake".to_owned(),
            n => format!("Show the {n} wakes"),
        }
    }

    /// What the coworker's line on it says after it, in a pill: how it ended, `Done`, once
    /// it did.
    #[must_use]
    pub fn pill(&self) -> Option<String> {
        self.end.map(|end| capitalised(end.words()))
    }
}

/// A conversation's incidents as they're read in, in time order.
#[derive(Debug, Clone, Default)]
pub struct Incidents {
    /// Every incident, by its first wake.
    pub all: Vec<Incident>,
}

impl Incidents {
    /// The incident `key` names.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Incident> {
        self.all.iter().find(|incident| incident.key == key)
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut Incident> {
        self.all.iter_mut().find(|incident| incident.key == key)
    }

    /// Takes in `woke`, said at `at_ms` in `words`: it opens an incident or adds to its
    /// open one, closes those it says closed, or is none's. The key of the incident it went
    /// to, when it went to one: its line is then the card's.
    pub fn woke(&mut self, at_ms: i64, woke: &Woke, words: &str) -> Option<String> {
        for key in closes(woke) {
            if let Some(incident) = self.get_mut(key).filter(|incident| incident.end.is_none()) {
                incident.end = Some(End::Landed);
            }
        }
        let key = key(woke)?;
        let seen = Seen {
            at_ms,
            words: words.to_owned(),
            quiet: woke.quiet,
        };
        if let Some(incident) = self.get_mut(&key).filter(|incident| incident.end.is_none()) {
            incident.seen.push(seen);
        } else {
            // One that closed and came again is an incident of its own.
            let head = head(woke)?;
            self.all.push(Incident {
                key: key.clone(),
                head,
                seen: vec![seen],
                end: None,
                outcome: None,
            });
        }
        Some(key)
    }

    /// Takes in `outcome`, the line `text` said at `at_ms`: the latest incident it's about
    /// has it, and closes when it's done. Whether one did.
    pub fn outcome(&mut self, at_ms: i64, outcome: &Outcome, text: &str) -> bool {
        let Some(incident) = self
            .all
            .iter_mut()
            .rev()
            .find(|incident| incident.key == outcome.about)
        else {
            return false;
        };
        incident.outcome = Some((at_ms, text.to_owned()));
        if outcome.done && incident.end.is_none() {
            incident.end = Some(End::Done);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_report::{OpenItem, RoleReport};
    use crate::woke::{By, What};

    fn pod() -> OpenItem {
        OpenItem::opened(
            RoleReport {
                summary: "web/web's pod web-7ff48bd49f-qgfqb: Error".into(),
                ..RoleReport::new("fixer", Subject::Container, "web/web")
            },
            0,
        )
    }

    fn warning() -> OpenItem {
        OpenItem::opened(
            RoleReport {
                build: Some("10819".into()),
                ..RoleReport::new(
                    "fixer",
                    Subject::Warning,
                    "WARNING: Build option --@@rules_android+//flags:android_platforms has changed, discarding analysis cache (this can be expensive, see https://bazel.build/advance",
                )
            },
            0,
        )
    }

    #[test]
    fn a_pod_alert_is_headed_by_its_workload_and_short_pod() {
        let head = head(&Woke::reported(&pod(), false)).unwrap();
        assert_eq!(head.source, "ALERTS");
        assert_eq!(head.title, "Web pod is failing");
        assert_eq!(head.reference.as_deref(), Some("web-7ff4…"));
        assert_eq!(head.detail.as_deref(), Some("Error"));
    }

    #[test]
    fn a_long_bazel_warning_is_cut_to_its_gist() {
        let head = head(&Woke::reported(&warning(), false)).unwrap();
        assert_eq!(head.source, "CI");
        assert_eq!(head.title, "New warning in build 10819");
        assert_eq!(
            head.detail.as_deref(),
            Some("Build option android_platforms has changed, discarding analysis cache")
        );
    }

    #[test]
    fn gist_keeps_ordinary_words_and_names() {
        assert_eq!(gist("unused import `N`"), "unused import `N`");
        assert_eq!(gist("see https://a.b/c for more."), "see for more.");
        assert_eq!(gist("cache (see https://x.y/z)."), "cache.");
        assert_eq!(short_pod("ergo-0"), "ergo-0");
        assert_eq!(short_pod("my-app-5d4f8b7c9d-x2k9p"), "my-app-5d4f…");
    }

    #[test]
    fn wakes_about_one_item_are_one_incident_counted_with_one_time() {
        let mut incidents = Incidents::default();
        let item = pod();
        let words = |reminder| Woke::reported(&item, reminder).words(None);
        let at = |minutes: i64| minutes * 60_000;
        assert!(
            incidents
                .woke(at(0), &Woke::reported(&item, false), &words(false))
                .is_some()
        );
        let other = warning();
        incidents.woke(at(1), &Woke::reported(&other, false), "w");
        for minute in [3, 5, 8] {
            incidents.woke(at(minute), &Woke::reported(&item, true), &words(true));
        }
        assert_eq!(incidents.all.len(), 2);
        let incident = incidents.get(&item.id).unwrap();
        assert_eq!(incident.seen.len(), 4);
        assert_eq!(incident.first_ms(), 0);
        let clock = |ms: i64| format!("20:{:02}", 33 + ms / 60_000);
        assert_eq!(
            incident.status("fixer", clock).as_deref(),
            Some("Still open · woke fixer 4 times, last 20:41")
        );
        assert_eq!(incident.show(), "Show the 4 wakes");
        assert!(incident.seen[1].words.starts_with("still open: "));
        assert_eq!(
            incidents
                .get(&other.id)
                .unwrap()
                .status("fixer", clock)
                .as_deref(),
            Some(STILL_OPEN)
        );
    }

    #[test]
    fn a_quiet_repeat_counts_but_didnt_wake() {
        let mut incidents = Incidents::default();
        let item = pod();
        incidents.woke(0, &Woke::reported(&item, false), "a");
        for at in [1, 2] {
            let quiet = Woke {
                quiet: true,
                ..Woke::reported(&item, true)
            };
            incidents.woke(at * 60_000, &quiet, "b");
        }
        let status = incidents
            .get(&item.id)
            .unwrap()
            .status("fixer", |_| "20:35".into());
        assert_eq!(
            status.as_deref(),
            Some("Still open · 3 times, woke fixer once, last 20:35")
        );
    }

    #[test]
    fn an_outcome_goes_on_its_incident_and_done_closes_it() {
        let mut incidents = Incidents::default();
        let item = warning();
        incidents.woke(0, &Woke::reported(&item, false), "w");
        let outcome = Outcome {
            role: "fixer".into(),
            about: item.id.clone(),
            done: true,
        };
        let (read, text) = Outcome::of(&outcome.event("Not a failure.")).unwrap();
        assert!(incidents.outcome(5, &read, &text));
        let incident = incidents.get(&item.id).unwrap();
        assert_eq!(incident.outcome, Some((5, "Not a failure.".into())));
        assert_eq!(incident.pill().as_deref(), Some("Done"));
        // Said once, in the pill.
        assert_eq!(incident.status("fixer", |_| String::new()), None);
        assert!(!incidents.outcome(
            6,
            &Outcome {
                about: "nope".into(),
                ..outcome
            },
            "x"
        ));
    }

    #[test]
    fn a_landing_closes_its_items_and_one_reported_again_is_new() {
        let mut incidents = Incidents::default();
        let item = pod();
        incidents.woke(0, &Woke::reported(&item, false), "a");
        let landed = Woke {
            items: vec![item.id.clone()],
            ..Woke::closed_by_landing("fixer", Some("613".into()), Some("fix".into()), 1)
        };
        assert_eq!(incidents.woke(1, &landed, "landed"), None);
        assert_eq!(incidents.get(&item.id).unwrap().end, Some(End::Landed));
        incidents.woke(2, &Woke::reported(&item, false), "again");
        assert_eq!(incidents.all.len(), 2);
        assert_eq!(incidents.all[1].end, None);
    }

    #[test]
    fn only_a_report_is_an_incident() {
        let build = Woke {
            build: Some("612".into()),
            state: Some(State::Failed),
            ..Woke::new(What::Build, By::Ci)
        };
        assert_eq!(key(&build), None);
        assert_eq!(key(&Woke::new(What::Wrote, By::Person)), None);
    }
}
