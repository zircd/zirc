//! Gotchas: one-line lessons a session learnt the hard way, tied to the paths they're about,
//! such as a lint only Linux CI runs, or a test that's flaky under load. `zirc gotcha` posts
//! one, as an [`KIND`] event in the repository's [`ROOM`], `#<repo>/gotchas`, so they live
//! in the server's history rather than the repository, shared at once by every machine;
//! `zirc gotchas <path>` lists those about a path, newest first. `zirc intend` says those
//! under the paths it names, and zircd tells a session those under the directories it
//! starts changing without one ([`under`]).

use serde::{Deserialize, Serialize};
use zirc_proto::Event;

use crate::ipc::Delivery;
use crate::presence::touches;

/// The kind of a gotcha's event.
pub const KIND: &str = "x-zirc/gotcha";

/// The standing room under a repository's channel that holds its gotchas.
pub const ROOM: &str = "gotchas";

/// How many of a room's newest messages are read for its gotchas: all of them, as only
/// gotchas go there.
pub const READ: usize = 5000;

/// How many gotchas `zirc intend`, and zircd's line on a session's first change, say at most.
pub const SHOWN: usize = 5;

/// A lesson about some paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gotcha {
    /// The paths it's about, relative to the repository's root: files or directories.
    pub paths: Vec<String>,
    /// The lesson.
    pub text: String,
    /// Who learnt it: the session, as it's called, or the machine's nick.
    pub who: String,
    /// When it was said, in RFC 3339, when the server says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Its message's id, when the server gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// What a gotcha's event carries besides its text: the paths it's about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Data {
    /// The paths it's about.
    pub paths: Vec<String>,
}

/// The gotcha `delivery` says, if it's one.
#[must_use]
pub fn of(delivery: &Delivery) -> Option<Gotcha> {
    let Event::Custom { kind, data, text } = &delivery.message.event else {
        return None;
    };
    if kind != KIND {
        return None;
    }
    let Data { paths } = serde_json::from_value(data.clone()).ok()?;
    let session = &delivery.message.session;
    let who = crate::clashes::label(
        &delivery.from,
        (!session.is_empty()).then_some(session.as_str()),
        delivery.message.name.as_deref(),
    );
    Some(Gotcha {
        paths,
        text: text.clone(),
        who,
        at: delivery.time.clone(),
        id: delivery.id.clone(),
    })
}

/// Whether `channel` is a repository's gotchas room.
#[must_use]
pub fn is_room(channel: &str) -> bool {
    channel
        .rsplit_once('/')
        .is_some_and(|(parent, room)| parent.len() > 1 && room.eq_ignore_ascii_case(ROOM))
}

/// Those of `gotchas` about any of `paths`, or what's in them, or what they're in: newest
/// first, each once.
#[must_use]
pub fn under<'a>(gotchas: &'a [Gotcha], paths: &[String]) -> Vec<&'a Gotcha> {
    let mut found: Vec<&Gotcha> = gotchas
        .iter()
        .filter(|gotcha| {
            gotcha
                .paths
                .iter()
                .any(|theirs| paths.iter().any(|mine| touches(theirs, mine)))
        })
        .collect();
    // RFC 3339 times from one server sort as text; one without sorts last.
    found.sort_by(|a, b| b.at.cmp(&a.at));
    found.dedup_by(|a, b| a.text == b.text && a.paths == b.paths);
    found
}

/// `gotcha` in a line: `2026-11-11 cc-1 (api): <text> (<paths>)`.
#[must_use]
pub fn line(gotcha: &Gotcha) -> String {
    let day = gotcha.at.as_deref().and_then(|at| at.get(..10));
    let paths = gotcha.paths.join(", ");
    match day {
        Some(day) => format!("{day} {}: {} ({paths})", gotcha.who, gotcha.text),
        None => format!("{}: {} ({paths})", gotcha.who, gotcha.text),
    }
}

/// `gotchas` as lines to show under a heading, the first [`SHOWN`] of them; empty when there
/// are none.
#[must_use]
pub fn text(gotchas: &[&Gotcha]) -> String {
    use std::fmt::Write as _;
    if gotchas.is_empty() {
        return String::new();
    }
    let mut text = String::from("gotchas others learnt here, newest first:\n");
    for gotcha in gotchas.iter().take(SHOWN) {
        let _ = writeln!(text, "  {}", line(gotcha));
    }
    if gotchas.len() > SHOWN {
        let _ = writeln!(
            text,
            "  and {} more: `zirc gotchas <path>`",
            gotchas.len() - SHOWN
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use zirc_proto::Message;

    use super::*;

    fn said(session: &str, paths: &[&str], text: &str, at: &str) -> Delivery {
        Delivery {
            id: Some(format!("id-{at}")),
            time: Some(at.into()),
            ..Delivery::new(
                "#api/gotchas",
                "mbp",
                Message::new(
                    session,
                    Event::Custom {
                        kind: KIND.into(),
                        data: serde_json::to_value(Data {
                            paths: paths.iter().map(|&path| path.to_owned()).collect(),
                        })
                        .unwrap(),
                        text: text.into(),
                    },
                ),
            )
        }
    }

    #[test]
    fn finds_those_about_a_path_or_what_holds_it_newest_first() {
        let history = [
            said(
                "cc-1",
                &["src/daemon"],
                "clippy's line limit is 100",
                "2026-11-09T10:00:00Z",
            ),
            said(
                "cc-2",
                &["src/daemon/awaits.rs"],
                "saved: keep serde defaults",
                "2026-11-10T10:00:00Z",
            ),
            said("cc-3", &["web"], "pnpm only", "2026-11-11T10:00:00Z"),
        ];
        let gotchas: Vec<Gotcha> = history.iter().filter_map(of).collect();
        assert_eq!(gotchas.len(), 3);
        let found = under(&gotchas, &["src/daemon/awaits.rs".to_owned()]);
        let lines: Vec<String> = found.iter().map(|gotcha| line(gotcha)).collect();
        assert_eq!(
            lines,
            [
                "2026-11-10 cc-2: saved: keep serde defaults (src/daemon/awaits.rs)",
                "2026-11-09 cc-1: clippy's line limit is 100 (src/daemon)",
            ]
        );
        // A directory finds those about files in it too.
        assert_eq!(under(&gotchas, &["src".to_owned()]).len(), 2);
        assert!(under(&gotchas, &["docs".to_owned()]).is_empty());
        assert!(
            text(&found).starts_with("gotchas others learnt here, newest first:\n  2026-11-10")
        );
        assert_eq!(text(&[]), "");
    }

    #[test]
    fn only_a_gotcha_event_is_one_and_only_the_room_is_the_room() {
        let mut other = said("cc-1", &["src"], "x", "2026-11-09T10:00:00Z");
        other.message.event = Event::Text { body: "x".into() };
        assert_eq!(of(&other), None);
        assert!(is_room("#api/gotchas"));
        assert!(!is_room("#api/agents"));
        assert!(!is_room("#gotchas"));
    }
}
