//! Gotchas' types: one-line lessons about some paths, kept in the organisation's gotchas room
//! ([`CHANNEL`], `#gotchas`) as [`KIND`] events, each saying its repository
//! ([`crate::Message::repo`]), and retired by [`RETIRED`] ones. Said here, in the
//! protocol, so zirc's core, which tells them, and the server's sidecar, which lists them for
//! the team's Playbook page, keep the same ones ([`kept`]).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;

/// The kind of a gotcha's event.
pub const KIND: &str = "x-zirc/gotcha";

/// The kind of the event that retires a gotcha.
pub const RETIRED: &str = "x-zirc/gotcha-retired";

/// The organisation's room that holds every repository's gotchas, each event saying which
/// repository it's about. The rooms each repository had before, `#<repo>/gotchas`, are left
/// as they were on 20 Nov 2026, and nothing reads them.
pub const CHANNEL: &str = "#gotchas";

/// How many of a room's newest messages are read for its gotchas: all of them, as only
/// gotchas go there.
pub const READ: usize = 5000;

/// How many gotchas `zirc intend`, and zircd's line on a session's first change, say at most.
pub const SHOWN: usize = 5;

/// A lesson about some paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
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

/// Whether `channel` is the gotchas room ([`CHANNEL`]).
#[must_use]
pub fn is_room(channel: &str) -> bool {
    channel.eq_ignore_ascii_case(CHANNEL)
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

/// One of a team's gotchas, as its server lists them for the Playbook page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "GotchaListed")]
pub struct Listed {
    /// The room it's kept in, [`CHANNEL`]. Kept for the dashboard, which reads a sidecar's
    /// listing from before rooms were the organisation's.
    pub room: String,
    /// The repository it's about, by name: `api`.
    pub repository: String,
    /// The gotcha.
    pub gotcha: Gotcha,
    /// The start of its id as zirc shows it (`[id 01jbx7ab]`), which `zirc gotcha --retire`
    /// takes; empty without an id.
    pub short: String,
}

impl Listed {
    /// `gotcha`, about `repository`.
    #[must_use]
    pub fn new(repository: &str, gotcha: Gotcha) -> Self {
        let short = gotcha
            .id
            .as_deref()
            .map(|id| crate::short_id(id).to_owned())
            .unwrap_or_default();
        Self {
            room: CHANNEL.to_owned(),
            repository: repository.to_owned(),
            gotcha,
            short,
        }
    }
}

/// A team's gotchas, as its server lists them: newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "GotchaListing")]
pub struct Listing {
    /// The gotchas.
    pub gotchas: Vec<Listed>,
}

/// How often one part of a team's playbook was used, and when last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "PlaybookUsed")]
pub struct Used {
    /// How many times.
    pub count: u64,
    /// When last, in RFC 3339.
    pub last: String,
}

/// How often each hint was shown and each gotcha told lately, as a team's server counts it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "PlaybookUsage")]
pub struct Usage {
    /// How many days back it counts.
    pub days: u32,
    /// By a hint's id.
    pub hints: std::collections::BTreeMap<String, Used>,
    /// By a gotcha's message id.
    pub gotchas: std::collections::BTreeMap<String, Used>,
}

/// One time a hint was shown or a gotcha told, as a team's server keeps it: to whom and when,
/// never what anyone said.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "PlaybookUse", optional_fields)]
pub struct Use {
    /// When, in RFC 3339.
    pub at: String,
    /// The machine's account it was said on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The agent session it was said to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The session's name, such as `controller`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The agent tool the session runs in, such as `claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// How: a hint's layer (`personal`, `repo`, `org` or `default`), or how a gotcha was told
    /// (`intent` or `start`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
}

/// One hint's or gotcha's uses lately, as a team's server lists them: how many each day, and
/// a page of them, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "PlaybookUses", optional_fields)]
pub struct Uses {
    /// How many days back it counts.
    pub days: u32,
    /// How many each UTC day, one a day for [`Uses::days`], the oldest first: the last is
    /// today so far.
    pub daily: Vec<u64>,
    /// A page of them, newest first.
    pub uses: Vec<Use>,
    /// What `before` takes for the page after this one, when there are older ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older: Option<String>,
}

/// The TypeScript declarations of what zirc.dev's Playbook page reads of a team's gotchas.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Gotcha::decl(config),
        Listed::decl(config),
        Listing::decl(config),
        Used::decl(config),
        Usage::decl(config),
        Use::decl(config),
        Uses::decl(config),
    ]
}
