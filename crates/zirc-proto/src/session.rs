//! What an agent session is called, beyond its short handle: `x-zirc/session`, a
//! [`crate::Event::Custom`] whose data is a [`SessionCard`], sent by the session's machine
//! once it knows the session's tool, name or title, and again when one changes. Its title is
//! the one it was given (`zirc name --title`), else the one its agent tool shows (Claude
//! Code's sidebar title), else the first task it took on. Apps keep the latest card for
//! each machine and session, and show its title first, above its short IRC-safe handle
//! (`desktop`, `ios`): [`agent_title`], [`agent_handle`].
//!
//! A card never carries where the session can be opened: that link is the owner's alone,
//! kept on their machine and served only over its zircd's socket.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::is_zero;
use ts_rs::TS;

use crate::Event;

/// The custom event kind a session's card is sent as.
pub const KIND: &str = "x-zirc/session";

/// The longest title a card carries, in characters.
pub const MAX_TITLE: usize = 120;

/// What a session is called, as its machine says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields)]
pub struct SessionCard {
    /// The session's id, as its messages carry it.
    pub session: String,
    /// Its short handle ([`crate::Message::name`]), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its full title, at most [`MAX_TITLE`] characters, when it has one: as it was given,
    /// else as its agent tool shows it, else the first task it took on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The agent tool it runs in ([`crate::Message::tool`]), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

impl SessionCard {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a card serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a session card serializes"),
            text: self.text(),
        }
    }

    /// The card `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// The session as people read it: its name, else its id.
    #[must_use]
    pub fn who(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.session)
    }

    /// What it says, for plain IRC clients: `desktop is "Desktop: 3f review fixes"`, or,
    /// untitled, `desktop · Claude Code`.
    #[must_use]
    pub fn text(&self) -> String {
        match &self.title {
            Some(title) => format!("{} is \"{title}\"", self.who()),
            None => format!(
                "{} · {}",
                self.who(),
                crate::tool_said(self.tool.as_deref())
            ),
        }
    }
}

/// The custom event kind a machine says which of its sessions are live in: [`Live`].
pub const LIVE_KIND: &str = "x-zirc/sessions";

/// How long a session stays in [`Live::sessions`] after its last `zirc watch` closed, in
/// seconds: as long as a `zirc watch --once` takes to be started again after it wakes, and
/// then some. Only for readers from before [`Live::away`]: an app shows a session online
/// strictly while a watch is open ([`Live::online`]).
pub const LIVE_GRACE_SECS: u64 = 180;

/// How long a session that stopped watching is still said to be away ([`Live::away`]), in
/// seconds: 12 hours. After that its row folds out of Messages (the user's choice, 23 Nov
/// 2026: "once its task lands, or after 12 hours away").
pub const AWAY_FOLD_SECS: u64 = 12 * 60 * 60;

/// How long a session is away, with a message to it waiting, before its machine tells the
/// owner once ([`not_listening`]), in seconds: 15 minutes (the user's choice, 23 Nov 2026).
pub const NOT_LISTENING_TELL_SECS: u64 = 15 * 60;

/// A session with no `zirc watch` open, as its machine says: since when, and how many
/// messages to it wait for its next watch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Away {
    /// When its last watch closed, in seconds since the Unix epoch.
    pub since: u64,
    /// How many messages from people to it wait: what a person wrote to it, or about it
    /// by name, since its watch closed.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[ts(as = "Option<u32>", optional)]
    pub waiting: u32,
}

/// Which of a machine's agent sessions are live, and which are away. A session is online
/// while a `zirc watch` of its is open, and away otherwise (the user's choice, 23 Nov 2026:
/// "online strictly while a watch is open"). Its machine says it as a [`LIVE_KIND`] event,
/// only true now ([`crate::live`]), in the organisation's channels, whenever it changes and
/// to whoever joins. It replaces what the machine said before: a session it names in neither
/// list has been away longer than [`AWAY_FOLD_SECS`], or never watched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Live {
    /// The sessions with a watch open, or one closed less than [`LIVE_GRACE_SECS`] ago,
    /// sorted: what readers from before [`Live::away`] show as live.
    pub sessions: Vec<String>,
    /// The sessions with no watch open, closed less than [`AWAY_FOLD_SECS`] ago, by session:
    /// since when, and what waits for them. A machine from before it says none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(as = "Option<BTreeMap<String, Away>>", optional)]
    pub away: BTreeMap<String, Away>,
}

impl Live {
    /// The event that carries it.
    ///
    /// # Panics
    ///
    /// Never: a list of strings serializes to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: LIVE_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a live list serializes"),
            text: match self.sessions.len() {
                1 => "1 agent session live".to_owned(),
                n => format!("{n} agent sessions live"),
            },
        }
    }

    /// The list `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == LIVE_KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// Whether this machine says `session` is online: a watch of its is open. A machine
    /// from before [`Live::away`] says only [`Live::sessions`], with its grace.
    #[must_use]
    pub fn online(&self, session: &str) -> bool {
        self.sessions.iter().any(|live| live == session) && !self.away.contains_key(session)
    }
}

/// What an agent away with messages waiting says on its header and card: `Not listening for
/// 17 min · 1 message waiting`, or `Not listening · 2 messages waiting` under a minute.
/// Every app reads it, never its own.
#[must_use]
pub fn not_listening(away_secs: u64, waiting: u32) -> String {
    let waiting = messages_waiting(waiting);
    match away_for(away_secs, " min", " h") {
        Some(away) => format!("Not listening for {away} · {waiting}"),
        None => format!("Not listening · {waiting}"),
    }
}

/// The same on a row of Messages, short enough for a sidebar's line (the user's choice, 23
/// Nov 2026): `17m away · 1 waiting`, `Away · 2 waiting` under a minute; the row's away
/// ring and warning colour say it isn't listening.
#[must_use]
pub fn not_listening_short(away_secs: u64, waiting: u32) -> String {
    match away_for(away_secs, "m", "h") {
        Some(away) => format!("{away} away · {waiting} waiting"),
        None => format!("Away · {waiting} waiting"),
    }
}

/// What its machine tells the owner once it's been away [`NOT_LISTENING_TELL_SECS`] with
/// messages waiting, as from the agent itself, `title` being what it's called
/// ([`agent_title`]): `replay-metrics isn't listening: 1 message waiting for 15 min`.
#[must_use]
pub fn not_listening_notice(title: &str, away_secs: u64, waiting: u32) -> String {
    let waiting = messages_waiting(waiting);
    match away_for(away_secs, " min", " h") {
        Some(away) => format!("{title} isn't listening: {waiting} for {away}"),
        None => format!("{title} isn't listening: {waiting}"),
    }
}

/// `1 message waiting`, `2 messages waiting`.
fn messages_waiting(waiting: u32) -> String {
    match waiting {
        1 => "1 message waiting".to_owned(),
        n => format!("{n} messages waiting"),
    }
}

/// How long `secs` is, in whole minutes under an hour, else whole hours, with `minutes` or
/// `hours` after: `17 min`, `3h`. `None` under a minute.
fn away_for(secs: u64, minutes: &str, hours: &str) -> Option<String> {
    match secs / 60 {
        0 => None,
        n @ 1..60 => Some(format!("{n}{minutes}")),
        n => Some(format!("{}{hours}", n / 60)),
    }
}

/// What an agent session is called on its row, its header, Working now and what its
/// machine says for it (the user's choice, 25 Nov 2026): its card's `title` (as given, else
/// as its tool shows it, else its first task), else the task it's on, else its short
/// `name`, else its id. Its handle goes under it ([`agent_handle`]). A reader that knows
/// more of the session than its machine said may stand in for the id alone: zirc.dev's
/// Working now takes the name its activity store heard, where the standing has only the id.
#[must_use]
pub fn agent_title(
    title: Option<&str>,
    task: Option<&str>,
    name: Option<&str>,
    session: &str,
) -> String {
    [title, task, name]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|said| !said.is_empty())
        .unwrap_or(session)
        .to_owned()
}

/// The small second line under an agent session's title ([`agent_title`]): its short
/// handle, its `name` else its id, so the handle is never alone as the title nor lost under
/// one. `None` when the title is the handle already.
#[must_use]
pub fn agent_handle(title: &str, name: Option<&str>, session: &str) -> Option<String> {
    let handle = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(session);
    (handle != title).then(|| handle.to_owned())
}

/// `title` as a card carries it: one line, trimmed, at most [`MAX_TITLE`] characters, cut
/// with `…`. `None` when nothing's left.
#[must_use]
pub fn title(title: &str) -> Option<String> {
    let line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    Some(crate::clip(&line, MAX_TITLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travels_as_a_custom_event_plain_clients_show() {
        let card = SessionCard {
            session: "cc-ab600723".into(),
            name: Some("desktop".into()),
            title: Some("Desktop: 3f review fixes".into()),
            tool: Some("claude-code".into()),
        };
        let event = card.event();
        assert_eq!(SessionCard::of(&event), Some(card.clone()));
        assert_eq!(
            event.human_text(),
            "desktop is \"Desktop: 3f review fixes\""
        );
        let untitled = SessionCard {
            title: None,
            ..card
        };
        assert_eq!(untitled.text(), "desktop · Claude Code");
        assert_eq!(SessionCard::of(&untitled.event()), Some(untitled));
        assert_eq!(SessionCard::of(&Event::Text { body: "hi".into() }), None);
    }

    #[test]
    fn a_live_list_is_only_true_now() {
        let live = Live {
            sessions: vec!["cc-1".into(), "cc-2".into()],
            ..Live::default()
        };
        let event = live.event();
        assert_eq!(Live::of(&event), Some(live.clone()));
        assert!(crate::live(&event));
        assert_eq!(event.level(), Some(crate::Level::Debug));
        assert_eq!(Live::of(&Event::Text { body: "hi".into() }), None);
    }

    #[test]
    fn a_session_is_online_only_while_its_watch_is_open() {
        let live = Live {
            sessions: vec!["cc-1".into(), "cc-2".into()],
            away: BTreeMap::from([(
                "cc-2".into(),
                Away {
                    since: 100,
                    waiting: 1,
                },
            )]),
        };
        assert!(live.online("cc-1"));
        // In its grace for older readers, but away.
        assert!(!live.online("cc-2"));
        assert!(!live.online("cc-3"));
        assert_eq!(Live::of(&live.event()), Some(live.clone()));
        let Event::Custom { data, .. } = live.event() else {
            unreachable!("a live list is a custom event")
        };
        assert_eq!(data["away"]["cc-2"]["since"], 100);
        // Nothing waiting isn't said.
        let quiet = Live {
            away: BTreeMap::from([(
                "cc-2".into(),
                Away {
                    since: 100,
                    waiting: 0,
                },
            )]),
            ..live
        };
        let Event::Custom { data, .. } = quiet.event() else {
            unreachable!("a live list is a custom event")
        };
        assert!(data["away"]["cc-2"].get("waiting").is_none());
        assert_eq!(Live::of(&quiet.event()), Some(quiet));
    }

    #[test]
    fn not_listening_says_how_long_and_how_many_wait() {
        assert_eq!(not_listening(20, 1), "Not listening · 1 message waiting");
        assert_eq!(
            not_listening(17 * 60 + 5, 1),
            "Not listening for 17 min · 1 message waiting"
        );
        assert_eq!(
            not_listening(3 * 3600 + 120, 2),
            "Not listening for 3 h · 2 messages waiting"
        );
    }

    #[test]
    fn a_row_says_it_short_and_the_owner_hears_it_from_the_agent_by_name() {
        assert_eq!(not_listening_short(17 * 60 + 5, 1), "17m away · 1 waiting");
        assert_eq!(not_listening_short(20, 2), "Away · 2 waiting");
        assert_eq!(not_listening_short(3 * 3600, 1), "3h away · 1 waiting");
        assert_eq!(
            not_listening_notice("replay-metrics", 15 * 60, 1),
            "replay-metrics isn't listening: 1 message waiting for 15 min"
        );
    }

    #[test]
    fn the_title_leads_and_the_handle_is_the_second_line() {
        let title = agent_title(
            Some("Session names: tool and title"),
            Some("the login page"),
            Some("presence"),
            "cc-1",
        );
        assert_eq!(title, "Session names: tool and title");
        assert_eq!(
            agent_handle(&title, Some("presence"), "cc-1").as_deref(),
            Some("presence")
        );
        let by_task = agent_title(None, Some("the login page"), Some("presence"), "cc-1");
        assert_eq!(by_task, "the login page");
        assert_eq!(
            agent_handle(&by_task, None, "cc-1").as_deref(),
            Some("cc-1")
        );
        // Only a name: it's the title, with nothing under it.
        let named = agent_title(Some(" "), None, Some("presence"), "cc-1");
        assert_eq!(named, "presence");
        assert_eq!(agent_handle(&named, Some("presence"), "cc-1"), None);
        assert_eq!(agent_title(None, None, None, "cc-1"), "cc-1");
        assert_eq!(agent_handle("cc-1", None, "cc-1"), None);
    }

    #[test]
    fn a_title_is_one_line_and_not_too_long() {
        assert_eq!(
            title("  Fix\nthe   login\tpage "),
            Some("Fix the login page".into())
        );
        assert_eq!(title(" \n "), None);
        let long = "x".repeat(MAX_TITLE + 10);
        let cut = title(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_TITLE);
        assert!(cut.ends_with('…'));
    }
}
