//! How something a team shares stands now, such as its default branch: `x-zirc/state`, a
//! [`crate::Event::Custom`] whose data is a [`State`]. Anyone may set one (`zirc state set`):
//! a person, an agent session, or a team's own scripts, such as CI saying the default
//! branch is broken and who's fixing it. The newest state of a name in a repository is how
//! it stands; one that's [`Level::Ok`] says it's fine again. zirc knows nothing of what set
//! it, so it stays a team's to say, whatever CI it uses.
//!
//! States go to the organisation's `#ops`, as decisions do, where every machine watching a
//! repository is: `zirc state` and `zirc status` read them there. The text names the owner,
//! which wakes their watch; nobody else's.
//!
//! It's a custom event, so readers that predate it, and plain IRC clients, show its text.

/// The organisation's incident room, where states and decisions go: what's said there needs
/// someone, so apps show it from warn up ([`crate::Level`]).
pub const CHANNEL: &str = "#ops";
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;

/// The custom event kind a state is sent as.
pub const KIND: &str = "x-zirc/state";

/// The longest a state's name may be.
pub const MAX_NAME: usize = 64;

/// How it stands.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "StateLevel")]
pub enum Level {
    /// Fine: what a state is when nobody has said otherwise.
    Ok,
    /// Worth knowing before relying on it, such as a flaky check.
    Warning,
    /// Broken: work that relies on it will fail until it's fixed.
    Broken,
}

impl Level {
    /// Its word: `ok`, `warning`, `broken`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Broken => "broken",
        }
    }

    /// Every level, best first.
    pub const ALL: [Self; 3] = [Self::Ok, Self::Warning, Self::Broken];

    /// The level `word` names, if it's one.
    #[must_use]
    pub fn of_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.word() == word)
    }
}

/// How one thing in a repository stands now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct State {
    /// The channel of the repository it's in, such as `#monorepo`: zircd's word, from where
    /// it was set.
    pub repo: String,
    /// What it's the state of, such as `master`: lowercase letters, digits, `-`, `_`, `.`
    /// and `/`, as a branch or an environment is named.
    pub name: String,
    /// How it stands.
    pub level: Level,
    /// Why, in a few words, such as which check fails since when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Who's on it: a session or a person, whose watch it wakes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Where to read more, such as a failed build's log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// When it ends by itself, as a time tag (`2026-11-14T15:00:00.000Z`): a hold, such as
    /// `landings` broken while a migration runs, reads as fine from then on
    /// ([`State::ended`]). Readers that predate it show it as it was said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

/// The name of the team's hold on landing: `landings` broken, until a time it must say,
/// stops every landing, its fixes too (the user's decisions, 14 Nov 2026).
pub const HOLD: &str = "landings";

/// What's wrong with `name` as a state's name, if anything.
#[must_use]
pub fn check_name(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.len() > MAX_NAME {
        return Some("a state's name is 1 to 64 characters");
    }
    let fine = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_./".contains(c);
    (!name.chars().all(fine))
        .then_some("a state's name is lowercase letters, digits, -, _, . and /, as in master")
}

impl State {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a state serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a state serializes"),
            text: self.text(),
        }
    }

    /// The state `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// Whether it has ended by itself by `now`, a time tag as `until` is: RFC 3339 in UTC
    /// sorts as text, so no clock is needed to compare them.
    #[must_use]
    pub fn ended(&self, now: &str) -> bool {
        self.until.as_deref().is_some_and(|until| until <= now)
    }

    /// Who's on it, each as its session when it's named as CI names one
    /// (`login-page (cc-1a2b3c)`), else as said: `owner` split at its commas.
    #[must_use]
    pub fn owners(&self) -> Vec<&str> {
        self.owner
            .as_deref()
            .into_iter()
            .flat_map(|owner| owner.split(','))
            .map(str::trim)
            .filter(|who| !who.is_empty())
            .map(|who| crate::session_of(who).unwrap_or(who))
            .collect()
    }

    /// The state that says it has ended by itself: fine, saying until when it stood as it did.
    #[must_use]
    pub fn over(&self) -> Self {
        Self {
            level: Level::Ok,
            why: Some(format!(
                "it was {} until {}",
                self.level.word(),
                self.until.as_deref().unwrap_or("?")
            )),
            owner: None,
            link: None,
            until: None,
            ..self.clone()
        }
    }

    /// What it says, for people: `#monorepo master is broken: //web:e2e fails since build
    /// 1008; cc-2b76ff is on it`, or `#monorepo master is ok`.
    #[must_use]
    pub fn text(&self) -> String {
        let mut said = format!("{} {} is {}", self.repo, self.name, self.level.word());
        if let Some(why) = &self.why {
            said = format!("{said}: {why}");
        }
        if let Some(until) = &self.until {
            said = format!("{said}; until {until}");
        }
        if let Some(owner) = &self.owner {
            said = format!("{said}; {owner} is on it");
        }
        if let Some(link) = &self.link {
            said = format!("{said} {link}");
        }
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travels_as_a_custom_event_any_reader_shows() {
        let state = State {
            repo: "#monorepo".into(),
            name: "master".into(),
            level: Level::Broken,
            why: Some("//web:e2e fails since build 1008".into()),
            owner: Some("cc-2b76ff".into()),
            link: None,
            until: None,
        };
        let event = state.event();
        assert!(crate::check_kind(KIND).is_none());
        assert_eq!(
            event.human_text(),
            "#monorepo master is broken: //web:e2e fails since build 1008; cc-2b76ff is on it"
        );
        assert_eq!(State::of(&event), Some(state.clone()));
        let fine = State {
            level: Level::Ok,
            why: None,
            owner: None,
            ..state
        };
        assert_eq!(fine.text(), "#monorepo master is ok");
        let held = State {
            name: HOLD.into(),
            level: Level::Broken,
            why: Some("the database moves".into()),
            until: Some("2026-11-14T15:00:00.000Z".into()),
            ..fine
        };
        assert_eq!(
            held.text(),
            "#monorepo landings is broken: the database moves; until 2026-11-14T15:00:00.000Z"
        );
        assert!(!held.ended("2026-11-14T14:59:59.999Z"));
        assert!(held.ended("2026-11-14T15:00:00.000Z"));
        assert_eq!(
            held.over().text(),
            "#monorepo landings is ok: it was broken until 2026-11-14T15:00:00.000Z"
        );
        assert_eq!(State::of(&held.event()), Some(held.clone()));
        assert_eq!(
            State::of(&Event::Text {
                body: "master is broken".into()
            }),
            None
        );
    }

    #[test]
    fn its_owners_are_sessions_where_named_as_ci_names_them() {
        let state = State {
            repo: "#monorepo".into(),
            name: "master".into(),
            level: Level::Broken,
            why: None,
            owner: Some("login-page (cc-1a2b3c), cc-2, mattias".into()),
            link: None,
            until: None,
        };
        assert_eq!(state.owners(), ["cc-1a2b3c", "cc-2", "mattias"]);
        let none = State {
            owner: None,
            ..state
        };
        assert!(none.owners().is_empty());
    }

    #[test]
    fn names_are_what_branches_and_environments_are_called() {
        assert_eq!(check_name("master"), None);
        assert_eq!(check_name("deploy/production"), None);
        assert!(check_name("").is_some());
        assert!(check_name("Master").is_some());
        assert!(check_name("master branch").is_some());
        assert!(check_name(&"x".repeat(MAX_NAME + 1)).is_some());
    }

    #[test]
    fn levels_say_their_words() {
        for level in [Level::Ok, Level::Warning, Level::Broken] {
            assert_eq!(Level::of_word(level.word()), Some(level));
        }
        assert_eq!(Level::of_word("red"), None);
    }
}
