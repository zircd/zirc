//! Gotchas' types: one-line lessons about some paths, kept in a repository's gotchas room
//! (`#<repo>/gotchas`) as [`KIND`] events, and retired by [`RETIRED`] ones. Said here, in the
//! protocol, so zirc's core, which tells them, and the server's sidecar, which lists them for
//! the team's Playbook page, keep the same ones ([`kept`]).

use serde::{Deserialize, Serialize};

use crate::Event;

/// The kind of a gotcha's event.
pub const KIND: &str = "x-zirc/gotcha";

/// The kind of the event that retires a gotcha.
pub const RETIRED: &str = "x-zirc/gotcha-retired";

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

/// What a retiring event carries: the gotcha it retires, by its paths and a hash of its
/// words ([`Retired::sha`]), which match it on every machine whether or not the server gave
/// its message an id there, and stay short when its text is cut to fit a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retired {
    /// The retired gotcha's paths.
    pub paths: Vec<String>,
    /// The start of the SHA-256 of the retired gotcha's words, in hex.
    pub sha: String,
}

impl Retired {
    /// What retires `gotcha`.
    #[must_use]
    pub fn of(gotcha: &Gotcha) -> Self {
        Self {
            paths: gotcha.paths.clone(),
            sha: Self::sha(&gotcha.text),
        }
    }

    /// The fingerprint of `lesson` ([`crate::hints::fingerprint`]).
    #[must_use]
    pub fn sha(lesson: &str) -> String {
        crate::hints::fingerprint(lesson)
    }

    /// Whether it retires `gotcha`.
    #[must_use]
    pub fn retires(&self, gotcha: &Gotcha) -> bool {
        self.paths == gotcha.paths && self.sha == Self::sha(&gotcha.text)
    }
}

/// The text of the event retiring `gotcha`, for apps and plain IRC clients.
#[must_use]
pub fn retired_text(gotcha: &Gotcha) -> String {
    format!(
        "retired a gotcha about {}: {}",
        gotcha.paths.join(", "),
        gotcha.text
    )
}

/// Whether `kind` is one of a gotchas room's: a gotcha or its retiring, which wake no one.
#[must_use]
pub fn is_kind(kind: &str) -> bool {
    kind == KIND || kind == RETIRED
}

/// Whether `channel` is a repository's gotchas room.
#[must_use]
pub fn is_room(channel: &str) -> bool {
    channel
        .rsplit_once('/')
        .is_some_and(|(parent, room)| parent.len() > 1 && room.eq_ignore_ascii_case(ROOM))
}

/// The paths and words of the gotcha `event` says, if it's one.
#[must_use]
pub fn lesson(event: &Event) -> Option<(Vec<String>, String)> {
    let Event::Custom { kind, data, text } = event else {
        return None;
    };
    if kind != KIND {
        return None;
    }
    let Data { paths } = serde_json::from_value(data.clone()).ok()?;
    Some((paths, text.clone()))
}

/// The retiring `event` says, if it's one.
#[must_use]
pub fn retiring(event: &Event) -> Option<Retired> {
    let Event::Custom { kind, data, .. } = event else {
        return None;
    };
    (kind == RETIRED)
        .then(|| serde_json::from_value(data.clone()).ok())
        .flatten()
}

/// What a gotchas room says, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// A gotcha.
    Kept(Gotcha),
    /// A gotcha's retiring.
    Retired(Retired),
}

/// The gotchas `said`, a room's history oldest first, keep: each said and not retired since.
/// Saying the same lesson again after its retiring keeps it once more.
#[must_use]
pub fn kept(said: impl IntoIterator<Item = Said>) -> Vec<Gotcha> {
    let mut kept: Vec<Gotcha> = Vec::new();
    for said in said {
        match said {
            Said::Kept(gotcha) => kept.push(gotcha),
            Said::Retired(retired) => kept.retain(|gotcha| !retired.retires(gotcha)),
        }
    }
    kept
}
