//! What wakes something on its own, said once for whatever it wakes: a state ([`crate::state`])
//! being said, a time coming round (`every`), words someone says (`said`), a person's
//! reaction to a message (`reaction`), or a landing that changes some paths (`landed`).
//! Workflows started runs on them; a role ([`crate::roles`]) wakes its holder on them. What
//! a trigger is, the rules it keeps ([`Trigger::check`]), whether something that happened
//! matches it ([`Trigger::matches`]), when a time comes round ([`Schedule`], [`came_round`])
//! and how people read it ([`Trigger::words`]) are all here, so the sidecar, zirc.dev and
//! the apps never decide them twice.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::state::Level;

/// What starts a run on its own, each trigger one of: a state ([`crate::state`]) being said, a
/// time coming round (`every`), words someone says where the workflow listens (`said`), a
/// person's reaction to a message there (`reaction`), or a landing that changes some paths
/// (`landed`). What it knows of that fills the run's inputs (`inputs`), but a time, which
/// knows nothing and is given them (`with`).
///
/// ```toml
/// [[on]]
/// state = "master"
/// level = "broken"
/// inputs = { owner = "owner" }
///
/// [[on]]
/// every = "7d"          # at least [`MIN_EVERY`]
/// day = "mon"           # with 7d alone; Thursdays without it
/// at = "09:00"          # UTC, when `every` is whole days; midnight without it
/// with = { what = "update dependencies" }
///
/// [[on]]
/// said = "refund"       # a person or an integration says it, as words of its own
/// inputs = { ask = "text", who = "by" }   # of [`SAID_FIELDS`]
///
/// [[on]]
/// reaction = "🐛"        # a person's, on a message the workflow heard
/// inputs = { report = "text", from = "author" }   # of [`REACTION_FIELDS`]
///
/// [[on]]
/// landed = ["crates/zirc", "MODULE.bazel"]   # files, or everything under a directory
/// inputs = { commit = "commit", by = "session" } # of [`LANDED_FIELDS`]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Trigger {
    /// The state's name, such as `master`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The level that starts a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    /// Which input each thing the trigger knows fills: `input = "field"`, where the field
    /// is one its kind has ([`TriggerKind::fields`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// Words that start a run when a person or an integration says them where the workflow
    /// listens, whatever their case, as words of their own: `refund` isn't in `refunded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// The reaction that starts a run when a person gives it to a message the workflow
    /// heard, such as `🐛`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<String>,
    /// Paths a landing changes to start a run: a file, or a directory for everything under
    /// it; `.` for any landing. As CI says what a landing changed ([`crate::ci::Data`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub landed: Vec<String>,
    /// How often a run starts by itself, such as `1d` ([`seconds`]): at every whole multiple
    /// of it since 1 January 1970 UTC, so `1d` is each midnight and `6h` is 0, 6, 12 and 18.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// The time of day it starts at, `HH:MM` in UTC, when `every` is whole days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The day of the week it starts on, `mon` to `sun`, when `every` is `7d`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    /// What a run started by `every` is given: `input = "value"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub with: BTreeMap<String, String>,
}

/// What a trigger is, by which of its fields it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// `state`.
    State,
    /// `every`.
    Every,
    /// `said`.
    Said,
    /// `reaction`.
    Reaction,
    /// `landed`.
    Landed,
}

/// What a `said` trigger can pass on as inputs: the message's text, who said it, where, and
/// its id on the server.
pub const SAID_FIELDS: [&str; 4] = ["text", "by", "channel", "id"];

/// What a `reaction` trigger can pass on: the message's text, who wrote it, who reacted,
/// where, and the message's id.
pub const REACTION_FIELDS: [&str; 5] = ["text", "author", "by", "channel", "id"];

/// What a `landed` trigger can pass on: the landing's commit, branch, the session that
/// queued it, its build, the build's page and the commit's subject.
pub const LANDED_FIELDS: [&str; 6] = ["commit", "branch", "session", "build", "link", "subject"];

/// The longest `said` is, in characters.
pub const MAX_SAID: usize = 80;

/// The most paths `landed` names.
pub const MAX_LANDED: usize = 16;

impl TriggerKind {
    /// The fields its `inputs` may read; none for a time, which is given `with`.
    #[must_use]
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Self::State => &STATE_FIELDS,
            Self::Every => &[],
            Self::Said => &SAID_FIELDS,
            Self::Reaction => &REACTION_FIELDS,
            Self::Landed => &LANDED_FIELDS,
        }
    }

    /// The field of a trigger that makes it this kind, as a file writes it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Every => "every",
            Self::Said => "said",
            Self::Reaction => "reaction",
            Self::Landed => "landed",
        }
    }
}

/// Whether a landing that changed `changed` starts a run of a trigger on `paths`: a changed
/// file is one of them or under one; `.` is any landing. A changed path ending in `/` is a
/// directory CI named in place of the many files it changed there: anything in it may have.
#[must_use]
pub fn landed_under(paths: &[String], changed: &[String]) -> bool {
    let under = |path: &str, dir: &str| {
        path.strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || dir.ends_with('/'))
    };
    paths.iter().any(|path| {
        let path = path.trim_end_matches('/');
        path == "."
            || changed.iter().any(|c| {
                under(c.trim_end_matches('/'), path) || (c.ends_with('/') && under(path, c))
            })
    })
}

/// The shortest `every` may be, in seconds: a workflow isn't a clock.
pub const MIN_EVERY: u64 = 300;

/// How long after its time a run `every` starts may still start, in seconds, as when the
/// server was restarting then: later, that time is passed over.
pub const EVERY_GRACE: u64 = 3600;

/// The days `day` may name, Monday first.
pub const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A day, in seconds.
pub const DAY: u64 = 86_400;

/// When a trigger's `every` comes round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    /// How often, in seconds.
    pub every: u64,
    /// How long after each whole multiple of `every` since 1970, in seconds: its `at`, and
    /// its `day`.
    pub offset: u64,
}

impl Schedule {
    /// The time it last came round, at or before `now` (unix seconds); none before the first.
    #[must_use]
    pub fn last(&self, now: u64) -> Option<u64> {
        let since = now.checked_sub(self.offset)?;
        Some(since - since % self.every + self.offset)
    }

    /// The time it next comes round, after `now`.
    #[must_use]
    pub fn next(&self, now: u64) -> u64 {
        self.last(now).map_or(self.offset, |last| last + self.every)
    }
}

impl Trigger {
    /// Which kind it is.
    ///
    /// # Errors
    ///
    /// When it has none, or more than one, of the fields that say.
    pub fn kind(&self) -> Result<TriggerKind, String> {
        let has = [
            (self.state.is_some(), TriggerKind::State),
            (self.every.is_some(), TriggerKind::Every),
            (self.said.is_some(), TriggerKind::Said),
            (self.reaction.is_some(), TriggerKind::Reaction),
            (!self.landed.is_empty(), TriggerKind::Landed),
        ];
        let mut kinds = has.iter().filter(|(has, _)| *has).map(|(_, kind)| *kind);
        match (kinds.next(), kinds.next()) {
            (Some(kind), None) => Ok(kind),
            _ => Err("a trigger has one of state, every, said, reaction and landed".to_owned()),
        }
    }

    /// When it comes round, if it's one that does (`every`).
    ///
    /// # Errors
    ///
    /// What's wrong with its `every`, `at` or `day`.
    pub fn schedule(&self) -> Result<Option<Schedule>, String> {
        let Some(every) = &self.every else {
            return Ok(None);
        };
        let every = seconds(every)?;
        if every < MIN_EVERY {
            return Err(format!("every is at least {MIN_EVERY}s"));
        }
        let mut offset = 0;
        if let Some(at) = &self.at {
            let bad = || format!("at {at:?} isn't a time of day like 09:00");
            if every % DAY != 0 {
                return Err("at needs an every of whole days".to_owned());
            }
            let (hours, minutes) = at.split_once(':').ok_or_else(bad)?;
            let part = |text: &str, below: u64| {
                (text.len() == 2)
                    .then(|| text.parse::<u64>().ok())
                    .flatten()
                    .filter(|n| *n < below)
            };
            offset +=
                part(hours, 24).ok_or_else(bad)? * 3600 + part(minutes, 60).ok_or_else(bad)? * 60;
        }
        if let Some(day) = &self.day {
            if every != 7 * DAY {
                return Err("day needs an every of 7d".to_owned());
            }
            let index = DAYS
                .iter()
                .position(|d| d == day)
                .ok_or_else(|| format!("day {day:?} is none of {}", DAYS.join(", ")))?;
            // 1 January 1970 was a Thursday.
            offset += (index as u64 + 7 - 3) % 7 * DAY;
        }
        Ok(Some(Schedule { every, offset }))
    }
}

/// The fields of a state a trigger can pass on as inputs.
pub const STATE_FIELDS: [&str; 5] = ["repo", "name", "why", "owner", "link"];

/// How many seconds `text` says, as every length of time is written ([`crate::duration`]),
/// such as `30m`: what a trigger's `every` and a workflow's waits take.
///
/// # Errors
///
/// When it isn't one, or is 0.
pub fn seconds(text: &str) -> Result<u64, String> {
    crate::duration(text)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| format!("{text:?} isn't a time like 90s, 30m, 2h, 2d or 1w"))
}

/// Something that happened, which a trigger may wake on. A time coming round is
/// [`came_round`]'s, as it happens without anyone saying so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Happened<'a> {
    /// The state `name` was said to be at `level`.
    State {
        /// The state, such as `master`.
        name: &'a str,
        /// How it stands.
        level: Level,
    },
    /// A person or an integration said `text`.
    Said {
        /// What they said.
        text: &'a str,
    },
    /// A person reacted with `reaction`.
    Reaction {
        /// The reaction, such as `🐛`.
        reaction: &'a str,
    },
    /// A landing changed `changed`, as CI says it ([`crate::ci::Data`]).
    Landed {
        /// The paths it changed.
        changed: &'a [String],
    },
}

/// Whether `id` stands in `said` as a word of its own, not inside a longer name: `fix-flaky-7`
/// names `flaky-7`, and `flaky-17` doesn't.
#[must_use]
pub fn names_whole(said: &str, id: &str) -> bool {
    let apart = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    said.match_indices(id).any(|(at, _)| {
        apart(said[..at].chars().next_back()) && apart(said[at + id.len()..].chars().next())
    })
}

/// Whether a time `schedule` comes round on, last dealt with at `dealt`, has come round
/// again by `now`: the time to note as dealt with, and whether to act on it, which a time
/// more than [`EVERY_GRACE`] ago isn't, as when the server was away then: passed over, never
/// made up for. `None` when nothing has come round since.
#[must_use]
pub fn came_round(schedule: Schedule, dealt: Option<u64>, now: u64) -> Option<(u64, bool)> {
    let at = schedule.last(now)?;
    if dealt.is_some_and(|dealt| dealt >= at) {
        return None;
    }
    Some((at, now - at <= EVERY_GRACE))
}

impl Trigger {
    /// The rules a trigger keeps that need nothing but itself: one kind, with only what its
    /// kind has, each field as it may be. Its kind.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for whoever wrote it.
    pub fn check(&self) -> Result<TriggerKind, String> {
        let kind = self.kind()?;
        let word = kind.word();
        self.schedule().map_err(|e| format!("a trigger: {e}"))?;
        if kind != TriggerKind::Every
            && (!self.with.is_empty() || self.at.is_some() || self.day.is_some())
        {
            return Err(format!(
                "the trigger on {word} has with, at or day, which only every has"
            ));
        }
        if (kind != TriggerKind::State && self.level.is_some())
            || (kind == TriggerKind::Every && !self.inputs.is_empty())
        {
            return Err(format!(
                "a trigger with {word} has a level or inputs, which only a state has: \
                 give a time's inputs as with"
            ));
        }
        match kind {
            TriggerKind::State => {
                let state = self.state.as_deref().unwrap_or_default();
                if let Some(why) = crate::state::check_name(state) {
                    return Err(why.to_owned());
                }
                if self.level.is_none() {
                    return Err(format!("the trigger on {state:?} needs a level"));
                }
            }
            TriggerKind::Said => {
                let said = self.said.as_deref().unwrap_or_default();
                if said.trim().is_empty() || said.chars().count() > MAX_SAID || said.contains('\n')
                {
                    return Err(format!(
                        "said is some words on a line, {MAX_SAID} characters at most"
                    ));
                }
            }
            TriggerKind::Reaction => {
                let reaction = self.reaction.as_deref().unwrap_or_default();
                if reaction.is_empty() || reaction.chars().count() > 8 || reaction.contains(' ') {
                    return Err(format!("reaction {reaction:?} isn't one reaction"));
                }
                if ["👍", "👎"].contains(&reaction) {
                    return Err("👍 and 👎 answer an approval: choose another reaction".to_owned());
                }
            }
            TriggerKind::Landed => {
                if self.landed.len() > MAX_LANDED {
                    return Err(format!("landed names more than {MAX_LANDED} paths"));
                }
                if let Some(bad) = self.landed.iter().find(|p| {
                    p.is_empty()
                        || p.starts_with('/')
                        || p.contains(['*', '\n'])
                        || p.contains("..")
                }) {
                    return Err(format!(
                        "landed names {bad:?}, which isn't a file or directory from the \
                         repository's root (no patterns: a directory is everything under it)"
                    ));
                }
            }
            TriggerKind::Every => {}
        }
        Ok(kind)
    }

    /// Whether `happened` is what it wakes on. A time coming round is [`came_round`]'s.
    #[must_use]
    pub fn matches(&self, happened: &Happened<'_>) -> bool {
        match *happened {
            Happened::State { name, level } => {
                self.state.as_deref() == Some(name) && self.level == Some(level)
            }
            Happened::Said { text } => self
                .said
                .as_deref()
                .is_some_and(|said| names_whole(&text.to_lowercase(), &said.to_lowercase())),
            Happened::Reaction { reaction } => self.reaction.as_deref() == Some(reaction),
            Happened::Landed { changed } => {
                !self.landed.is_empty() && landed_under(&self.landed, changed)
            }
        }
    }

    /// What it wakes on, as people read it after "Wakes on" or "Starts when": `master is
    /// broken`, `someone says "refund"`, `someone reacts 🐛`, `a landing changes server/`,
    /// `every day at 09:00 UTC`. [`WORDS`] filled in.
    #[must_use]
    pub fn words(&self) -> String {
        let Ok(kind) = self.kind() else {
            return String::new();
        };
        let template = WORDS
            .iter()
            .find(|(word, _)| *word == kind.word())
            .map_or("", |(_, template)| *template);
        let every = self
            .every
            .as_deref()
            .map(duration_words)
            .unwrap_or_default();
        let every = every
            .strip_prefix("a ")
            .or_else(|| every.strip_prefix("an "))
            .unwrap_or(&every);
        let mut words = template
            .replace("{state}", self.state.as_deref().unwrap_or_default())
            .replace("{level}", self.level.map_or("", Level::word))
            .replace("{said}", self.said.as_deref().unwrap_or_default())
            .replace("{reaction}", self.reaction.as_deref().unwrap_or_default())
            .replace("{landed}", &self.landed.join(", "))
            .replace("{every}", every);
        if let Some(day) = &self.day {
            words += &EVERY_DAY.replace("{day}", day_name(day));
        }
        if let Some(at) = &self.at {
            words += &EVERY_AT.replace("{at}", at);
        }
        words
    }
}

/// How each kind of trigger is said ([`Trigger::words`]), by its word ([`TriggerKind::word`]),
/// `{field}` standing for its field's value: said here once, and to readers that can't run
/// this crate as TypeScript ([`typescript`]).
pub const WORDS: [(&str, &str); 5] = [
    ("state", "{state} is {level}"),
    ("every", "every {every}"),
    ("said", "someone says \"{said}\""),
    ("reaction", "someone reacts {reaction}"),
    ("landed", "a landing changes {landed}"),
];

/// What an `every` with a `day` adds to its words.
pub const EVERY_DAY: &str = " on {day}";

/// What an `every` with an `at` adds to its words.
pub const EVERY_AT: &str = " at {at} UTC";

/// Each unit a length of time is written in ([`crate::duration`]), with its name.
pub const UNITS: [(char, &str); 5] = [
    ('s', "second"),
    ('m', "minute"),
    ('h', "hour"),
    ('d', "day"),
    ('w', "week"),
];

/// The days [`DAYS`] name, in full.
pub const DAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// A duration as a trigger writes one (`15m`, `1d`), in words: `15 minutes`, `a day`; as
/// written when it isn't one.
#[must_use]
pub fn duration_words(text: &str) -> String {
    let text = text.trim();
    let Some(unit) = text.chars().last() else {
        return String::new();
    };
    let count = &text[..text.len() - unit.len_utf8()];
    let Some((_, name)) = UNITS.iter().find(|(u, _)| *u == unit) else {
        return text.to_owned();
    };
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
        return text.to_owned();
    }
    if count == "1" {
        let article = if unit == 'h' { "an" } else { "a" };
        format!("{article} {name}")
    } else {
        format!("{count} {name}s")
    }
}

/// The day `day` names ([`DAYS`]) in full: `Monday` for `mon`; as written when it's none.
fn day_name(day: &str) -> &str {
    DAYS.iter()
        .position(|d| *d == day)
        .map_or(day, |i| DAY_NAMES[i])
}

/// The TypeScript readers that can't run this crate say triggers with: [`Trigger`], and
/// [`WORDS`] with what fills them.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    let json = |value: serde_json::Value| value.to_string();
    vec![
        Trigger::decl(config),
        format!(
            "const TRIGGER_WORDS: Record<string, string> = {};",
            json(
                WORDS
                    .iter()
                    .map(|(k, v)| (k.to_string(), serde_json::Value::from(*v)))
                    .collect::<serde_json::Map<_, _>>()
                    .into()
            )
        ),
        format!("const TRIGGER_EVERY_DAY = {EVERY_DAY:?};"),
        format!("const TRIGGER_EVERY_AT = {EVERY_AT:?};"),
        format!(
            "const DURATION_UNITS: Record<string, string> = {};",
            json(
                UNITS
                    .iter()
                    .map(|(k, v)| (k.to_string(), serde_json::Value::from(*v)))
                    .collect::<serde_json::Map<_, _>>()
                    .into()
            )
        ),
        format!("const TRIGGER_DAYS = {};", json(DAYS.into())),
        format!("const DAY_NAMES = {};", json(DAY_NAMES.into())),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(toml_like: serde_json::Value) -> Trigger {
        serde_json::from_value(toml_like).unwrap()
    }

    #[test]
    fn matches_what_happened_of_its_kind() {
        let broken = trigger(serde_json::json!({"state": "master", "level": "broken"}));
        assert!(broken.matches(&Happened::State {
            name: "master",
            level: Level::Broken
        }));
        assert!(!broken.matches(&Happened::State {
            name: "master",
            level: Level::Ok
        }));
        let said = trigger(serde_json::json!({"said": "Refund"}));
        assert!(said.matches(&Happened::Said {
            text: "a refund please"
        }));
        assert!(!said.matches(&Happened::Said { text: "refunded" }));
        let landed = trigger(serde_json::json!({"landed": ["server"]}));
        let changed = ["server/src/main.rs".to_owned()];
        assert!(landed.matches(&Happened::Landed { changed: &changed }));
        assert!(!broken.matches(&Happened::Landed { changed: &changed }));
        let bug = trigger(serde_json::json!({"reaction": "🐛"}));
        assert!(bug.matches(&Happened::Reaction { reaction: "🐛" }));
    }

    #[test]
    fn says_what_it_wakes_on() {
        let words = |json| trigger(json).words();
        assert_eq!(
            words(serde_json::json!({"state": "master", "level": "broken"})),
            "master is broken"
        );
        assert_eq!(
            words(serde_json::json!({"every": "1d", "at": "09:00"})),
            "every day at 09:00 UTC"
        );
        assert_eq!(
            words(serde_json::json!({"every": "7d", "day": "mon"})),
            "every 7 days on Monday"
        );
        assert_eq!(
            words(serde_json::json!({"landed": ["server/"]})),
            "a landing changes server/"
        );
        assert_eq!(
            words(serde_json::json!({"said": "refund"})),
            "someone says \"refund\""
        );
        assert_eq!(duration_words("2h"), "2 hours");
        assert_eq!(duration_words("1h"), "an hour");
        assert_eq!(duration_words("soon"), "soon");
    }

    #[test]
    fn a_time_comes_round_once_and_not_after_its_grace() {
        let daily = Schedule {
            every: DAY,
            offset: 9 * 3600,
        };
        let nine = 20_773 * DAY + 9 * 3600;
        assert_eq!(came_round(daily, None, nine + 60), Some((nine, true)));
        assert_eq!(came_round(daily, Some(nine), nine + 60), None);
        assert_eq!(
            came_round(daily, None, nine + EVERY_GRACE + 1),
            Some((nine, false))
        );
    }
}
