//! zirc's events and the envelope they travel in over IRC.
//!
//! A zirc message is a PRIVMSG body framed as a CTCP ACTION, so that Ergo keeps it in
//! chathistory and plain IRC clients still show something readable:
//!
//! ```text
//! \x01ACTION ZIRC <base64 json> :<human-readable text>\x01
//! ```
//!
//! The JSON is a [`Message`]; the human text is derived from its [`Event`] and only ever
//! read by people.

pub mod activity;
pub mod attached;
pub mod card;
pub mod ci;
pub mod colleagues;
pub mod decision;
pub mod desired;
pub mod device_key;
pub mod files;
pub mod gotchas;
pub mod hints;
pub mod images;
pub mod irc;
pub mod join;
pub mod lines;
pub mod link;
pub mod logging;
pub mod login;
pub mod media;
pub mod names;
pub mod onboarding;
pub mod profile;
pub mod push;
pub mod receipt;
pub mod report;
pub mod request;
pub mod role;
pub mod run;
pub mod sealed;
pub mod search;
pub mod session;
pub mod state;
pub mod suggestions;
pub mod telemetry;
pub mod thread;
pub mod uploads;
pub mod workflow;

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The session zircd signs messages with when it knows of none, such as a working set from
/// a worktree no session said it works in.
pub const NO_SESSION: &str = "watcher";

/// zirc's name as people read it where they mean the product, its apps, its service or the
/// company, a proper name: in apps' titles, menus and messages. Bundle and display names
/// take it from `bazel/brand.bzl`'s `PRODUCT_NAME`, a copy Bazel needs, which a test
/// keeps the same.
pub const NAME: &str = "Zirc";

/// The program's name, as its commands and what they print say it: `zirc` and `zircd`,
/// their versions, install, config and logs. Identifiers, the domain and the wordmark are
/// lowercase too.
pub const PROGRAM: &str = "zirc";

/// The word after `ACTION` that marks a body as zirc's.
pub const TAG: &str = "ZIRC";

/// The version of [`Message`] this crate writes. Readers reject versions they don't know.
pub const VERSION: u32 = 1;

/// The organisation-wide channel, which every member and machine is in.
pub const ORG_CHANNEL: &str = "#general";

/// The organisation's one channel for its agents' work, whatever repository it's in: what
/// sessions take on and finish, their intents, working sets, commits and pushes, and CI's
/// landings, deploys and checks. Each line says its repository ([`Message::repo`]). There are
/// no channels per repository: those from before 20 Nov 2026 are left as they were.
pub const AGENTS_CHANNEL: &str = "#agents";

/// How many digits of a commit's id a working set says what it's built on by
/// ([`Event::Working`]'s `base`): enough to name one commit of a repository, in a line whose
/// sender, worktree and branch leave some 60 bytes.
pub const BASE_DIGITS: usize = 12;

/// The longest PRIVMSG body we write. IRC lines are 512 bytes including the sender's
/// prefix, the command, the target and CRLF, so the body gets what's left with some room.
pub const MAX_BODY_BYTES: usize = 400;

const CTCP: char = '\x01';
const ACTION_PREFIX: &str = "\x01ACTION ";

/// Something one agent or person tells the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    /// What a session is about to work on, sent before it starts editing so that others
    /// changing the same paths hear of it while it's cheap to avoid. A session's intent
    /// replaces its previous one, and ends when it expires or the session posts `done:` or
    /// `dropped:`.
    Intent {
        /// One line, as a person would say it.
        summary: String,
        /// Paths or directories it expects to touch, relative to the repository root.
        paths: Vec<String>,
        /// When it ends unless renewed, in seconds since the Unix epoch, by the sender's
        /// clock. One without lasts until its session is done.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires: Option<u64>,
        /// How many seconds it lasts from when it was sent: from the message's server time,
        /// which works for replayed messages and other clocks alike. Readers prefer it to
        /// `expires`, which stays for readers that predate it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lasts: Option<u64>,
    },
    /// Every path a machine has uncommitted changes in, relative to the repository root,
    /// sent whenever the set changes and when someone joins. A set too long for one line is
    /// split: the first part replaces what the receiver knew, and the parts after it are
    /// marked `continued` and add to it.
    ///
    /// A machine with several worktrees sends one part per worktree with changes, each
    /// naming its worktree and branch, and the message's session is the one working there.
    /// A reader that ignores `worktree` and `branch` still gets the machine's whole set. A
    /// machine whose worktrees are all clean sends one part with no paths.
    ///
    /// Each part also says which it is of how many, so a reader that gets only some of
    /// them, as a history replay cut short can give it, can tell and keep what it had.
    ///
    /// Between whole sets, a machine says a worktree's changed set alone, numbered [`ALONE`]:
    /// it replaces that worktree's set and no other.
    Working {
        /// The paths.
        paths: Vec<String>,
        /// Whether this adds to the previous message's paths rather than replacing them.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        continued: bool,
        /// The name of the worktree's directory the paths are in; never its full path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree: Option<String>,
        /// The branch checked out in that worktree, unless its HEAD is detached.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// Which part of the set this is and how many there are, counting from 1, or
        /// [`ALONE`] for one worktree's set said alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<[u32; 2]>,
        /// The commit the worktree's work is built on: the newest commit of the remote's
        /// default branch that it has, by the first [`BASE_DIGITS`] digits of its id, as a
        /// line has little room. With it, anyone who knows what landed since can tell
        /// whether the work is behind a landing. Left out without a remote default branch,
        /// where it doesn't fit, and by machines that predate it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        /// The paths the worktree's own commits since `base` change, beyond `paths`: with
        /// `paths`, all its work changes. A worktree with commits of its own and nothing
        /// uncommitted is said with these alone, which readers that predate them take as a
        /// worktree with no changes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<String>>", optional)]
        committed: Vec<String>,
    },
    /// A new commit.
    Commit {
        /// The branch it was made on.
        branch: String,
        /// The commit's full id.
        sha: String,
        /// The first line of its message.
        subject: String,
        /// The paths it changed, relative to the repository's root, so a session changing
        /// them too hears of it before the branch lands. Left out of a very large commit.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<String>>", optional)]
        paths: Vec<String>,
        /// The repository's web address, such as `https://github.com/zircapp/monorepo`, so
        /// apps can link the commit, its branch and its files. Left out where it has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        repo: Option<String>,
    },
    /// A push to the remote.
    Push {
        /// The branch pushed.
        branch: String,
        /// The commit the branch now points at.
        head: String,
        /// Whether the push rewrote history.
        forced: bool,
        /// The subjects of the commits it pushed, oldest first, so a branch's name needn't
        /// say what it holds.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<String>>", optional)]
        subjects: Vec<String>,
        /// The paths its commits changed, relative to the repository's root, so a session
        /// working on them hears that it may need to rebase. Left out of a very large push.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<String>>", optional)]
        paths: Vec<String>,
        /// The commit the remote branch pointed at before, which `lines` count in.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        base: Option<String>,
        /// For each path, the lines of `base` the push changes, as `[first, count]`, with
        /// a count of 0 for lines added after `first`: so a machine without the push's
        /// commits can tell whether its own changes to those lines will conflict. Left out
        /// first of a large push.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        #[ts(as = "Option<BTreeMap<String, Vec<[u32; 2]>>>", optional)]
        lines: BTreeMap<String, Vec<[u32; 2]>>,
        /// The repository's web address, such as `https://github.com/zircapp/monorepo`, so
        /// apps can link the push, its branch and its files. Left out where it has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        repo: Option<String>,
        /// The branch not landed yet that this one is built on, as the pushing machine's git
        /// says ([`zirc stack`]'s): whoever works on that one hears that someone builds on it,
        /// before they amend or force-push it from under them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        on: Option<String>,
    },
    /// Free text, for people.
    Text {
        /// The text.
        body: String,
    },
    /// A question for agents, sent with `zirc ask` to a machine's nick: unlike other
    /// messages to a machine, `zirc watch` delivers it, to the session it names or to every
    /// session there, when the machine lets the sender query its agents. It's a teammate's
    /// request, never the person's instruction.
    Query {
        /// What names it, to answer it by: unique enough among the asker's queries.
        id: String,
        /// The session on the machine it's for; every session there when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        /// The question.
        text: String,
    },
    /// The answer to a [`Event::Query`], sent with `zirc reply` to the asking machine's
    /// nick, which hands it to the session that asked.
    Answer {
        /// The query's id.
        query: String,
        /// The session that asked, as the query's message named it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        /// The answer.
        text: String,
    },
    /// A note for one agent session, sent with `zirc note` to the nick of the machine it works
    /// on: `zirc watch` delivers it to that session alone, when the machine lets the sender
    /// query its agents, as it does a [`Event::Query`]. Unlike a query it asks for no answer:
    /// it's a report or a heads-up for one session, which nobody else need read. It's a
    /// teammate's word, never the person's instruction.
    Note {
        /// The session it's for: its id, or the name it goes by.
        to: String,
        /// What it says.
        text: String,
    },
    /// A file someone shared, stored by zirc.dev: an image, a video or any other file. The
    /// human text is its link, which works in any client once the person is signed in to
    /// zirc.dev; zirc's apps draw it inline from what's here without asking first.
    File(File),
    /// An event a team made up, such as a deploy or who's on call, sent by `zirc emit`: zirc
    /// carries it without knowing what it means. Readers that predate it drop it, and plain
    /// IRC clients and zirc's apps show its text.
    Custom {
        /// What kind of event it is, in the team's namespace: `x-<team>/<name>`, such as
        /// `x-acme/deploy`. See [`check_kind`].
        kind: String,
        /// Whatever the team wants it to carry.
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        #[ts(as = "Option<serde_json::Value>", optional)]
        data: serde_json::Value,
        /// What it says, for people.
        text: String,
    },
}

/// A file shared in chat: what [`Event::File`] carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "SharedFile")]
pub struct File {
    /// Where it's downloaded: `https://zirc.dev/m/<org>/<id>`, its id unguessable and never
    /// derived from its name. An image is resized by asking for it with `?w=<width>`.
    pub url: String,
    /// Its name as uploaded, maybe shortened to fit the line.
    pub name: String,
    /// Its media type, from its bytes rather than its name, such as `image/webp`.
    pub mime: String,
    /// Its size in bytes.
    pub size: u64,
    /// An image's or video's width in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// An image's or video's height in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A video's length in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Why a custom event's kind isn't one: `None` when it's fine. A kind is `x-`, a namespace
/// and a name joined by `/`, each of lowercase letters, digits and dashes, such as
/// `x-acme/deploy`: zirc's own events never start with `x-`, so a team's never clash with
/// them, and teams keep apart by their namespaces.
#[must_use]
pub fn check_kind(kind: &str) -> Option<&'static str> {
    let word = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let Some(rest) = kind.strip_prefix("x-") else {
        return Some("a custom kind starts with x-, as in x-acme/deploy");
    };
    match rest.split_once('/') {
        Some((team, name)) if word(team) && word(name) => None,
        _ => Some(
            "a custom kind is x-<team>/<name>, each of lowercase letters, digits and dashes, \
             as in x-acme/deploy",
        ),
    }
}

/// Every kind zirc's own custom events go by, as JSON's `.event.kind` says them: what a
/// filter on zirc's events can name. zircd's own bookkeeping goes by `.event.type` instead
/// ([`MACHINE_EVENTS`]).
pub const ZIRC_KINDS: [&str; 24] = [
    activity::WAKE_KIND,
    activity::WAKE_OUTCOME_KIND,
    activity::PROMPT_KIND,
    activity::NOTICE_KIND,
    activity::HINT_SHOWN_KIND,
    activity::GOTCHA_TOLD_KIND,
    hints::CHANGED_KIND,
    ci::LANDING_KIND,
    ci::DEPLOY_KIND,
    ci::CHECK_KIND,
    colleagues::KIND,
    decision::KIND,
    lines::KIND,
    request::KIND,
    run::KIND,
    run::STEP_KIND,
    session::KIND,
    session::LIVE_KIND,
    state::KIND,
    suggestions::KIND,
    thread::KIND,
    workflow::KIND,
    LEASE_KIND,
    RENEWAL_KIND,
];

/// The first `x-zirc/…` kind `text` names, such as a jq filter, that zirc never sends: one
/// that isn't in [`ZIRC_KINDS`]. A filter naming one can never pick anything out.
#[must_use]
pub fn unknown_zirc_kind(text: &str) -> Option<&str> {
    text.match_indices("x-zirc/").find_map(|(at, _)| {
        let rest = &text[at..];
        let end = rest
            .char_indices()
            .skip("x-zirc/".len())
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '-'))
            .map_or(rest.len(), |(end, _)| end);
        let kind = &rest[..end];
        (!ZIRC_KINDS.contains(&kind)).then_some(kind)
    })
}

/// The [`Event`] kinds zircd sends on its own, as JSON's `type` says them: bookkeeping about
/// who's changing what, rather than anything someone said ([`Event::is_machine`]).
pub const MACHINE_EVENTS: [&str; 4] = ["intent", "working", "commit", "push"];

/// How many characters of a commit id are shown.
pub const SHORT_SHA: usize = 7;

/// `sha` as shown: its first [`SHORT_SHA`] characters.
#[must_use]
pub fn short_sha(sha: &str) -> &str {
    sha.get(..SHORT_SHA).unwrap_or(sha)
}

/// How many of a push's subjects [`push_line`] names before saying how many more.
const PUSH_LINE_SUBJECTS: usize = 2;

/// What a push did: `pushed login to 0123456`, and how many commits when it pushed more
/// than one, `pushed login to 0123456 (2 commits)`. Every place that says a push starts
/// with this: [`push_line`], and apps' rows, which list the subjects under it.
#[must_use]
pub fn push_lead(branch: &str, head: &str, forced: bool, commits: usize) -> String {
    let verb = if forced { "force-pushed" } else { "pushed" };
    let to = short_sha(head);
    match commits {
        0 | 1 => format!("{verb} {branch} to {to}"),
        n => format!("{verb} {branch} to {to} ({n} commits)"),
    }
}

/// A push in one line, as plain IRC clients, logs and screen readers read it: its
/// [`push_lead`], then its commits' subjects, oldest first, the first few and how many more:
/// `pushed login to 0123456 (3 commits): One; Two and 1 more`.
#[must_use]
pub fn push_line(branch: &str, head: &str, forced: bool, subjects: &[String]) -> String {
    let lead = push_lead(branch, head, forced, subjects.len());
    if subjects.is_empty() {
        return lead;
    }
    let named = subjects[..subjects.len().min(PUSH_LINE_SUBJECTS)].join("; ");
    match subjects.len().saturating_sub(PUSH_LINE_SUBJECTS) {
        0 => format!("{lead}: {named}"),
        more => format!("{lead}: {named} and {more} more"),
    }
}

impl Event {
    /// A team's own event of `kind` that carries nothing but what it says ([`Event::Custom`]).
    #[must_use]
    pub fn custom(kind: impl Into<String>, text: impl Into<String>) -> Self {
        Self::Custom {
            kind: kind.into(),
            data: serde_json::Value::Null,
            text: text.into(),
        }
    }

    /// Its kind, as JSON's `type` says it, such as `text` or `intent`: what a log may say of
    /// it without what anyone wrote.
    #[must_use]
    pub fn kind(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value["type"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// Whether zircd sent it on its own, as bookkeeping about who's changing what, rather
    /// than someone saying it: an intent, a working set, a commit or a push. A log or a
    /// history that's for people leaves these out.
    #[must_use]
    pub fn is_machine(&self) -> bool {
        matches!(
            self,
            Event::Intent { .. }
                | Event::Working { .. }
                | Event::Commit { .. }
                | Event::Push { .. }
        )
    }

    /// The readable text that travels next to the payload.
    #[must_use]
    pub fn human_text(&self) -> String {
        match self {
            Event::Intent { summary, paths, .. } => match paths.as_slice() {
                [] => format!("intends: {summary}"),
                paths => format!("about to change {}: {summary}", working_list(paths)),
            },
            Event::Working { paths, .. } => working_text(paths),
            Event::Commit {
                branch, subject, ..
            } => format!("committed on {branch}: {subject}"),
            Event::Push {
                branch,
                head,
                forced,
                subjects,
                ..
            } => push_line(branch, head, *forced, subjects),
            Event::Text { body } | Event::Custom { text: body, .. } => body.clone(),
            Event::Query { text, .. } => format!("asks: {text}"),
            Event::Answer { text, .. } => format!("answers: {text}"),
            Event::Note { to, text } => format!("notes to {to}: {text}"),
            Event::File(File { url, name, .. }) => format!("shared {name}: {url}"),
        }
    }
}

/// "working on a, b, c and 4 more", short enough to leave the payload most of the line.
fn working_text(paths: &[String]) -> String {
    match paths.len() {
        0 => "has no uncommitted changes".to_owned(),
        _ => format!("working on {}", working_list(paths)),
    }
}

/// "a, b, c and 4 more".
fn working_list(paths: &[String]) -> String {
    const SHOWN: usize = 3;
    match paths.len() {
        n if n <= SHOWN => paths.join(", "),
        n => format!("{} and {} more", paths[..SHOWN].join(", "), n - SHOWN),
    }
}

/// An [`Event`] with who sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields)]
pub struct Message {
    /// The format version, [`VERSION`] when written by this crate.
    pub v: u32,
    /// The agent session or person on the sending machine. The machine itself is the
    /// IRC nick the message arrives from.
    pub session: String,
    /// What happened.
    pub event: Event,
    /// For one of zirc's own integrations, the short name apps show for it in place of a
    /// generic app badge, such as `deploy`: see [`app_label`]. Without it, apps show its
    /// session's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// What people call its session, such as `zirc-chat` or `docs-site`, which apps show
    /// in place of an id like `cc-598aa1`: see [`session_name`]. Without it, they show the
    /// session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The agent tool its session runs in, such as `claude-code` or `codex`, so apps can
    /// show its icon: see [`session_tool`]. Without it, the session is a person's or
    /// runs in a tool zirc doesn't know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The picture its session picked from [`AGENT_AVATARS`], by id, such as `rocket`, which
    /// apps show in place of the tool's icon. Without it, they show the tool's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// Whether it's for every session hearing it, as `zirc post --everyone` says: an agent's
    /// broadcast. Never its text: `@everyone` written in it, as in a quote or a code block,
    /// is only text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub everyone: bool,
    /// What it asks of whoever reads it, as its sender says: see [`Level`]. Without it, the
    /// level its kind and data say ([`Message::level`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    /// The task its session is on, so a task's posts, intents, commits and landing can be
    /// told apart from the rest: the id zircd gives a session when it posts `taking:`
    /// ([`task_id`]), kept until it posts `done:` or `dropped:` ([`says_finished`]). Only
    /// an id, never what the task is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The repository it's about, by name as zirc spells it (`monorepo` for
    /// `git@github.com:zircd/monorepo.git`): every repository's sessions and CI speak in the
    /// organisation's one [`AGENTS_CHANNEL`], so a reader tells them apart by this. Apps show
    /// it only where the organisation has more than one. Without it, it's about none, as a
    /// person's line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// What a post that starts a task starts with ([`says_taking`]).
pub const TAKING: &str = "taking:";

/// Whether `text`, a post, starts a task: it starts `taking:`, whatever the case.
#[must_use]
pub fn says_taking(text: &str) -> bool {
    says(text, TAKING)
}

/// The post that starts `task`: `taking: <task>`, as `zirc intend` posts it for a session
/// on none.
#[must_use]
pub fn taking(task: &str) -> String {
    format!("{TAKING} {}", task.trim())
}

/// The longest a task id is ([`task_id`]).
pub const MAX_TASK_ID: usize = 16;

/// `id` as a task's id when it is one: `t-`, then lowercase letters and digits, at most
/// [`MAX_TASK_ID`] characters in all. `None` for anything else, so a field written by hand
/// can't carry text.
#[must_use]
pub fn task_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("t-")?;
    (!rest.is_empty()
        && id.len() <= MAX_TASK_ID
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
    .then(|| id.to_owned())
}

/// What a line asks of whoever reads it, not how big the change is: the sender says it, so
/// nothing has to guess it from the wording. Apps show a room's lines at or above the level a
/// person picks for it; zircd wakes a session for a line by its level and by how the line
/// concerns the session. Ordered from least to most.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
// `state::Level` is another: how something in the repository stands.
#[ts(rename = "EventLevel")]
#[schemars(rename = "EventLevel")]
pub enum Level {
    /// State that machines need and no person needs to read, such as what a machine is
    /// changing, its commits and pushes, or a check that passed as it does every time.
    Debug,
    /// Something started or finished: worth seeing, needs no one. A session's post, a
    /// landing, a deploy.
    Info,
    /// Something may need someone soon: one session or owner should look. An overlap, a
    /// conflict ahead, a check that didn't run.
    Warn,
    /// Something needs someone now: a build failed, a deploy failed, an alert is firing.
    Error,
}

impl Level {
    /// The level a CI line's state says (`ops/say.sh`'s `$ZIRC_STATE`, [`ci::Data::state`]):
    /// `passed` and `running` are info; `pending`, `warning` and `cancelled` warn; `failed` is
    /// an error. `None` for a state it doesn't know.
    #[must_use]
    pub fn of_state(state: &str) -> Option<Self> {
        match state {
            "passed" | "running" => Some(Self::Info),
            "pending" | "warning" | "cancelled" => Some(Self::Warn),
            "failed" => Some(Self::Error),
            _ => None,
        }
    }

    /// Its name, as it's written: `debug`, `info`, `warn` or `error`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::str::FromStr for Level {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name.to_ascii_lowercase().as_str() {
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" | "warning" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(format!("no level {name:?}: debug, info, warn or error")),
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Message {
    /// A message in the current format.
    #[must_use]
    pub fn new(session: impl Into<String>, event: Event) -> Self {
        Self {
            v: VERSION,
            session: session.into(),
            event,
            app: None,
            name: None,
            tool: None,
            avatar: None,
            everyone: false,
            level: None,
            task: None,
            repo: None,
        }
    }

    /// What a search finds it by, for every search alike (the server's index, a
    /// conversation's loaded messages): its text as people read it ([`Event::human_text`]),
    /// then every value its data carries, one a line, such as a working set's paths, a
    /// push's branch and commits or a CI line's build, and its session, name and task. Never
    /// the fields' names nor its kind's, so searching `paths` or `branch` doesn't find every
    /// message that has one. Its sealed parts travel apart from it ([`sealed::TAG`]), so
    /// none is here.
    #[must_use]
    pub fn search_text(&self) -> String {
        fn values(value: &serde_json::Value, out: &mut Vec<String>) {
            match value {
                serde_json::Value::String(text) if !text.is_empty() => out.push(text.clone()),
                serde_json::Value::Number(number) => out.push(number.to_string()),
                serde_json::Value::Array(items) => items.iter().for_each(|v| values(v, out)),
                serde_json::Value::Object(fields) => fields.values().for_each(|v| values(v, out)),
                _ => {}
            }
        }
        let mut lines = vec![self.event.human_text()];
        if let Ok(serde_json::Value::Object(mut fields)) = serde_json::to_value(&self.event) {
            fields.remove("type");
            values(&serde_json::Value::Object(fields), &mut lines);
        }
        lines.extend(
            [
                &Some(self.session.clone()),
                &self.name,
                &self.task,
                &self.app,
            ]
            .into_iter()
            .flatten()
            .filter(|value| !value.is_empty())
            .cloned(),
        );
        let mut seen = std::collections::HashSet::new();
        lines.retain(|line| seen.insert(line.clone()));
        lines.join("\n")
    }

    /// What it asks of whoever reads it, when its sender or its kind says: the level it
    /// carries; else, for one of CI's lines, the level its state says ([`Level::of_state`]);
    /// else the level of its kind of event ([`Event::level`]). `None` only for a line of
    /// another team's or one of zirc's integrations' that says nothing of how it stands,
    /// which only its words can tell ([`Message::level_or_info`] takes it as info).
    #[must_use]
    pub fn level(&self) -> Option<Level> {
        self.declared_level().or_else(|| self.event.level())
    }

    /// The level it says itself: the one it carries, or, for one of CI's lines, the one its
    /// state says ([`Level::of_state`]). `None` for a line that says neither, as one from a
    /// sender that predates levels, whose level only its kind or its words can tell.
    #[must_use]
    pub fn declared_level(&self) -> Option<Level> {
        self.level.or_else(|| match &self.event {
            Event::Custom { data, .. } => data
                .get("state")
                .and_then(serde_json::Value::as_str)
                .and_then(Level::of_state),
            _ => None,
        })
    }

    /// Whether it's debug ([`Message::level`]), and so only true now: it goes as a `TAGMSG`
    /// with [`EVENT_TAG`], out of every channel's history, and is said again to whoever
    /// joins. Anything a machine may read back later is a record of what happened, at info
    /// or above, kept in history (decided 7 Nov 2026: debug means only true now).
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.level() == Some(Level::Debug)
    }

    /// [`Message::level`], or info when nothing says: as apps show a line.
    #[must_use]
    pub fn level_or_info(&self) -> Level {
        self.level().unwrap_or(Level::Info)
    }
}

impl Event {
    /// The level this kind of event has when its sender says none: what's only true now
    /// ([`live`]: a machine's working set, leases, an intent's renewal) is debug; records of
    /// what happened, which machines read back from history (commits, pushes, intents),
    /// and posts, questions, answers, notes and files are info. zircd's own intents say
    /// debug. `None` for a custom event of any other kind, which only its sender can say.
    #[must_use]
    pub fn level(&self) -> Option<Level> {
        match self {
            Self::Working { .. } => Some(Level::Debug),
            Self::Custom { .. } if live(self) => Some(Level::Debug),
            Self::Custom { .. } => None,
            Self::Intent { .. }
            | Self::Commit { .. }
            | Self::Push { .. }
            | Self::Text { .. }
            | Self::Query { .. }
            | Self::Answer { .. }
            | Self::Note { .. }
            | Self::File(_) => Some(Level::Info),
        }
    }

    /// Whether it says back what someone decided, for the record: a decision, a request
    /// answered or taken as recommended, a gotcha kept or a suggestion muted, a hint added or
    /// removed. It names whoever decided, never to ask them anything, so it's no mention of
    /// them: phones pushed four "mattias kept the gotcha" and four "recommendation taken" to
    /// the person who had just decided, 16-19 Nov 2026.
    #[must_use]
    pub fn says_a_choice(&self) -> bool {
        decision::Decision::of(self).is_some()
            || hints::Changed::of(self).is_some()
            || request::Request::of(self).is_some_and(|asked| asked.state != request::State::Open)
            || suggestions::Suggestion::of(self).is_some_and(|said| {
                matches!(
                    said.state,
                    suggestions::State::Kept | suggestions::State::Muted
                )
            })
    }
}

/// What starts a session's post saying its work is done, as zircd says for it once its
/// landing lands: `done: the login page`.
pub const DONE: &str = "done:";

/// What starts a session's post saying it stopped before finishing: `dropped: releases`.
pub const DROPPED: &str = "dropped:";

/// The labels that start a session's status line: [`TAKING`], [`DONE`], [`DROPPED`].
pub const STATUS: [&str; 3] = [TAKING, DONE, DROPPED];

/// Whether `text` starts with `label`, after any space, whatever the case.
#[must_use]
pub fn says(text: &str, label: &str) -> bool {
    text.trim_start()
        .get(..label.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(label))
}

/// What a post `text` says it's taking on, when it starts [`TAKING`], whatever the case
/// ([`taking`] writes one): `the login page`.
#[must_use]
pub fn taken(text: &str) -> Option<&str> {
    // Whatever the case, as [`says`] reads it: `TAKING` is ASCII, so its length is where
    // the task starts.
    let text = text.trim_start();
    says(text, TAKING).then(|| text[TAKING.len()..].trim())
}

/// Whether `text`, an intent's summary or a post, says its work is done: it starts
/// [`DONE`], whatever the case.
#[must_use]
pub fn says_done(text: &str) -> bool {
    says(text, DONE)
}

/// Whether `text`, an intent's summary or a post, says its work is finished: done
/// ([`says_done`]) or [`DROPPED`], whatever the case. Such an intent ends at once, rather
/// than when it expires, and a claim ends with such a post.
#[must_use]
pub fn says_finished(text: &str) -> bool {
    says_done(text) || says(text, DROPPED)
}

/// `bytes` in lowercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// `text` in at most `max` characters, cut with `…` when it's longer. Line breaks are left as
/// they are: a caller after one line replaces them first.
#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The longest a session's name is: see [`session_name`].
pub const MAX_SESSION_NAME: usize = 24;

/// How a line for one session starts, as CI's do: its id, or its name with the id after it,
/// which people can read and `zirc watch` still wakes on: `cc-1a2b3c: landed …` or
/// `login-page (cc-1a2b3c): landed …`. See [`addressed`].
#[must_use]
pub fn address(session: &str, name: Option<&str>) -> String {
    match name.filter(|name| !name.is_empty() && *name != session) {
        Some(name) => format!("{name} ({session})"),
        None => session.to_owned(),
    }
}

/// The session a line is for, when it starts with one as [`address`] writes it, and what it
/// says after: `("cc-1a2b3c", "landed …")`. `None` for any other line.
#[must_use]
pub fn addressed(text: &str) -> Option<(&str, &str)> {
    let (who, said) = text.split_once(": ")?;
    Some((session_of(who)?, said))
}

/// The session `who` names, as [`address`] writes one: `cc-1a2b3c` or `login-page (cc-1a2b3c)`.
pub(crate) fn session_of(who: &str) -> Option<&str> {
    let session = match who.split_once(" (") {
        Some((name, rest)) if !name.contains(' ') => rest.strip_suffix(')')?,
        Some(_) => return None,
        None => who,
    };
    (!session.is_empty() && !session.contains([' ', '(', ')'])).then_some(session)
}

/// What a line says after whom it's for, be that one session ([`addressed`]) or several, as
/// CI names every session whose landing one build of the Apple apps covers:
/// `cc-1 (apple/login), cc-2 (apple/rail): build 7's Apple apps compile on 1a2b3c4`. The
/// whole line when it doesn't start with whom it's for. Reading only the first of several
/// took such a line for one that says something else, and woke each session it named
/// (7 lines, 19 names, in the week to 13 Nov 2026).
#[must_use]
pub fn said(text: &str) -> &str {
    match text.split_once(": ") {
        Some((who, said)) if who.split(", ").all(|one| session_of(one).is_some()) => said,
        _ => text,
    }
}

/// `name` as a session's name, as apps show it in place of its id: lowercase letters,
/// digits, `-`, `_` and `.`, at most [`MAX_SESSION_NAME`] characters, spaces becoming `-`.
/// `None` when nothing is left.
#[must_use]
pub fn session_name(name: &str) -> Option<String> {
    plain(name, MAX_SESSION_NAME)
}

/// Claude Code, as [`Message::tool`] says it.
pub const CLAUDE_CODE: &str = "claude-code";

/// Codex, as [`Message::tool`] says it.
pub const CODEX: &str = "codex";

/// `tool` as the agent tool a session runs in, plain as [`session_name`] makes a name:
/// [`CLAUDE_CODE`], [`CODEX`], or another tool's own short name. `None` when nothing is
/// left.
#[must_use]
pub fn session_tool(tool: &str) -> Option<String> {
    plain(tool, MAX_SESSION_NAME)
}

/// The pictures an agent session can go by ([`Message::avatar`]), by id, each with what it
/// shows, in the order apps offer them: a bot dressed for each role an agent plays, and
/// [`DEFAULT_ROLE`]'s plain one. Apps draw each one; nobody uploads a picture.
pub const AGENT_AVATARS: [(&str, &str); 17] = [
    (
        DEFAULT_ROLE,
        "a plain bot, for an agent whose role isn't said",
    ),
    ("reviewer", "a bot in glasses, for reviewing changes"),
    ("fixer", "a bot with a wrench, for fixing what's broken"),
    ("builder", "a bot in a hard hat, for building features"),
    ("tester", "a bot with a flask, for tests"),
    ("docs", "a bot with a quill, for docs and writing"),
    ("designer", "a bot in a beret, for design"),
    ("ops", "a bot in a headset, for ops, deploys and alerts"),
    ("security", "a bot with a shield, for security"),
    ("data", "a bot with a chart, for data"),
    ("release", "a bot with a flag, for releases"),
    ("researcher", "a bot with a magnifier, for research"),
    (
        "coordinator",
        "a bot with a megaphone, for coordinating others",
    ),
    ("frontend", "a bot with </> for a face, for the frontend"),
    ("backend", "a bot with servers, for the backend"),
    ("mobile", "a bot with a phone, for mobile apps"),
    (
        "performance",
        "a bot with a lightning bolt, for performance",
    ),
];

/// The plain bot of [`AGENT_AVATARS`], for an agent whose role is neither said nor told by
/// its name.
pub const DEFAULT_ROLE: &str = "agent";

/// The pictures agent sessions went by before roles, still taken from older zircs and
/// `--avatar`, and drawn as the role its name tells ([`agent_picture`]).
const OLD_AVATARS: [&str; 24] = [
    "cat",
    "dog",
    "bird",
    "fish",
    "rabbit",
    "squirrel",
    "turtle",
    "snail",
    "bug",
    "ghost",
    "rocket",
    "robot",
    "anchor",
    "compass",
    "telescope",
    "atom",
    "gem",
    "coffee",
    "leaf",
    "flame",
    "bolt",
    "moon",
    "star",
    "mountain",
];

/// The words in a session's name that tell its role, for one that doesn't say it: the first
/// word of its name found here decides ([`guess_role`]).
const ROLE_WORDS: [(&str, &[&str]); 16] = [
    ("reviewer", &["review", "reviewer", "reviews", "pr"]),
    (
        "fixer",
        &[
            "fix", "fixer", "fixes", "hotfix", "bug", "bugs", "flake", "flaky", "repair",
        ],
    ),
    (
        "builder",
        &[
            "build",
            "builder",
            "feature",
            "implement",
            "scaffold",
            "developer",
            "engineer",
            "coder",
        ],
    ),
    (
        "tester",
        &[
            "test", "tests", "tester", "testing", "e2e", "qa", "spec", "specs",
        ],
    ),
    (
        "docs",
        &["doc", "docs", "readme", "guide", "writing", "blog"],
    ),
    (
        "designer",
        &[
            "design", "designer", "ux", "logo", "icon", "icons", "avatars", "theme",
        ],
    ),
    (
        "ops",
        &[
            "ops", "deploy", "infra", "k8s", "cluster", "oncall", "alert", "alerts", "ci",
        ],
    ),
    (
        "security",
        &[
            "security", "auth", "secret", "secrets", "audit", "vuln", "sealed",
        ],
    ),
    (
        "data",
        &[
            "data",
            "sql",
            "db",
            "database",
            "migration",
            "analytics",
            "metrics",
            "insights",
        ],
    ),
    (
        "release",
        &[
            "release",
            "releases",
            "ship",
            "publish",
            "version",
            "changelog",
            "store",
        ],
    ),
    (
        "researcher",
        &[
            "research",
            "researcher",
            "explore",
            "investigate",
            "spike",
            "study",
        ],
    ),
    (
        "coordinator",
        &["coordinator", "coord", "operator", "lead", "triage", "plan"],
    ),
    (
        "frontend",
        &["frontend", "web", "css", "react", "ui", "landing"],
    ),
    (
        "backend",
        &[
            "backend",
            "api",
            "server",
            "controller",
            "sidecar",
            "daemon",
            "zircd",
        ],
    ),
    (
        "mobile",
        &["mobile", "ios", "android", "iphone", "phone", "phones"],
    ),
    (
        "performance",
        &[
            "perf",
            "performance",
            "speed",
            "latency",
            "cache",
            "fast",
            "slow",
        ],
    ),
];

/// `id` as a picture an agent session may go by, whatever its case: one of
/// [`AGENT_AVATARS`]' roles or a picture from before roles; `None` for one that's neither.
#[must_use]
pub fn agent_avatar(id: &str) -> Option<&'static str> {
    let id = id.trim();
    AGENT_AVATARS
        .iter()
        .map(|(avatar, _)| *avatar)
        .chain(OLD_AVATARS)
        .find(|avatar| avatar.eq_ignore_ascii_case(id))
}

/// The role a session's `name` tells, by its first word that names one, such as `fixer`
/// for `fix-ci`: `None` when none does.
#[must_use]
pub fn guess_role(name: &str) -> Option<&'static str> {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .find_map(|word| {
            ROLE_WORDS.iter().find_map(|(role, words)| {
                words
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(word))
                    .then_some(*role)
            })
        })
}

/// The picture of [`AGENT_AVATARS`] an agent session is drawn with: the role its `avatar`
/// says, else the one its `name` tells ([`guess_role`]), else [`DEFAULT_ROLE`]'s. A picture
/// from before roles says none.
#[must_use]
pub fn agent_picture(avatar: Option<&str>, name: Option<&str>) -> &'static str {
    avatar
        .and_then(agent_avatar)
        .filter(|id| !OLD_AVATARS.contains(id) && *id != DEFAULT_ROLE)
        .or_else(|| name.and_then(guess_role))
        .unwrap_or(DEFAULT_ROLE)
}

/// `text` in lowercase letters, digits, `-`, `_` and `.`, at most `max` of them, spaces
/// becoming `-`.
fn plain(text: &str, max: usize) -> Option<String> {
    let plain: String = text
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(max)
        .collect();
    let plain = plain.trim_matches('-');
    (!plain.is_empty()).then(|| plain.to_owned())
}

/// The longest an integration's short name is: see [`app_label`].
pub const MAX_APP_LABEL: usize = 12;

/// `name` as an integration's short name, as apps show it, `[ci]` or a `CI` badge:
/// lowercase letters, digits and `-`, at most [`MAX_APP_LABEL`] characters. `None` when
/// nothing is left.
#[must_use]
pub fn app_label(name: &str) -> Option<String> {
    let label: String = name
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(MAX_APP_LABEL)
        .collect();
    let label = label.trim_matches('-').to_owned();
    (!label.is_empty()).then_some(label)
}

/// Why a body couldn't be written or read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The encoded body is longer than [`MAX_BODY_BYTES`].
    #[error("message is {0} bytes, more than fits")]
    TooLong(usize),
    /// The payload isn't valid base64.
    #[error("payload isn't base64: {0}")]
    Base64(#[from] base64::DecodeError),
    /// The payload isn't a [`Message`].
    #[error("payload isn't a zirc message: {0}")]
    Json(#[from] serde_json::Error),
    /// The message was written by a newer zirc.
    #[error("message is version {0}, this zirc reads up to {VERSION}")]
    UnknownVersion(u32),
}

/// The PRIVMSG body that carries `message`.
///
/// # Errors
///
/// [`Error::TooLong`] when the body wouldn't fit on an IRC line.
pub fn encode(message: &Message) -> Result<String, Error> {
    encode_up_to(message, MAX_BODY_BYTES)
}

/// The PRIVMSG body that carries `message`, if it's at most `max_bytes` long: for a body
/// sent as an IRCv3 multiline message, which can be longer than a line.
///
/// # Errors
///
/// [`Error::TooLong`] when the body is longer than `max_bytes`.
pub fn encode_up_to(message: &Message, max_bytes: usize) -> Result<String, Error> {
    framed(message, max_bytes, false)
}

/// The same, keeping the line breaks in what it says for people: for a body sent as an
/// IRCv3 multiline message a line each, the payload on the first and `\x01` closing the
/// last, which plain IRC clients show as the lines they are.
///
/// # Errors
///
/// [`Error::TooLong`] when the body is longer than `max_bytes`.
pub fn encode_lines_up_to(message: &Message, max_bytes: usize) -> Result<String, Error> {
    framed(message, max_bytes, true)
}

fn framed(message: &Message, max_bytes: usize, lines: bool) -> Result<String, Error> {
    let json = serde_json::to_vec(message)?;
    let human = message.event.human_text();
    let human = if lines {
        human
            .replace('\r', "")
            .replace(CTCP, " ")
            .trim_end()
            .to_owned()
    } else {
        human.replace(['\r', '\n', CTCP], " ")
    };
    let body = format!(
        "{ACTION_PREFIX}{TAG} {} :{human}{CTCP}",
        BASE64.encode(json)
    );
    match body.len() {
        len if len > max_bytes => Err(Error::TooLong(len)),
        _ => Ok(body),
    }
}

/// Whether a channel message's `body` is something a person would read: anything but a
/// session's title card ([`session::KIND`]). Plain text, actions, notices, zirc's events and
/// a body that can't be read all count, so a reader deciding a channel was never used, such
/// as a janitor of empty threads, errs towards keeping it.
#[must_use]
pub fn worth_reading(body: &str) -> bool {
    !matches!(decode(body), Ok(Some(Message { event: Event::Custom { ref kind, .. }, .. })) if kind == session::KIND)
}

/// Every room a repository had under its channel before 20 Nov 2026, `#<repo>/<room>`, when
/// its agents spoke there rather than in [`AGENTS_CHANNEL`]: no longer made or read. The
/// first three are the [`STANDING_ROOMS`]; the rest were threads like any other.
pub const REPOSITORY_ROOMS: &[&str] = &[
    AGENTS_ROOM,
    "landings",
    "gotchas",
    "ci",
    "apps",
    "flakes",
    "ops",
];

/// The [`REPOSITORY_ROOMS`] still never counted as threads, left as they were, history and
/// all, so the server's janitor of unused threads leaves them be.
pub const STANDING_ROOMS: &[&str] = REPOSITORY_ROOMS.split_at(3).0;

/// The organisation's feeds, where systems post rather than people talk: its
/// [`AGENTS_CHANNEL`], gotchas room, `#ops` and `#decisions`. Every app draws them as feeds
/// (zirc-chat's `is_feed`), and the server never purges them.
pub const FEEDS: &[&str] = &[
    AGENTS_CHANNEL,
    gotchas::CHANNEL,
    state::CHANNEL,
    request::CHANNEL,
];

/// The header (IRC topic) of a room whose job is the same in every tenant, which every app
/// shows under the room's name: the organisation's [`AGENTS_CHANNEL`], gotchas room, `#ops`
/// or `#decisions`. `None` for any other channel, which its people name. Said once, here:
/// the tenant's sidecar sets it on a room that has no topic yet and never replaces one a
/// person set, and the agent guide's table of rooms says the same in longer words. Plain
/// text: a topic has no Markdown.
#[must_use]
pub fn room_header(channel: &str) -> Option<&'static str> {
    match channel.to_ascii_lowercase().as_str() {
        AGENTS_CHANNEL => Some(
            "Agents' work in every repository, and CI's. A line wakes only the session it names",
        ),
        gotchas::CHANNEL => {
            Some("Lessons sessions learnt about paths. zirc gotchas <path> lists them")
        }
        state::CHANNEL => Some("Alerts and a broken master: what needs someone now"),
        request::CHANNEL => Some("Decisions made and questions waiting for a yes or no"),
        _ => None,
    }
}

/// The room each repository's agents spoke in before 20 Nov 2026, `#<repo>/agents`
/// ([`STANDING_ROOMS`]).
pub const AGENTS_ROOM: &str = "agents";

/// Whether `channel` is a thread, `#<parent>/<name>` with text both sides of the `/`, and not
/// one of the [`STANDING_ROOMS`].
#[must_use]
pub fn is_thread(channel: &str) -> bool {
    channel.split_once('/').is_some_and(|(parent, name)| {
        parent.len() > 1
            && !name.is_empty()
            && !STANDING_ROOMS
                .iter()
                .any(|room| room.eq_ignore_ascii_case(name))
    })
}

/// What a `/me` says, `\x01ACTION <text>\x01`, if `body` is one and not a zirc message.
#[must_use]
pub fn action(body: &str) -> Option<&str> {
    let action = body.strip_prefix(ACTION_PREFIX)?;
    let action = action.strip_suffix(CTCP).unwrap_or(action);
    (!action
        .strip_prefix(TAG)
        .is_some_and(|rest| rest.starts_with(' ')))
    .then_some(action)
}

/// A PRIVMSG body as people read it: a zirc message's own text, without its framing, whatever
/// its version; a `/me` as `* <text>`; anything else as it is.
#[must_use]
pub fn readable(body: &str) -> String {
    if let Some(action) = action(body) {
        return format!("* {action}");
    }
    let Some(zirc) = body
        .strip_prefix(ACTION_PREFIX)
        .and_then(|rest| rest.strip_prefix(TAG))
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return body.to_owned();
    };
    let zirc = zirc.strip_suffix(CTCP).unwrap_or(zirc);
    zirc.split_once(" :")
        .map_or_else(String::new, |(_, text)| text.to_owned())
}

/// The framing's and machine events' rules as TypeScript constants, and a message's types,
/// for the web app's generated file: what it needs to write a session's lines as zircd does
/// (the review sandbox's, web/src/lib/sandbox.ts), as [`encode`] frames them.
#[must_use]
pub fn frame_typescript(config: &ts_rs::Config) -> Vec<String> {
    let kinds: Vec<String> = MACHINE_EVENTS
        .iter()
        .map(|kind| format!("{kind:?}"))
        .collect();
    vec![
        format!("const ZIRC_FRAME_PATTERN = {:?};", frame_pattern()),
        format!(
            "const MACHINE_EVENTS: readonly string[] = [{}];",
            kinds.join(", ")
        ),
        format!("const ZIRC_TAG = {TAG:?};"),
        format!("const ZIRC_VERSION = {VERSION};"),
        format!("const ZIRC_MAX_BODY_BYTES = {MAX_BODY_BYTES};"),
        format!(
            "const AGENT_AVATARS: readonly string[] = [{}];",
            AGENT_AVATARS.map(|(id, _)| format!("{id:?}")).join(", ")
        ),
        format!("const SESSION_CARD_KIND = {:?};", session::KIND),
        format!("const DECISION_REQUEST_KIND = {:?};", request::KIND),
        format!("const DECISION_REQUEST_CHANNEL = {:?};", request::CHANNEL),
        <serde_json::Value as TS>::decl(config),
        Message::decl(config),
        Event::decl(config),
        Level::decl(config),
        session::SessionCard::decl(config),
        request::Request::decl(config),
        request::Answer::decl(config),
        request::State::decl(config),
        request::Withdrawal::decl(config),
        card::Card::decl(config),
        card::Field::decl(config),
        card::Link::decl(config),
        card::Step::decl(config),
        card::Image::decl(config),
        card::Action::decl(config),
        card::Who::decl(config),
        card::State::decl(config),
    ]
}

/// A zirc message's framing as a regular expression, for languages that look for one that
/// way: its payload is the first group.
#[must_use]
pub fn frame_pattern() -> String {
    format!("\\x01ACTION {TAG} ([A-Za-z0-9+/=]+) :")
}

/// The message a PRIVMSG body carries, or `None` when the body isn't zirc's.
///
/// # Errors
///
/// When the body is marked as zirc's but its payload can't be read.
pub fn decode(body: &str) -> Result<Option<Message>, Error> {
    let Some(rest) = body
        .strip_prefix(ACTION_PREFIX)
        .and_then(|rest| rest.strip_prefix(TAG))
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return Ok(None);
    };
    let payload = rest.split([' ', CTCP]).next().unwrap_or_default();
    from_payload(payload).map(Some)
}

/// How many characters of an id zirc shows, as in `[id 01jbx7ab]` and `[hint 0123abcd]`: what
/// `zirc react`, `zirc gotcha --retire`, `zirc hints remove` and the like take.
pub const SHORT_ID: usize = 8;

/// The start of `id`, as zirc shows it ([`SHORT_ID`]).
#[must_use]
pub fn short_id(id: &str) -> &str {
    id.get(..SHORT_ID).unwrap_or(id)
}

/// The custom event kind leases are agreed through (zirc-core's `leases`).
pub const LEASE_KIND: &str = "x-zirc/lease";

/// The custom event kind a machine says a branch gone from origin unlanded with, when
/// another session had said its own was built on it: its data is `{"branch": …}`, and each
/// zircd with a session stacked on that branch tells it what to do.
pub const DROPPED_KIND: &str = "x-zirc/dropped";

/// The custom event kind an intent's renewal goes as ([`renewal`]).
pub const RENEWAL_KIND: &str = "x-zirc/intent-renewed";

/// `intent` said again only to keep it going, as zircd renews the intents it says for a
/// session by itself: an [`RENEWAL_KIND`] event carrying it, which is [`live`], so it's no
/// line of history or chat and wakes nobody, while readers keep the intent going as if
/// it were said again ([`renewed`]). Readers that predate it drop it, and the intent
/// ends for them when it expires. `None` for any other event.
#[must_use]
pub fn renewal(intent: &Event) -> Option<Event> {
    let Event::Intent { summary, .. } = intent else {
        return None;
    };
    Some(Event::Custom {
        kind: RENEWAL_KIND.to_owned(),
        data: serde_json::to_value(intent).ok()?,
        text: format!("still {summary}"),
    })
}

/// The intent a [`renewal`] carries, or `None` for any other event.
#[must_use]
pub fn renewed(event: &Event) -> Option<Event> {
    let Event::Custom { kind, data, .. } = event else {
        return None;
    };
    if kind != RENEWAL_KIND {
        return None;
    }
    serde_json::from_value(data.clone())
        .ok()
        .filter(|intent| matches!(intent, Event::Intent { .. }))
}

/// Whether `event` says only what's true now, and so goes as a `TAGMSG` with
/// [`EVENT_TAG`], out of every channel's history: a machine's working set, what its
/// sessions want, release or extend of a lease, an intent's [`renewal`], and which of its
/// sessions are live ([`session::Live`]).
#[must_use]
pub fn live(event: &Event) -> bool {
    match event {
        Event::Working { .. } => true,
        Event::Custom { kind, .. } => {
            kind == LEASE_KIND
                || kind == RENEWAL_KIND
                || kind == lines::KIND
                || kind == session::LIVE_KIND
                || activity::KINDS.contains(&kind.as_str())
        }
        _ => false,
    }
}

/// The part number of a working set that's one worktree's alone, said between a machine's
/// whole sets ([`Event::Working`]).
pub const ALONE: [u32; 2] = [0, 0];

/// The client tag a zirc event goes in when it's sent as an IRCv3 `TAGMSG` rather than a
/// PRIVMSG: for what's only true now, such as what a machine is working on, which servers
/// keep out of history (Ergo keeps only the tags its `tagmsg-storage` lists) and plain IRC
/// clients never show. Its value is the payload a PRIVMSG body carries ([`payload`]).
pub const EVENT_TAG: &str = "+zirc/event";

/// `message` as [`EVENT_TAG`]'s value: its JSON in base64, as in a PRIVMSG body.
///
/// # Errors
///
/// When it can't be written as JSON.
pub fn payload(message: &Message) -> Result<String, Error> {
    Ok(BASE64.encode(serde_json::to_vec(message)?))
}

/// The message a payload carries, from a PRIVMSG body or an [`EVENT_TAG`].
///
/// # Errors
///
/// When it can't be read, or a newer zirc wrote it.
pub fn from_payload(payload: &str) -> Result<Message, Error> {
    let message: Message = serde_json::from_slice(&BASE64.decode(payload)?)?;
    match message.v {
        v if v > VERSION => Err(Error::UnknownVersion(v)),
        _ => Ok(message),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_push_is_said_from_one_lead() {
        let subjects = |n: usize| -> Vec<String> { (1..=n).map(|i| format!("S{i}")).collect() };
        let line = |n| super::push_line("login", "0123456789", false, &subjects(n));
        assert_eq!(line(0), "pushed login to 0123456");
        assert_eq!(line(1), "pushed login to 0123456: S1");
        assert_eq!(line(2), "pushed login to 0123456 (2 commits): S1; S2");
        assert_eq!(
            line(5),
            "pushed login to 0123456 (5 commits): S1; S2 and 3 more"
        );
        assert_eq!(
            super::push_lead("login", "0123456789", true, 2),
            "force-pushed login to 0123456 (2 commits)"
        );
    }

    #[test]
    fn a_line_for_a_session_says_its_name_and_keeps_its_id() {
        assert_eq!(
            super::address("cc-1", Some("login-page")),
            "login-page (cc-1)"
        );
        assert_eq!(super::address("cc-1", None), "cc-1");
        assert_eq!(super::address("cc-1", Some("cc-1")), "cc-1");
        assert_eq!(
            super::addressed("login-page (cc-1): landed login as 1a2b3c4"),
            Some(("cc-1", "landed login as 1a2b3c4"))
        );
        assert_eq!(super::addressed("cc-1: landed"), Some(("cc-1", "landed")));
        assert_eq!(super::addressed("master is broken: x"), None);
        assert_eq!(super::addressed("a b (cc-1): x"), None);
        assert_eq!(super::addressed("no address here"), None);
        // What it says, after one session or several.
        assert_eq!(super::said("cc-1: landed"), "landed");
        assert_eq!(
            super::said("cc-1 (apple/login), ada-mbp (apple/rail): build 7's Apple apps compile"),
            "build 7's Apple apps compile"
        );
        assert_eq!(super::said("master is broken: x"), "master is broken: x");
        assert_eq!(super::said("cc-1, see above: x"), "cc-1, see above: x");
        assert_eq!(super::said("no address here"), "no address here");
    }

    use super::*;

    #[test]
    fn a_search_finds_a_message_by_its_data_but_not_its_field_names() {
        let event: Event = serde_json::from_value(serde_json::json!({
            "type": "push",
            "branch": "login-page",
            "head": "0123456789abcdef",
            "forced": false,
            "subjects": ["Sign in with a passkey"],
        }))
        .unwrap();
        let mut message = Message::new("cc-1", event);
        message.task = Some("t-abc".into());
        let text = message.search_text();
        assert!(text.starts_with(&message.event.human_text()));
        for wanted in ["0123456789abcdef", "login-page", "t-abc", "cc-1", "passkey"] {
            assert!(text.contains(wanted), "{wanted} in {text}");
        }
        for unwanted in ["branch", "subjects", "push\n", "type"] {
            assert!(!text.contains(unwanted), "{unwanted} in {text}");
        }
    }

    #[test]
    fn a_task_starts_with_taking_and_its_id_carries_no_text() {
        assert!(says_taking("  Taking: the recorder"));
        assert!(!says_taking("done: the recorder"));
        assert_eq!(taking(" the recorder "), "taking: the recorder");
        assert!(says_taking(&taking("the recorder")));
        assert_eq!(task_id("t-mh3k2a9x1q"), Some("t-mh3k2a9x1q".into()));
        for not in ["t-", "x-abc", "t-ABC", "t-a b", "t-0123456789abcdef"] {
            assert_eq!(task_id(not), None, "{not}");
        }
        let mut message = Message::new("cc-1", Event::Text { body: "hi".into() });
        message.task = Some("t-1".into());
        let back: Message =
            serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
        assert_eq!(back.task.as_deref(), Some("t-1"));
    }

    #[test]
    fn the_organisations_agents_gotchas_ops_and_decisions_have_a_header() {
        for channel in ["#agents", "#Agents", "#gotchas", "#ops", "#decisions"] {
            assert!(room_header(channel).is_some(), "{channel}");
        }
        // Everything else is named by its people: a thread, #general, and the rooms
        // repositories had before.
        for channel in [
            "#monorepo",
            "#monorepo/agents",
            "#monorepo/landings",
            "#general",
            "#monorepo/web-c26b2cc",
            "#monorepo/ops",
            "#/agents",
        ] {
            assert_eq!(room_header(channel), None, "{channel}");
        }
        // An IRC topic is one line of plain text, kept short enough to read in a header.
        for channel in ["#agents", "#gotchas", "#ops", "#decisions"] {
            let header = room_header(channel).unwrap();
            assert!(
                header.len() <= 100 && !header.contains(['\n', '*', '`']),
                "{header}"
            );
        }
    }

    #[test]
    fn a_thread_is_neither_a_channel_nor_a_standing_room() {
        assert!(is_thread("#monorepo/web-c26b2cc"));
        assert!(!is_thread("#monorepo"));
        assert!(!is_thread("#monorepo/agents"));
        assert!(!is_thread("#monorepo/Landings"));
        // The organisation's #ops is the ops room; a repository's is a thread like any other.
        assert!(is_thread("#monorepo/ops"));
        assert!(!is_thread("#/x"));
        assert!(!is_thread("#monorepo/"));
    }

    #[test]
    fn everything_but_a_session_card_is_worth_reading() {
        let card = encode(&Message::new(
            "cc-1",
            Event::Custom {
                kind: session::KIND.into(),
                data: serde_json::Value::Null,
                text: "cc-1 works on web".into(),
            },
        ))
        .unwrap();
        assert!(!worth_reading(&card));
        let text = encode(&Message::new("cc-1", Event::Text { body: "hi".into() })).unwrap();
        assert!(worth_reading(&text));
        assert!(worth_reading("plain words"));
        assert!(worth_reading("\x01ACTION waves\x01"));
    }

    #[test]
    fn a_note_names_its_session_and_reads_as_one() {
        let note = Message::new(
            "cc-1",
            Event::Note {
                to: "coordinator".into(),
                text: "landed the login page".into(),
            },
        );
        let line = encode(&note).unwrap();
        assert!(line.contains("notes to coordinator: landed the login page"));
        assert_eq!(decode(&line).unwrap(), Some(note));
    }

    #[test]
    fn bazel_names_the_product_as_name_does() {
        let brand = include_str!("../../../bazel/brand.bzl");
        assert!(
            brand.contains(&format!("PRODUCT_NAME = {NAME:?}")),
            "oss/bazel/brand.bzl's PRODUCT_NAME isn't {NAME:?}"
        );
    }

    #[test]
    fn only_what_is_true_now_is_live() {
        let custom = |kind: &str| Event::Custom {
            kind: kind.into(),
            data: serde_json::Value::Null,
            text: String::new(),
        };
        assert!(live(&custom(LEASE_KIND)));
        assert!(live(&Event::Working {
            paths: Vec::new(),
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        }));
        assert!(!live(&custom(ci::LANDING_KIND)));
        assert!(!live(&Event::Text { body: "hi".into() }));
    }

    #[test]
    fn a_messages_level_is_what_it_says_else_its_state_else_its_kind() {
        let text = || Event::Text { body: "hi".into() };
        let custom = |kind: &str| Event::Custom {
            kind: kind.into(),
            data: serde_json::Value::Null,
            text: String::new(),
        };
        let ci = |state: &str| Event::Custom {
            kind: ci::LANDING_KIND.into(),
            data: serde_json::json!({ "state": state }),
            text: "cc-1: landed login".into(),
        };
        // Said outright, it wins, and goes and comes back as a word.
        let warned = Message {
            level: Some(Level::Warn),
            ..Message::new("cc-1", ci("passed"))
        };
        assert_eq!(warned.level(), Some(Level::Warn));
        let back = decode(&encode(&warned).unwrap()).unwrap().unwrap();
        assert_eq!(back.level, Some(Level::Warn));
        assert!(
            serde_json::to_string(&warned)
                .unwrap()
                .contains(r#""level":"warn""#)
        );
        // CI's state.
        assert_eq!(Message::new("ci", ci("passed")).level(), Some(Level::Info));
        assert_eq!(Message::new("ci", ci("pending")).level(), Some(Level::Warn));
        assert_eq!(Message::new("ci", ci("failed")).level(), Some(Level::Error));
        assert_eq!(
            Message::new("ci", ci("failed")).declared_level(),
            Some(Level::Error)
        );
        // Its kind.
        assert_eq!(Message::new("cc-1", text()).level(), Some(Level::Info));
        assert_eq!(Message::new("cc-1", text()).declared_level(), None);
        let commit = Event::Commit {
            branch: "main".into(),
            sha: "abc".into(),
            subject: "Fix".into(),
            paths: Vec::new(),
            repo: None,
        };
        assert_eq!(Message::new("cc-1", commit).level(), Some(Level::Info));
        assert_eq!(
            Message::new("cc-1", custom(LEASE_KIND)).level(),
            Some(Level::Debug)
        );
        // Debug is exactly what's only true now: it goes live, and nothing else does.
        for event in [
            text(),
            custom(LEASE_KIND),
            custom(RENEWAL_KIND),
            custom(ci::LANDING_KIND),
        ] {
            assert_eq!(
                event.level() == Some(Level::Debug),
                live(&event),
                "{event:?}"
            );
            assert_eq!(Message::new("cc-1", event.clone()).is_live(), live(&event));
        }
        let auto = Message {
            level: Some(Level::Debug),
            ..Message::new(
                "cc-1",
                Event::Intent {
                    summary: "changing files here (said by zircd)".into(),
                    paths: vec!["web".into()],
                    expires: None,
                    lasts: Some(1800),
                },
            )
        };
        assert!(auto.is_live());
        assert!(
            !Message {
                level: None,
                ..auto
            }
            .is_live()
        );
        // Another team's kind says nothing, and shows as info.
        let theirs = Message::new("cc-1", custom("x-acme/deploy"));
        assert_eq!(theirs.level(), None);
        assert_eq!(theirs.level_or_info(), Level::Info);
        // One without a level is written without one, as before levels.
        assert!(!serde_json::to_string(&theirs).unwrap().contains("level"));
        assert!(Level::Debug < Level::Info && Level::Warn < Level::Error);
        assert_eq!("warning".parse::<Level>(), Ok(Level::Warn));
        assert_eq!(Level::Error.to_string(), "error");
        assert!("loud".parse::<Level>().is_err());
    }

    #[test]
    fn a_status_line_is_read_by_its_label_whatever_the_case() {
        assert_eq!(taken("  taking:  the login page "), Some("the login page"));
        assert_eq!(taken(&taking("the login page")), Some("the login page"));
        assert_eq!(taken("on it"), None);
        // Whatever the case, as says_taking counts it.
        assert_eq!(taken("Taking: the login page"), Some("the login page"));
        assert_eq!(taken("TAKING:x"), Some("x"));
        assert!(says_taking("Taking: it"));
        assert!(says_done("Done: it") && says_done(" done: it"));
        assert!(!says_done("dropped: it") && !says_done("undone: it"));
        assert!(says_finished("DROPPED: it") && says_finished("done: it"));
        assert!(!says_finished("taking: it"));
    }

    #[test]
    fn a_renewal_is_live_and_carries_its_intent() {
        let intent = Event::Intent {
            summary: "changing files here (said by zircd)".into(),
            paths: vec!["src".into()],
            expires: Some(1800),
            lasts: Some(1800),
        };
        let renewing = renewal(&intent).unwrap();
        assert!(live(&renewing));
        assert_eq!(renewed(&renewing), Some(intent));
        // Through a line and back, as other machines read it.
        let message = Message::new("cc-1", renewing);
        let read = from_payload(&payload(&message).unwrap()).unwrap();
        assert!(renewed(&read.event).is_some());
        // Nothing else is one.
        assert_eq!(renewal(&Event::Text { body: "hi".into() }), None);
        assert_eq!(renewed(&Event::Text { body: "hi".into() }), None);
        let other = Event::Custom {
            kind: RENEWAL_KIND.into(),
            data: serde_json::json!({ "type": "text", "body": "hi" }),
            text: String::new(),
        };
        assert_eq!(renewed(&other), None);
    }

    #[test]
    fn reads_any_body_as_people_do() {
        let commit = Message::new(
            "cc-1",
            Event::Commit {
                branch: "main".into(),
                sha: "abc".into(),
                subject: "Fix".into(),
                paths: Vec::new(),
                repo: None,
            },
        );
        let body = encode(&commit).unwrap();
        assert_eq!(readable(&body), "committed on main: Fix");
        assert_eq!(action(&body), None);
        assert_eq!(readable("\x01ACTION waves\x01"), "* waves");
        assert_eq!(action("\x01ACTION waves\x01"), Some("waves"));
        assert_eq!(readable("hello"), "hello");
        // A newer zirc's message still reads as its text, unparsed.
        assert_eq!(readable("\x01ACTION ZIRC e30= :hi there\x01"), "hi there");
        assert!(frame_pattern().contains("ACTION ZIRC"));
    }

    #[test]
    fn machine_events_are_the_bookkeeping_kinds() {
        let commit = Event::Commit {
            branch: "main".into(),
            sha: "abc".into(),
            subject: "Fix".into(),
            paths: Vec::new(),
            repo: None,
        };
        let working = Event::Working {
            paths: Vec::new(),
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        };
        for event in [commit, working] {
            assert!(event.is_machine());
            let kind = serde_json::to_value(&event).unwrap()["type"].clone();
            assert!(MACHINE_EVENTS.contains(&kind.as_str().unwrap()), "{kind}");
        }
        assert!(!Event::Text { body: "hi".into() }.is_machine());
    }

    #[test]
    fn an_agent_is_drawn_as_the_role_it_says_else_its_name_tells_else_a_plain_bot() {
        assert_eq!(agent_picture(Some("tester"), Some("fix-ci")), "tester");
        assert_eq!(agent_picture(None, Some("fix-ci")), "fixer");
        assert_eq!(
            agent_picture(Some("rocket"), Some("android-shots")),
            "mobile"
        );
        assert_eq!(agent_picture(Some("agent"), Some("web-e2e")), "frontend");
        assert_eq!(agent_picture(None, Some("previews")), DEFAULT_ROLE);
        assert_eq!(agent_picture(None, None), DEFAULT_ROLE);
        assert_eq!(guess_role("Review_PR-42"), Some("reviewer"));
        // Every role a name can tell is one apps draw.
        for (role, _) in ROLE_WORDS {
            assert!(AGENT_AVATARS.iter().any(|(id, _)| *id == role), "{role}");
        }
    }

    #[test]
    fn tells_a_kind_zirc_never_sends() {
        let push = r#"select(.message.event.kind == "x-zirc/push")"#;
        assert_eq!(unknown_zirc_kind(push), Some("x-zirc/push"));
        let landing = r#"select(.message.event.kind == "x-zirc/landing" and (.x | test("a")))"#;
        assert_eq!(unknown_zirc_kind(landing), None);
        assert_eq!(unknown_zirc_kind(r#".kind == "x-acme/deploy""#), None);
        // Every kind is told apart, none twice.
        let kinds: std::collections::BTreeSet<_> = ZIRC_KINDS.iter().collect();
        assert_eq!(kinds.len(), ZIRC_KINDS.len());
        assert!(
            ZIRC_KINDS
                .iter()
                .all(|kind| unknown_zirc_kind(kind).is_none())
        );
    }

    #[test]
    fn a_session_name_is_short_and_plain() {
        assert_eq!(session_name("zirc-chat").as_deref(), Some("zirc-chat"));
        assert_eq!(session_name("  Docs Site ").as_deref(), Some("docs-site"));
        assert_eq!(session_name("cc-598aa1/x!").as_deref(), Some("cc-598aa1x"));
        assert_eq!(
            session_name("a-very-long-name-for-a-session-indeed").map(|n| n.len()),
            Some(24)
        );
        assert_eq!(session_name(" !? "), None);
        assert_eq!(session_tool("Claude Code").as_deref(), Some(CLAUDE_CODE));
        assert_eq!(session_tool(" ").as_deref(), None);
        assert_eq!(agent_avatar(" Rocket"), Some("rocket"));
        assert_eq!(agent_avatar("Reviewer"), Some("reviewer"));
        assert_eq!(agent_avatar("unicorn"), None);
        assert_eq!(AGENT_AVATARS[0].0, DEFAULT_ROLE);
        assert_eq!(AGENT_AVATARS[16].0, "performance");
        let named = Message {
            name: Some("zirc-chat".into()),
            tool: Some(CODEX.into()),
            avatar: Some("rocket".into()),
            ..Message::new("cc-598aa1", Event::Text { body: "hi".into() })
        };
        let back = decode(&encode(&named).unwrap()).unwrap().unwrap();
        assert_eq!(back.name.as_deref(), Some("zirc-chat"));
        assert_eq!(back.tool.as_deref(), Some(CODEX));
        assert_eq!(back.avatar.as_deref(), Some("rocket"));
        assert_eq!(
            decode(&encode(&Message::new("cc-1", Event::Text { body: "hi".into() })).unwrap())
                .unwrap()
                .unwrap()
                .name,
            None
        );
    }

    #[test]
    fn keeps_line_breaks_for_a_multiline_message() {
        let message = Message::new(
            "cc-1",
            Event::Text {
                body: "first\r\nsecond\x01".into(),
            },
        );
        let body = encode_lines_up_to(&message, 4096).unwrap();
        assert!(body.ends_with(":first\nsecond\x01"), "{body:?}");
        assert_eq!(decode(&body).unwrap().unwrap(), message);
        assert!(!encode(&message).unwrap().contains('\n'));
    }

    #[test]
    fn makes_short_app_labels() {
        assert_eq!(app_label("CI").as_deref(), Some("ci"));
        assert_eq!(app_label(" Deploy Bot! ").as_deref(), Some("deploybot"));
        assert_eq!(
            app_label("a-very-long-integration").as_deref(),
            Some("a-very-long")
        );
        assert_eq!(app_label("!!"), None);
        // Old messages, without it, still read.
        let old =
            serde_json::to_string(&Message::new("ci", Event::Text { body: "x".into() })).unwrap();
        assert!(!old.contains("app"));
        let mut with = Message::new("ci", Event::Text { body: "x".into() });
        with.app = Some("deploy".into());
        let back: Message = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back.app.as_deref(), Some("deploy"));
    }

    fn commit() -> Message {
        Message::new(
            "cc-3f2a",
            Event::Commit {
                paths: Vec::new(),
                branch: "fix-login".into(),
                sha: "a1b2c3d4".into(),
                subject: "Refresh tokens before they expire".into(),
                repo: None,
            },
        )
    }

    #[test]
    fn round_trips() {
        let message = commit();
        let body = encode(&message).unwrap();
        assert_eq!(decode(&body).unwrap(), Some(message));
    }

    #[test]
    fn body_is_readable_in_plain_clients() {
        let body = encode(&commit()).unwrap();
        assert!(body.starts_with("\x01ACTION ZIRC "));
        assert!(body.ends_with(" :committed on fix-login: Refresh tokens before they expire\x01"));
    }

    #[test]
    fn human_text_cannot_break_the_frame() {
        let message = Message::new(
            "me",
            Event::Text {
                body: "two\nlines\x01".into(),
            },
        );
        let body = encode(&message).unwrap();
        assert_eq!(body.matches('\x01').count(), 2);
        assert!(!body.contains('\n'));
        assert_eq!(decode(&body).unwrap(), Some(message));
    }

    #[test]
    fn ignores_other_messages() {
        assert_eq!(decode("hello").unwrap(), None);
        assert_eq!(decode("\x01ACTION waves\x01").unwrap(), None);
        assert_eq!(decode("\x01ACTION ZIRCON\x01").unwrap(), None);
    }

    #[test]
    fn rejects_newer_versions() {
        let mut message = commit();
        message.v = VERSION + 1;
        let json = serde_json::to_vec(&message).unwrap();
        let body = format!("\x01ACTION ZIRC {} :x\x01", BASE64.encode(json));
        assert!(matches!(decode(&body), Err(Error::UnknownVersion(_))));
    }

    #[test]
    fn refuses_bodies_too_long_for_a_line() {
        let message = Message::new(
            "me",
            Event::Working {
                paths: vec!["src/a/very/long/path.rs".repeat(20)],
                continued: false,
                worktree: None,
                branch: None,
                part: None,
                base: None,
                committed: Vec::new(),
            },
        );
        assert!(matches!(encode(&message), Err(Error::TooLong(_))));
    }

    #[test]
    fn summarizes_working_sets() {
        let working = |n: usize| Event::Working {
            paths: (1..=n).map(|i| format!("f{i}")).collect(),
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        };
        assert_eq!(working(0).human_text(), "has no uncommitted changes");
        assert_eq!(working(2).human_text(), "working on f1, f2");
        assert_eq!(working(5).human_text(), "working on f1, f2, f3 and 2 more");
    }

    #[test]
    fn leaves_continued_out_of_first_parts() {
        let message = Message::new(
            "me",
            Event::Working {
                paths: vec!["a".into()],
                continued: false,
                worktree: None,
                branch: None,
                part: None,
                base: None,
                committed: Vec::new(),
            },
        );
        let json = serde_json::to_string(&message).unwrap();
        assert!(!json.contains("continued"), "{json}");
        assert!(!json.contains("worktree"), "{json}");
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), message);
    }

    #[test]
    fn reads_working_sets_from_before_worktrees() {
        let json = r#"{"v":1,"session":"watcher","event":{"type":"working","paths":["a"]}}"#;
        let message: Message = serde_json::from_str(json).unwrap();
        assert_eq!(
            message.event,
            Event::Working {
                paths: vec!["a".into()],
                continued: false,
                worktree: None,
                branch: None,
                part: None,
                base: None,
                committed: Vec::new(),
            }
        );
    }

    #[test]
    fn custom_events_carry_their_data_and_show_their_text() {
        let message = Message::new(
            "deploy-bot",
            Event::Custom {
                kind: "x-acme/deploy".into(),
                data: serde_json::json!({"env": "prod", "version": "1.4.2"}),
                text: "deployed 1.4.2 to prod".into(),
            },
        );
        let body = encode(&message).unwrap();
        assert!(body.ends_with(" :deployed 1.4.2 to prod\x01"), "{body}");
        assert_eq!(decode(&body).unwrap(), Some(message));
        let json = r#"{"type":"custom","kind":"x-acme/ping","text":"ping"}"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&event).unwrap(), json);
    }

    #[test]
    fn files_show_their_link_and_leave_out_what_they_lack() {
        let message = Message::new(
            "mattias",
            Event::File(File {
                url: "https://zirc.dev/m/acme/7Hq2xV9kLmN3pR8sT4wZbc".into(),
                name: "IMG_2041.jpeg".into(),
                mime: "image/jpeg".into(),
                size: 2_345_678,
                width: Some(4032),
                height: Some(3024),
                duration_ms: None,
            }),
        );
        let body = encode(&message).unwrap();
        assert!(
            body.ends_with(
                " :shared IMG_2041.jpeg: https://zirc.dev/m/acme/7Hq2xV9kLmN3pR8sT4wZbc\x01"
            ),
            "{body}"
        );
        assert_eq!(decode(&body).unwrap(), Some(message));
        let json = r#"{"type":"file","url":"u","name":"n.txt","mime":"text/plain","size":3}"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&event).unwrap(), json);
    }

    #[test]
    fn custom_kinds_are_namespaced() {
        assert_eq!(check_kind("x-acme/deploy"), None);
        assert_eq!(check_kind("x-acme-2/on-call"), None);
        for kind in [
            "deploy",
            "x-acme",
            "x-/deploy",
            "x-acme/",
            "x-Acme/deploy",
            "x-acme/a/b",
        ] {
            assert!(check_kind(kind).is_some(), "{kind}");
        }
    }
}
