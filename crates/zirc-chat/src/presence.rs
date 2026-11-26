//! Who is working on what, per channel, from the `working` events machines send, and where
//! that overlaps with one's own uncommitted changes: another machine's, or another worktree
//! on this machine, where each agent session can have its own. Pure bookkeeping: the daemon
//! feeds it what arrives and asks it what to say.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use zirc_line::Line;
use zirc_proto::{Event, Message};

/// Paths work elsewhere changes that a worktree on this machine changes too: another
/// machine's working set, or a push, another worktree's here included, whose lines meet the
/// worktree's ([`Overlap::pushed`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Overlap {
    /// The channel both work in.
    pub channel: String,
    /// The other machine's nick.
    pub nick: String,
    /// The paths both have changed, sorted.
    pub paths: Vec<String>,
    /// Which worktree here changes them, when zircd says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<Worktree>,
    /// That machine's sessions whose lines in those paths were compared with the
    /// worktree's, sorted: none when it said no lines, or what they're counted from isn't
    /// here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
    /// Of the paths, the ones both change on the same or neighbouring lines, with those
    /// lines: likely to conflict.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub same_lines: BTreeMap<String, Vec<[u32; 2]>>,
    /// Of the paths, the ones both change in different parts: likely to merge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apart: Vec<String>,
    /// Whether their lines were compared as each side counts them, from commits the two
    /// don't share: what landed between the two isn't allowed for, so they may be off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_common_base: bool,
    /// The branch whose push changes them, when it's a push that was compared with the
    /// worktree: whichever of the two merges second has a conflict to resolve. Until 25 Nov
    /// 2026 that was an alert of its own, "conflict ahead".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed: Option<String>,
    /// Why it concerns the watching session, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
}

/// A working tree on this machine, and the branch checked out in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Worktree {
    /// Its root.
    pub path: PathBuf,
    /// The branch checked out, unless HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// Paths two worktrees on this machine both have uncommitted changes in: two sessions
/// stepping on each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocalOverlap {
    /// The channel both worktrees' repository works in.
    pub channel: String,
    /// The two worktrees, sorted by path, then branch.
    pub worktrees: [Worktree; 2],
    /// The paths both have changed, sorted.
    pub paths: Vec<String>,
    /// Of those, the ones both change on the same or neighbouring lines, with those lines
    /// of their merge base: likely to conflict.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub same_lines: BTreeMap<String, Vec<[u32; 2]>>,
    /// Of those, the ones both change in different parts: likely to merge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apart: Vec<String>,
    /// Whether their lines were compared as each side counts them, from commits the two
    /// don't share: what landed between the two isn't allowed for, so they may be off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_common_base: bool,
    /// Why it concerns the watching session, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
}

/// One worktree's working set in a channel, or a whole machine's when it doesn't say which
/// worktree its paths are in, as zircds before 0.2 don't.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Member {
    /// Its nick.
    pub nick: String,
    /// For this machine, which of its worktrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<Worktree>,
    /// The name of the worktree's directory, when known: a better label than a branch
    /// named by a tool, such as `mg-branch-39`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The branch the paths are changed on, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The session working there, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// What that session's called, when it says ([`Message::name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// The agent tool that session runs in, when it says ([`Message::tool`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tool: Option<String>,
    /// The picture that session goes by, when it says ([`Message::avatar`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_avatar: Option<String>,
    /// The paths it has uncommitted changes in, sorted.
    pub paths: Vec<String>,
}

/// Where on a machine a working set is, as far as its `working` events say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Place {
    /// The name of the worktree's directory, or `None` for the whole machine.
    pub worktree: Option<String>,
    /// The branch checked out there.
    pub branch: Option<String>,
    /// The session working there.
    pub session: Option<String>,
    /// The commit its work is built on, when its machine says
    /// ([`Event::Working`]'s `base`).
    pub base: Option<String>,
    /// The paths its own commits change beyond the uncommitted ones, when its machine says.
    pub committed: BTreeSet<String>,
}

/// What a worktree's work is built on and all it changes, as its machine says: enough to
/// tell, of something that landed, whether the work is behind it and depends on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Built {
    /// The machine's nick.
    pub nick: String,
    /// The session working there, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The branch checked out there, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The commit of the default branch its work is built on, by the start of its id
    /// ([`zirc_proto::BASE_DIGITS`] digits).
    pub base: String,
    /// The paths its work changes since `base`, committed or not, sorted.
    pub changed: Vec<String>,
}

/// A machine's working sets, by worktree name and branch. A `None` worktree is the machine as a whole.
type Sets = BTreeMap<(Option<String>, Option<String>), (Place, BTreeSet<String>)>;

/// One part of a working set: where its paths are, and the paths.
type Part = (Place, Vec<String>);

/// Every path in a machine's working sets: what's uncommitted, and what its own commits
/// change that hasn't landed. Work committed straight away, as agents do every turn, was
/// never compared, and conflicted on landing with nobody told (15 Nov 2026).
fn all_paths(sets: &Sets) -> BTreeSet<String> {
    sets.values()
        .flat_map(|(place, paths)| paths.iter().chain(&place.committed).cloned())
        .collect()
}

/// What a session said it's taking on, with `zirc post "taking: …"`, until it posts
/// `done: …` or `dropped: …` or leaves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Claim {
    /// The machine's nick.
    pub nick: String,
    /// The session that posted it.
    pub session: String,
    /// What that session's called, when it says ([`Message::name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// The agent tool that session runs in, when it says ([`Message::tool`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tool: Option<String>,
    /// The picture that session goes by, when it says ([`Message::avatar`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_avatar: Option<String>,
    /// What it's taking on, as it said it.
    pub task: String,
    /// When it said so, in seconds since the Unix epoch.
    pub since: u64,
}

/// What a session said it's about to touch, with `zirc intend`, until it expires, the
/// session posts `done: …` or `dropped: …`, or its machine leaves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Intent {
    /// The machine's nick.
    pub nick: String,
    /// The session that sent it.
    pub session: String,
    /// What that session's called, when it says ([`Message::name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// The agent tool that session runs in, when it says ([`Message::tool`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tool: Option<String>,
    /// The picture that session goes by, when it says ([`Message::avatar`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_avatar: Option<String>,
    /// What it's about to do, as it said it.
    pub summary: String,
    /// The paths or directories it expects to touch.
    pub paths: Vec<String>,
    /// When it ends, in seconds since the Unix epoch, if it does on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
}

/// A push to a repository's default branch: work that's landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Landing {
    /// The nick of the machine that pushed.
    pub nick: String,
    /// The session that pushed it.
    pub session: String,
    /// The branch pushed, the default one.
    pub branch: String,
    /// The subjects of the commits it pushed, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<String>,
    /// The paths they changed, when the push said.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// When it was heard, in seconds since the Unix epoch.
    pub at: u64,
}

/// How many landings each channel remembers.
pub const LANDINGS: usize = 5;

/// Whether `intent` hasn't expired at `now`, nor said its work is finished.
fn live(intent: &Intent, now: u64) -> bool {
    intent.expires.is_none_or(|expires| expires > now)
        && !zirc_proto::says_finished(&intent.summary)
}

/// Whether two paths relative to a repository's root are, or contain, the same file: equal,
/// or one a directory the other is in.
#[must_use]
pub fn touches(a: &str, b: &str) -> bool {
    let plain = |path: &'_ str| -> String {
        match path.trim_start_matches("./").trim_end_matches('/') {
            "." => String::new(),
            path => path.to_owned(),
        }
    };
    let (a, b) = (plain(a), plain(b));
    let (a, b) = (a.as_str(), b.as_str());
    let within = |inner: &str, outer: &str| {
        inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('/'))
    };
    a == b || within(a, b) || within(b, a) || a.is_empty() || b.is_empty()
}

/// Which of `theirs` touch any of `mine`, as [`touches`] says.
#[must_use]
pub fn touching<'a>(theirs: impl IntoIterator<Item = &'a String>, mine: &[&str]) -> Vec<String> {
    theirs
        .into_iter()
        .filter(|path| mine.iter().any(|other| touches(path, other)))
        .cloned()
        .collect()
}

/// How long a path that's stopped overlapping stays alerted about: one that overlaps again
/// within this, as when a working set arrives in parts, isn't news.
pub const ALERT_GRACE_SECS: u64 = 5 * 60;

/// Overlapping paths already alerted about, and since when each has stopped overlapping, if
/// it has.
type Alerted = BTreeMap<String, Option<u64>>;

/// Which of `current` paths are new to `alerted` at `now`: never alerted about, or back after
/// being gone longer than [`ALERT_GRACE_SECS`]. Notes which have gone, and forgets those gone
/// too long.
fn fresh(alerted: &mut Alerted, current: &BTreeSet<String>, now: u64) -> Vec<String> {
    let gone_too_long = |since: Option<u64>| {
        since.is_some_and(|since| now.saturating_sub(since) >= ALERT_GRACE_SECS)
    };
    let new = current
        .iter()
        .filter(|path| alerted.get(*path).is_none_or(|since| gone_too_long(*since)))
        .cloned()
        .collect();
    for (path, since) in alerted.iter_mut() {
        if !current.contains(path) && since.is_none() {
            *since = Some(now);
        }
    }
    alerted.retain(|path, since| current.contains(path) || !gone_too_long(*since));
    for path in current {
        alerted.insert(path.clone(), None);
    }
    new
}

/// Everyone's working sets, and which overlaps have been pointed out already.
#[derive(Debug, Default)]
pub struct Presence {
    /// channel → nick → worktree → paths
    working: BTreeMap<String, BTreeMap<String, Sets>>,
    /// channel → (nick, session) → what it's taking on
    claims: BTreeMap<String, BTreeMap<(String, String), Claim>>,
    /// channel → (nick, session) → what it's about to touch
    intents: BTreeMap<String, BTreeMap<(String, String), Intent>>,
    /// (nick in ASCII lower case, session) → when it last said its task is done, as zircd
    /// says for it once its landing lands, until it takes something on again. Kept across
    /// reconnects: it's what happened.
    done: BTreeMap<(String, String), u64>,
    /// what a conflict is between → clashing paths already alerted about → since when gone,
    /// if gone. This machine's own, kept across reconnects.
    alerted_conflicts: BTreeMap<String, Alerted>,
    /// (channel, nick) → the numbered parts of a working set heard so far: the last part's
    /// number and the parts.
    partial: BTreeMap<(String, String), (u32, Vec<Part>)>,
    /// channel → its latest landings, oldest first. Kept across reconnects: they're what
    /// happened, not who's here.
    landings: BTreeMap<String, Vec<Landing>>,
    /// (nick, session) → what the session's called, as its latest message said.
    names: BTreeMap<(String, String), String>,
    /// (nick, session) → the agent tool the session runs in, as its latest message said.
    tools: BTreeMap<(String, String), String>,
    /// (nick, session) → the picture the session goes by, as its latest message said.
    avatars: BTreeMap<(String, String), String>,
    /// channel → nick → session → the lines its work changes ([`zirc_proto::lines`]).
    lines: BTreeMap<String, BTreeMap<String, BTreeMap<String, zirc_proto::lines::Lines>>>,
    /// A room every repository shares (`#agents`), lowercase → the scopes of the repositories
    /// heard in it ([`crate::names::scope_of`]): what's said there is kept by its
    /// repository's scope, as zircd keeps it, and read back as the room's.
    scopes: BTreeMap<String, BTreeSet<String>>,
}

impl Presence {
    /// Takes in `message`, which `nick` sent to `channel` at `at`, in seconds since the Unix
    /// epoch: the line's server time when it has one. Working sets, `taking:`, `done:` and
    /// `dropped:` posts and intents change what's known; so does a push to `default_branch`,
    /// the channel's repository's, which is a landing. Every client keeps its state this
    /// way: zircd, and the apps from the lines they're sent.
    pub fn apply(
        &mut self,
        channel: &str,
        nick: &str,
        message: &Message,
        at: u64,
        default_branch: Option<&str>,
    ) {
        // A renewal keeps its intent going as the intent said again would.
        if let Some(intent) = zirc_proto::renewed(&message.event) {
            let said = Message {
                event: intent,
                ..message.clone()
            };
            return self.apply(channel, nick, &said, at, default_branch);
        }
        // In a room every repository shares, each repository's work apart: one machine says
        // each repository's working sets in turn, each replacing what it said before.
        let scope = crate::names::scope_of(channel, message.repo.as_deref());
        if scope != channel {
            self.scopes
                .entry(channel.to_lowercase())
                .or_default()
                .insert(scope.clone());
        }
        let channel = scope.as_str();
        let session = &message.session;
        if let Some(name) = &message.name {
            self.names
                .insert((nick.to_owned(), session.clone()), name.clone());
        }
        if let Some(tool) = &message.tool {
            self.tools
                .insert((nick.to_owned(), session.clone()), tool.clone());
        }
        if let Some(avatar) = &message.avatar {
            self.avatars
                .insert((nick.to_owned(), session.clone()), avatar.clone());
        }
        match &message.event {
            Event::Working {
                paths,
                worktree,
                branch,
                part,
                base,
                committed,
                ..
            } => {
                let place = Place {
                    worktree: worktree.clone(),
                    branch: branch.clone(),
                    session: (session != zirc_proto::NO_SESSION).then(|| session.clone()),
                    base: base.clone(),
                    committed: committed.iter().cloned().collect(),
                };
                self.working_part(channel, nick, place, paths.clone(), *part);
            }
            Event::Text { body } => self.posted(channel, nick, session, body, at),
            Event::Intent {
                summary,
                paths,
                expires,
                lasts,
            } => {
                let intent = Intent {
                    nick: nick.to_owned(),
                    session: session.clone(),
                    session_name: None,
                    session_tool: None,
                    session_avatar: None,
                    summary: summary.clone(),
                    paths: paths.clone(),
                    expires: lasts.map(|lasts| at + lasts).or(*expires),
                };
                self.intended(channel, intent, at);
            }
            Event::Push {
                branch,
                subjects,
                paths,
                ..
            } if default_branch == Some(branch.as_str()) => {
                let landing = Landing {
                    nick: nick.to_owned(),
                    session: session.clone(),
                    branch: branch.clone(),
                    subjects: subjects.clone(),
                    paths: paths.clone(),
                    at,
                };
                self.landed(channel, landing);
            }
            Event::Custom { .. }
                if let Some(lines) = zirc_proto::lines::Lines::of(&message.event) =>
            {
                self.lines_said(channel, nick, session, lines);
            }
            // Nothing more to keep: a team's own event, for one, says nothing about who's
            // working on what.
            Event::Push { .. }
            | Event::Commit { .. }
            | Event::Custom { .. }
            | Event::File(_)
            | Event::Query { .. }
            | Event::Answer { .. }
            | Event::Note { .. } => {}
        }
    }

    /// One part of a working set. Numbered parts are held until the last arrives, then
    /// replace what was known; a set that's missing a part is dropped and what was known
    /// stays. One worktree's set said alone ([`zirc_proto::ALONE`]) replaces its own.
    fn working_part(
        &mut self,
        channel: &str,
        nick: &str,
        place: Place,
        paths: Vec<String>,
        part: Option<[u32; 2]>,
    ) {
        let Some([number, count]) = part else {
            return;
        };
        if [number, count] == zirc_proto::ALONE {
            self.working_alone(channel, nick, place, paths);
            return;
        }
        let key = (channel.to_owned(), nick.to_owned());
        if number == 1 {
            self.partial.insert(key.clone(), (0, Vec::new()));
        }
        let Some((last, parts)) = self.partial.get_mut(&key) else {
            return;
        };
        if number != *last + 1 {
            self.partial.remove(&key);
            return;
        }
        *last = number;
        parts.push((place, paths));
        if number >= count {
            let (_, parts) = self.partial.remove(&key).unwrap_or_default();
            for (i, (place, paths)) in parts.into_iter().enumerate() {
                self.working_at(channel, nick, place, paths, i > 0);
            }
        }
    }

    /// Records a `working` event from `nick` in `channel` with paths at `place`. One that
    /// isn't `continued` replaces all of the machine's sets, whichever worktree it names:
    /// a machine sends its worktrees' sets one after another, the first replacing and the
    /// rest continuing.
    fn working_at(
        &mut self,
        channel: &str,
        nick: &str,
        place: Place,
        paths: Vec<String>,
        continued: bool,
    ) {
        let members = self.working.entry(channel.to_owned()).or_default();
        let sets = members.entry(nick.to_owned()).or_default();
        if !continued {
            sets.clear();
        }
        let key = (place.worktree.clone(), place.branch.clone());
        let (known, set) = sets.entry(key).or_default();
        // A set split over several parts has its committed paths split over them too.
        let mut place = place;
        place.committed.append(&mut known.committed);
        // And says what it's built on in one of them.
        if place.base.is_none() {
            place.base = known.base.take();
        }
        *known = place;
        set.extend(paths);
        sets.retain(|_, (place, paths)| !paths.is_empty() || !place.committed.is_empty());
        if sets.is_empty() {
            members.remove(nick);
        }
    }

    /// Records one worktree's set from `nick` in `channel`, said alone between the machine's
    /// whole sets: it replaces the set at `place` and leaves the machine's others as they
    /// are.
    pub fn working_alone(&mut self, channel: &str, nick: &str, place: Place, paths: Vec<String>) {
        let members = self.working.entry(channel.to_owned()).or_default();
        let sets = members.entry(nick.to_owned()).or_default();
        let key = (place.worktree.clone(), place.branch.clone());
        // Said without what it's built on, which didn't fit the line beside its paths: that
        // stays as the machine last said it.
        let mut place = place;
        if !paths.is_empty()
            && place.base.is_none()
            && place.committed.is_empty()
            && let Some((known, _)) = sets.get(&key)
        {
            place.base.clone_from(&known.base);
            place.committed.clone_from(&known.committed);
        }
        if paths.is_empty() && place.committed.is_empty() {
            sets.remove(&key);
        } else {
            sets.insert(key, (place, paths.into_iter().collect()));
        }
        if sets.is_empty() {
            members.remove(nick);
        }
    }

    /// Records a post by `session` on `nick` in `channel`: `taking: <task>` claims a task,
    /// and `done: …` or `dropped: …` gives it up. Anything else isn't about claims.
    pub fn posted(&mut self, channel: &str, nick: &str, session: &str, text: &str, now: u64) {
        let claims = self.claims.entry(channel.to_owned()).or_default();
        let key = (nick.to_owned(), session.to_owned());
        let text = text.trim();
        let done = (nick.to_ascii_lowercase(), session.to_owned());
        if let Some(task) = zirc_proto::taken(text) {
            if self.done.get(&done).is_some_and(|&at| at <= now) {
                self.done.remove(&done);
            }
            claims.insert(
                key,
                Claim {
                    nick: nick.to_owned(),
                    session: session.to_owned(),
                    task: task.to_owned(),
                    session_name: None,
                    session_tool: None,
                    session_avatar: None,
                    since: now,
                },
            );
        } else if zirc_proto::says_finished(text) {
            if zirc_proto::says_done(text) {
                let at = self.done.entry(done).or_insert(now);
                *at = (*at).max(now);
            }
            claims.remove(&key);
            if let Some(intents) = self.intents.get_mut(channel) {
                intents.remove(&key);
            }
        }
    }

    /// When the session `session` on the machine `nick` last said its task is done (`done:
    /// …`, as zircd says once its landing lands), in seconds since the Unix epoch, unless
    /// it has taken something on since.
    #[must_use]
    pub fn done_at(&self, nick: &str, session: &str) -> Option<u64> {
        self.done
            .get(&(nick.to_ascii_lowercase(), session.to_owned()))
            .copied()
    }

    /// Records a push to `channel`'s default branch.
    pub fn landed(&mut self, channel: &str, landing: Landing) {
        let landings = self.landings.entry(channel.to_owned()).or_default();
        landings.push(landing);
        if landings.len() > LANDINGS {
            landings.remove(0);
        }
    }

    /// The latest landings in `channel`, newest first.
    #[must_use]
    pub fn landings(&self, channel: &str) -> Vec<Landing> {
        let mut landings: Vec<Landing> = self
            .keys(channel)
            .filter_map(|key| self.landings.get(key))
            .flatten()
            .cloned()
            .collect();
        landings.reverse();
        landings
    }

    /// How many repositories have been heard in `channel`, a room every repository shares
    /// (`#agents`): none for any other.
    #[must_use]
    pub fn repositories(&self, channel: &str) -> usize {
        self.scopes_in(channel).map_or(0, BTreeSet::len)
    }

    /// The scopes heard in `channel`, a room every repository shares, found without making a
    /// string: it's asked for every line drawn.
    fn scopes_in(&self, channel: &str) -> Option<&BTreeSet<String>> {
        self.scopes
            .iter()
            .find(|(room, _)| room.eq_ignore_ascii_case(channel))
            .map(|(_, scopes)| scopes)
    }

    /// What `channel`'s presence is kept under: itself, and, for a room every repository
    /// shares, each repository's scope heard there.
    pub fn keys<'a>(&'a self, channel: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        std::iter::once(channel).chain(
            self.scopes_in(channel)
                .into_iter()
                .flatten()
                .map(String::as_str),
        )
    }

    /// Records `intent` in `channel` at `now`, replacing what its session intended before,
    /// and forgets the channel's intents that have expired.
    pub fn intended(&mut self, channel: &str, intent: Intent, now: u64) {
        let key = (intent.nick.clone(), intent.session.clone());
        let intents = self.intents.entry(channel.to_owned()).or_default();
        intents.retain(|_, intent| live(intent, now));
        intents.insert(key, intent);
    }

    /// The intents in `channel` that haven't expired at `now`, by nick and session.
    #[must_use]
    pub fn intents(&self, channel: &str, now: u64) -> Vec<Intent> {
        self.keys(channel)
            .filter_map(|key| self.intents.get(key))
            .flat_map(BTreeMap::values)
            .filter(|intent| live(intent, now))
            .map(|intent| Intent {
                session_name: self.name_of(&intent.nick, &intent.session),
                session_tool: self.tool_of(&intent.nick, &intent.session),
                session_avatar: self.avatar_of(&intent.nick, &intent.session),
                ..intent.clone()
            })
            .collect()
    }

    /// The agent tool the session `session` on `nick` runs in, as its latest message said.
    #[must_use]
    pub fn tool_of(&self, nick: &str, session: &str) -> Option<String> {
        self.tools
            .get(&(nick.to_owned(), session.to_owned()))
            .cloned()
    }

    /// The picture the session `session` on `nick` goes by, as its latest message said.
    #[must_use]
    pub fn avatar_of(&self, nick: &str, session: &str) -> Option<String> {
        self.avatars
            .get(&(nick.to_owned(), session.to_owned()))
            .cloned()
    }

    /// What the session `session` on `nick` is called, as its latest message said.
    #[must_use]
    pub fn name_of(&self, nick: &str, session: &str) -> Option<String> {
        self.names
            .get(&(nick.to_owned(), session.to_owned()))
            .cloned()
    }

    /// What the session `session` on `nick` has said it's taking on, in each channel it
    /// said so in, and when, as [`Presence::claims`] has it without the session's name,
    /// tool or picture: asked for every agent the lists show, each refresh, so one pass
    /// over every claim rather than a look-up of each channel.
    pub fn claims_of(&self, nick: &str, session: &str) -> impl Iterator<Item = (&str, &str, u64)> {
        self.every_claim()
            .filter(move |(_, claim)| {
                claim.session == session && claim.nick.eq_ignore_ascii_case(nick)
            })
            .map(|(channel, claim)| (channel, claim.task.as_str(), claim.since))
    }

    /// Every session's claims, each with the channel it was said in: for a list that wants
    /// many sessions' at once, one pass where [`Presence::claims_of`] for each would be one
    /// each.
    pub fn every_claim(&self) -> impl Iterator<Item = (&str, &Claim)> {
        self.claims.iter().flat_map(move |(key, claims)| {
            let channel = self.room_of(key);
            claims.values().map(move |claim| (channel, claim))
        })
    }

    /// The channel what's kept under `key` was said in: the room every repository shares
    /// for one of its repositories' scopes ([`Presence::keys`]), else `key` itself.
    #[must_use]
    pub fn room_of<'a>(&'a self, key: &'a str) -> &'a str {
        self.scopes
            .iter()
            .find(|(_, scopes)| scopes.contains(key))
            .map_or(key, |(room, _)| room.as_str())
    }

    /// What sessions in `channel` have said they're taking on, oldest first.
    #[must_use]
    pub fn claims(&self, channel: &str) -> Vec<Claim> {
        let mut claims: Vec<Claim> = self
            .keys(channel)
            .filter_map(|key| self.claims.get(key))
            .flat_map(BTreeMap::values)
            .map(|claim| Claim {
                session_name: self.name_of(&claim.nick, &claim.session),
                session_tool: self.tool_of(&claim.nick, &claim.session),
                session_avatar: self.avatar_of(&claim.nick, &claim.session),
                ..claim.clone()
            })
            .collect();
        claims.sort_by_key(|claim| claim.since);
        claims
    }

    /// What `session` on `nick` says it changes: one that's not `continued` replaces what
    /// was said before.
    fn lines_said(
        &mut self,
        channel: &str,
        nick: &str,
        session: &str,
        lines: zirc_proto::lines::Lines,
    ) {
        let sessions = self
            .lines
            .entry(channel.to_owned())
            .or_default()
            .entry(nick.to_owned())
            .or_default();
        match sessions.get_mut(session) {
            Some(known) if lines.continued && known.base == lines.base => {
                known.lines.extend(lines.lines);
            }
            _ if lines.lines.is_empty() => {
                sessions.remove(session);
            }
            _ => {
                sessions.insert(session.to_owned(), lines);
            }
        }
    }

    /// The lines `session` on `nick` last said its work in `channel` changes, and the
    /// commit they're of, if it said.
    #[must_use]
    pub fn lines_of(
        &self,
        channel: &str,
        nick: &str,
        session: &str,
    ) -> Option<&zirc_proto::lines::Lines> {
        self.lines.get(channel)?.get(nick)?.get(session)
    }

    /// Every session's lines said in `channel`: its nick, the session, and the lines.
    #[must_use]
    pub fn all_lines(&self, channel: &str) -> Vec<(String, String, zirc_proto::lines::Lines)> {
        self.keys(channel)
            .filter_map(|key| self.lines.get(key))
            .flatten()
            .flat_map(|(nick, sessions)| {
                sessions
                    .iter()
                    .map(|(session, lines)| (nick.clone(), session.clone(), lines.clone()))
            })
            .collect()
    }

    /// `nick` left `channel`: whatever it was working on or taking on there no longer
    /// counts.
    pub fn left(&mut self, channel: &str, nick: &str) {
        let keys: Vec<String> = self.keys(channel).map(str::to_owned).collect();
        for key in keys {
            self.left_key(&key, nick);
        }
    }

    /// [`Presence::left`] for one of a channel's keys ([`Presence::keys`]).
    fn left_key(&mut self, channel: &str, nick: &str) {
        if let Some(members) = self.working.get_mut(channel) {
            members.remove(nick);
        }
        if let Some(members) = self.lines.get_mut(channel) {
            members.remove(nick);
        }
        if let Some(claims) = self.claims.get_mut(channel) {
            claims.retain(|(claimer, _), _| claimer != nick);
        }
        if let Some(intents) = self.intents.get_mut(channel) {
            intents.retain(|(intender, _), _| intender != nick);
        }
    }

    /// `nick` left the server.
    pub fn quit(&mut self, nick: &str) {
        let channels: BTreeSet<String> = self
            .working
            .keys()
            .chain(self.lines.keys())
            .chain(self.claims.keys())
            .chain(self.intents.keys())
            .cloned()
            .collect();
        for channel in channels {
            self.left(&channel, nick);
        }
    }

    /// `nick` is now called `new`.
    pub fn renamed(&mut self, nick: &str, new: &str) {
        for members in self.working.values_mut() {
            if let Some(paths) = members.remove(nick) {
                members.insert(new.to_owned(), paths);
            }
        }
        for members in self.lines.values_mut() {
            if let Some(lines) = members.remove(nick) {
                members.insert(new.to_owned(), lines);
            }
        }
        for claims in self.claims.values_mut() {
            let moved: Vec<(String, String)> = claims
                .keys()
                .filter(|(claimer, _)| claimer == nick)
                .cloned()
                .collect();
            for key in moved {
                if let Some(mut claim) = claims.remove(&key) {
                    new.clone_into(&mut claim.nick);
                    claims.insert((new.to_owned(), key.1), claim);
                }
            }
        }
        for intents in self.intents.values_mut() {
            let moved: Vec<(String, String)> = intents
                .keys()
                .filter(|(intender, _)| intender == nick)
                .cloned()
                .collect();
            for key in moved {
                if let Some(mut intent) = intents.remove(&key) {
                    new.clone_into(&mut intent.nick);
                    intents.insert((new.to_owned(), key.1), intent);
                }
            }
        }
    }

    /// Forgets everything heard from the server, as when the connection drops.
    pub fn clear(&mut self) {
        self.working.clear();
        self.lines.clear();
        self.claims.clear();
        self.intents.clear();
        self.scopes.clear();
    }

    /// The working sets in `channel`, by nick, then by worktree: in a room every repository
    /// shares, every repository's ([`Presence::keys`]).
    #[must_use]
    pub fn members(&self, channel: &str) -> Vec<Member> {
        self.keys(channel)
            .flat_map(|key| self.members_in(key))
            .collect()
    }

    /// The working sets kept under `key`, one of a channel's [`Presence::keys`].
    #[must_use]
    pub fn members_in(&self, key: &str) -> Vec<Member> {
        self.working
            .get(key)
            .into_iter()
            .flatten()
            .flat_map(|(nick, sets)| {
                // A worktree with commits of its own and nothing uncommitted is known for
                // what it's built on ([`Presence::built`]), and isn't a member.
                let changing = sets.values().filter(|(_, paths)| !paths.is_empty());
                changing.map(|(place, paths)| Member {
                    nick: nick.clone(),
                    worktree: None,
                    name: place.worktree.clone(),
                    branch: place.branch.clone(),
                    session: place.session.clone(),
                    session_name: place
                        .session
                        .as_deref()
                        .and_then(|session| self.name_of(nick, session)),
                    session_tool: place
                        .session
                        .as_deref()
                        .and_then(|session| self.tool_of(nick, session)),
                    session_avatar: place
                        .session
                        .as_deref()
                        .and_then(|session| self.avatar_of(nick, session)),
                    paths: paths.iter().cloned().collect(),
                })
            })
            .collect()
    }

    /// What each worktree's work in `channel` is built on and changes, for those whose
    /// machines say.
    #[must_use]
    pub fn built(&self, channel: &str) -> Vec<Built> {
        self.keys(channel)
            .filter_map(|key| self.working.get(key))
            .flatten()
            .flat_map(|(nick, sets)| {
                sets.values().filter_map(|(place, paths)| {
                    Some(Built {
                        nick: nick.clone(),
                        session: place.session.clone(),
                        branch: place.branch.clone(),
                        base: place.base.clone()?,
                        changed: paths.union(&place.committed).cloned().collect(),
                    })
                })
            })
            .collect()
    }

    /// Where others in `channel` work on what `mine` has changed.
    #[must_use]
    pub fn overlaps(&self, channel: &str, mine: &BTreeSet<String>) -> Vec<Overlap> {
        self.working
            .get(channel)
            .into_iter()
            .flatten()
            .filter_map(|(nick, sets)| {
                let paths: Vec<String> = all_paths(sets).intersection(mine).cloned().collect();
                (!paths.is_empty()).then(|| Overlap {
                    channel: channel.to_owned(),
                    nick: nick.clone(),
                    paths,
                    ..Overlap::default()
                })
            })
            .collect()
    }

    /// Which of `clashing`, the paths a conflict between two sides named by `key` is in,
    /// are news at `now`: once per path, until it stops clashing for longer than
    /// [`ALERT_GRACE_SECS`].
    pub fn new_conflicts(
        &mut self,
        key: &str,
        clashing: &BTreeSet<String>,
        now: u64,
    ) -> Vec<String> {
        let alerted = self.alerted_conflicts.entry(key.to_owned()).or_default();
        let new = fresh(alerted, clashing, now);
        if alerted.is_empty() {
            self.alerted_conflicts.remove(key);
        }
        new
    }
}

/// Why something heard concerns the session watching. Whatever doesn't is still sent, without
/// a reason, for whoever wants everything (`zirc watch --all`, `--json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Why {
    /// Someone else is changing paths the session has changed too.
    Overlap,
    /// A message to this machine, or to the session.
    Direct,
    /// A post that names this machine or the session.
    Mention,
    /// A post or claim that names paths the session has changed.
    Paths,
    /// Someone is about to touch paths the session has changed or is about to touch.
    Intent,
    /// A post in a topic channel the session joined.
    Topic,
    /// A push to the default branch, or to the session's branch, that changed paths the
    /// session has changed: it may need to rebase.
    Landing,
    /// A push of a branch built on the session's own, which hasn't landed: rewriting the
    /// session's from under it breaks theirs.
    StackedOn,
    /// Said in a channel the watch was asked to print all of (`zirc watch --channel`).
    Channel,
    /// A decision a person made (`x-zirc/decision`), which every session hears.
    Decision,
    /// A reaction to one of the session's own messages.
    Reacted,
    /// CI saying how a landing went that the session said it waits on: another session's,
    /// a build or a branch (`zirc wait-for`).
    Awaited,
    /// CI saying a branch landed that zircd had told the session was changing its files, by
    /// an overlap, a push or a commit: the session should update onto it.
    BranchLanded,
    /// A state the session waits on (`zirc wait-for --state`), such as master, said at a
    /// level other than the one it stood at: broken, or fine again.
    StateChanged,
    /// The machine's person gave a reaction the session waits for (`zirc wait-for
    /// --reaction`), such as 👀, to a message it hadn't woken the session for yet.
    ReactionAwaited,
    /// Landings went onto the default branch since the session's work was built: said
    /// with its next wake, never waking it, for the session to judge whether to rebase.
    DefaultMoved,
    /// A reason a newer zircd gives that this zirc doesn't know: it still concerns the
    /// session.
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|&path| path.to_owned()).collect()
    }

    fn paths(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|&path| path.to_owned()).collect()
    }

    #[test]
    fn work_committed_but_not_landed_overlaps_too() {
        let mut presence = Presence::default();
        let committed = Place {
            committed: set(&["guide.md"]),
            ..Place::default()
        };
        // Everything committed, nothing left uncommitted, as an agent's worktree each turn.
        presence.working_at("#api", "beta", committed, Vec::new(), false);
        assert_eq!(
            presence.overlaps("#api", &set(&["guide.md", "a.rs"])),
            [Overlap {
                channel: "#api".into(),
                nick: "beta".into(),
                paths: paths(&["guide.md"]),
                ..Overlap::default()
            }]
        );
    }

    #[test]
    fn finds_overlaps_with_each_member() {
        let mut presence = Presence::default();
        presence.working_at(
            "#api",
            "beta",
            Place::default(),
            paths(&["a.rs", "b.rs"]),
            false,
        );
        presence.working_at("#api", "gamma", Place::default(), paths(&["c.rs"]), false);
        presence.working_at("#web", "delta", Place::default(), paths(&["a.rs"]), false);
        assert_eq!(
            presence.overlaps("#api", &set(&["a.rs", "c.rs"])),
            [
                Overlap {
                    channel: "#api".into(),
                    nick: "beta".into(),
                    paths: paths(&["a.rs"]),
                    ..Overlap::default()
                },
                Overlap {
                    channel: "#api".into(),
                    nick: "gamma".into(),
                    paths: paths(&["c.rs"]),
                    ..Overlap::default()
                },
            ]
        );
    }

    #[test]
    fn replaces_sets_and_adds_continued_parts() {
        let mut presence = Presence::default();
        presence.working_at("#api", "beta", Place::default(), paths(&["old.rs"]), false);
        presence.working_at("#api", "beta", Place::default(), paths(&["a.rs"]), false);
        presence.working_at("#api", "beta", Place::default(), paths(&["b.rs"]), true);
        assert_eq!(presence.members("#api")[0].paths, paths(&["a.rs", "b.rs"]));
        // An empty set means nothing to work on: the member is gone.
        presence.working_at("#api", "beta", Place::default(), vec![], false);
        assert!(presence.members("#api").is_empty());
    }

    #[test]
    fn a_worktrees_set_said_alone_replaces_only_its_own() {
        let mut presence = Presence::default();
        let at = |worktree: &str| Place {
            worktree: Some(worktree.into()),
            branch: Some(format!("{worktree}-b")),
            session: Some(format!("cc-{worktree}")),
            ..Place::default()
        };
        presence.working_at("#api", "beta", at("one"), paths(&["a.rs"]), false);
        presence.working_at("#api", "beta", at("two"), paths(&["b.rs"]), true);
        let alone = |paths: Vec<String>, worktree: &str| {
            Message::new(
                format!("cc-{worktree}"),
                Event::Working {
                    paths,
                    continued: false,
                    worktree: Some(worktree.into()),
                    branch: Some(format!("{worktree}-b")),
                    part: Some(zirc_proto::ALONE),
                    base: None,
                    committed: Vec::new(),
                },
            )
        };
        presence.apply("#api", "beta", &alone(paths(&["c.rs"]), "two"), 10, None);
        let shown = |presence: &Presence| -> Vec<Vec<String>> {
            presence
                .members("#api")
                .into_iter()
                .map(|m| m.paths)
                .collect()
        };
        assert_eq!(shown(&presence), [paths(&["a.rs"]), paths(&["c.rs"])]);
        presence.apply("#api", "beta", &alone(paths(&["d.rs"]), "three"), 11, None);
        assert_eq!(shown(&presence).len(), 3);
        // Committed: that worktree's set is gone, the others stay.
        presence.apply("#api", "beta", &alone(vec![], "one"), 12, None);
        assert_eq!(shown(&presence), [paths(&["d.rs"]), paths(&["c.rs"])]);
        // Committed, by a machine that says what its work is built on: no member, as
        // nothing's uncommitted, but known for its base and what it changes.
        let committed = Message::new(
            "cc-two",
            Event::Working {
                paths: vec![],
                continued: false,
                worktree: Some("two".into()),
                branch: Some("two-b".into()),
                part: Some(zirc_proto::ALONE),
                base: Some("b".repeat(40)),
                committed: paths(&["c.rs"]),
            },
        );
        presence.apply("#api", "beta", &committed, 13, None);
        assert_eq!(shown(&presence), [paths(&["d.rs"])]);
        assert_eq!(
            presence.built("#api"),
            [Built {
                nick: "beta".into(),
                session: Some("cc-two".into()),
                branch: Some("two-b".into()),
                base: "b".repeat(40),
                changed: paths(&["c.rs"]),
            }]
        );
    }

    #[test]
    fn keeps_each_worktrees_set_apart() {
        let mut presence = Presence::default();
        let at = |worktree: &str, branch: &str| Place {
            worktree: Some(worktree.into()),
            branch: Some(branch.into()),
            session: Some(format!("cc-{worktree}")),
            ..Place::default()
        };
        presence.working_at("#api", "beta", at("one", "login"), paths(&["a.rs"]), false);
        presence.working_at("#api", "beta", at("two", "logout"), paths(&["b.rs"]), true);
        let members = presence.members("#api");
        let shown: Vec<(Option<&str>, Option<&str>, &[String])> = members
            .iter()
            .map(|m| {
                (
                    m.branch.as_deref(),
                    m.session.as_deref(),
                    m.paths.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (Some("login"), Some("cc-one"), &paths(&["a.rs"])[..]),
                (Some("logout"), Some("cc-two"), &paths(&["b.rs"])[..]),
            ]
        );
        assert_eq!(
            presence.overlaps("#api", &set(&["a.rs", "b.rs"]))[0]
                .paths
                .len(),
            2
        );
        // The next set replaces both worktrees': the second has committed its changes.
        presence.working_at("#api", "beta", at("one", "login"), paths(&["a.rs"]), false);
        assert_eq!(presence.members("#api").len(), 1);
        // An older zircd's set stands for its whole machine.
        presence.working_at("#api", "gamma", Place::default(), paths(&["c.rs"]), false);
        assert_eq!(presence.members("#api")[1].branch, None);
    }

    #[test]
    fn keeps_a_worktrees_branches_apart() {
        let mut presence = Presence::default();
        let at = |branch: &str, session: Option<&str>| Place {
            worktree: Some("api".into()),
            branch: Some(branch.into()),
            session: session.map(Into::into),
            ..Place::default()
        };
        let one = at("login", Some("cc-one"));
        let two = at("logout", Some("cc-two"));
        let nobody = at("main", None);
        presence.working_at("#api", "beta", one, paths(&["a.rs"]), false);
        presence.working_at("#api", "beta", two, paths(&["b.rs"]), true);
        presence.working_at("#api", "beta", nobody, paths(&["c.rs"]), true);
        let members = presence.members("#api");
        let shown: Vec<(Option<&str>, Option<&str>, &[String])> = members
            .iter()
            .map(|m| {
                (
                    m.branch.as_deref(),
                    m.session.as_deref(),
                    m.paths.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (Some("login"), Some("cc-one"), &paths(&["a.rs"])[..]),
                (Some("logout"), Some("cc-two"), &paths(&["b.rs"])[..]),
                (Some("main"), None, &paths(&["c.rs"])[..]),
            ]
        );
    }

    #[test]
    fn forgets_those_who_leave() {
        let mut presence = Presence::default();
        presence.working_at("#api", "beta", Place::default(), paths(&["a.rs"]), false);
        presence.working_at("#web", "beta", Place::default(), paths(&["b.rs"]), false);
        presence.left("#api", "beta");
        assert!(presence.members("#api").is_empty());
        presence.renamed("beta", "beta_");
        assert_eq!(presence.members("#web")[0].nick, "beta_");
        presence.quit("beta_");
        assert!(presence.members("#web").is_empty());
    }

    #[test]
    fn keeps_track_of_what_sessions_take_on() {
        let mut presence = Presence::default();
        presence.posted("#api", "alpha", "cc-1", "taking: the login flow", 100);
        presence.posted("#api", "beta", "cc-2", "taking:  releases ", 50);
        presence.posted("#api", "beta", "cc-3", "just saying hi", 60);
        let tasks: Vec<(String, String)> = presence
            .claims("#api")
            .into_iter()
            .map(|claim| (claim.session, claim.task))
            .collect();
        assert_eq!(
            tasks,
            [
                ("cc-2".to_owned(), "releases".to_owned()),
                ("cc-1".to_owned(), "the login flow".to_owned())
            ]
        );
        // Taking something else replaces a session's claim; done or dropped ends it.
        presence.posted("#api", "alpha", "cc-1", "taking: the logout flow", 110);
        assert_eq!(presence.claims("#api")[1].task, "the logout flow");
        presence.posted("#api", "alpha", "cc-1", "done: the logout flow", 120);
        presence.posted("#api", "beta", "cc-2", "dropped: releases", 130);
        assert!(presence.claims("#api").is_empty());
    }

    #[test]
    fn forgets_the_claims_of_those_who_leave() {
        let mut presence = Presence::default();
        presence.posted("#api", "beta", "cc-2", "taking: releases", 1);
        presence.renamed("beta", "beta_");
        assert_eq!(presence.claims("#api")[0].nick, "beta_");
        presence.quit("beta_");
        assert!(presence.claims("#api").is_empty());
    }

    fn intent(nick: &str, session: &str, paths: &[&str], expires: Option<u64>) -> Intent {
        Intent {
            nick: nick.into(),
            session: session.into(),
            session_name: None,
            session_tool: None,
            session_avatar: None,
            summary: format!("{session}'s plan"),
            paths: self::paths(paths),
            expires,
        }
    }

    #[test]
    fn a_reason_from_a_newer_zircd_still_reads() {
        let why: Why = serde_json::from_str("\"from-the-future\"").unwrap();
        assert_eq!(why, Why::Other);
        let why: Why = serde_json::from_str("\"awaited\"").unwrap();
        assert_eq!(why, Why::Awaited);
    }

    #[test]
    fn keeps_intents_until_they_expire_or_their_session_is_done() {
        let mut presence = Presence::default();
        presence.intended("#api", intent("alpha", "cc-1", &["a.rs"], Some(100)), 0);
        presence.intended("#api", intent("beta", "cc-2", &["b.rs"], None), 0);
        assert_eq!(presence.intents("#api", 50).len(), 2);
        // A session's next intent replaces its last.
        presence.intended("#api", intent("alpha", "cc-1", &["c.rs"], Some(100)), 0);
        assert_eq!(presence.intents("#api", 50)[0].paths, paths(&["c.rs"]));
        assert_eq!(presence.intents("#api", 100).len(), 1);
        presence.posted("#api", "beta", "cc-2", "done: b", 110);
        assert!(presence.intents("#api", 110).is_empty());

        presence.intended("#api", intent("beta", "cc-2", &["b.rs"], None), 120);
        presence.renamed("beta", "beta_");
        assert_eq!(presence.intents("#api", 120)[0].nick, "beta_");
        presence.quit("beta_");
        assert!(presence.intents("#api", 120).is_empty());
    }

    #[test]
    fn an_intent_that_says_its_done_or_dropped_ends_at_once() {
        let mut presence = Presence::default();
        for (session, summary) in [
            ("cc-1", "done: presence.rs"),
            ("cc-2", " Dropped: the login page"),
        ] {
            let mut finished = intent("alpha", session, &["presence.rs"], Some(1800));
            finished.summary = summary.into();
            presence.intended("#api", finished, 0);
        }
        assert!(presence.intents("#api", 10).is_empty());
    }

    #[test]
    fn a_directory_touches_what_is_in_it() {
        assert!(touches("crates/zircd", "crates/zircd/src/daemon.rs"));
        assert!(touches("crates/zircd/src/daemon.rs", "crates/zircd/"));
        assert!(touches("a.rs", "a.rs"));
        assert!(!touches("crates/zirc", "crates/zircd/src/daemon.rs"));
        assert!(!touches("a.rs", "b.rs"));
        assert_eq!(
            touching(&paths(&["a.rs", "web/x.ts", "b.rs"]), &["web", "b.rs"]),
            paths(&["web/x.ts", "b.rs"])
        );
    }

    #[test]
    fn remembers_the_latest_landings() {
        let mut presence = Presence::default();
        for at in 0..7 {
            presence.landed(
                "#api",
                Landing {
                    nick: "alpha".into(),
                    session: "cc-1".into(),
                    branch: "main".into(),
                    subjects: vec![format!("Change {at}")],
                    paths: Vec::new(),
                    at,
                },
            );
        }
        presence.clear();
        let landings = presence.landings("#api");
        assert_eq!(landings.len(), LANDINGS);
        assert_eq!(landings[0].at, 6);
    }

    fn working(paths: &[&str], worktree: &str, part: [u32; 2]) -> Message {
        Message::new(
            "cc-1",
            Event::Working {
                paths: self::paths(paths),
                continued: part[0] > 1,
                worktree: Some(worktree.into()),
                branch: Some(format!("{worktree}-branch")),
                part: Some(part),
                base: None,
                committed: Vec::new(),
            },
        )
    }

    #[test]
    fn takes_a_numbered_working_set_whole_or_not_at_all() {
        let mut presence = Presence::default();
        let apply = |presence: &mut Presence, message: Message| {
            presence.apply("#api", "beta", &message, 0, Some("main"));
        };
        apply(&mut presence, working(&["a.rs"], "one", [1, 2]));
        // Nothing yet: the set isn't whole.
        assert!(presence.members("#api").is_empty());
        apply(&mut presence, working(&["b.rs"], "two", [2, 2]));
        assert_eq!(presence.members("#api").len(), 2);
        assert_eq!(presence.members("#api")[0].session.as_deref(), Some("cc-1"));

        // A set missing its first part, as a replay can start halfway, changes nothing.
        apply(&mut presence, working(&["c.rs"], "two", [2, 2]));
        assert_eq!(presence.members("#api").len(), 2);
        // Nor does one missing a middle part.
        apply(&mut presence, working(&["d.rs"], "one", [1, 3]));
        apply(&mut presence, working(&["e.rs"], "three", [3, 3]));
        assert_eq!(presence.members("#api")[1].paths, paths(&["b.rs"]));

        // A machine with nothing to change says so in one part.
        apply(&mut presence, working(&[], "none", [1, 1]));
        assert!(presence.members("#api").is_empty());
    }

    #[test]
    fn times_an_intent_from_when_it_was_sent() {
        let mut presence = Presence::default();
        let intent = Message::new(
            "cc-1",
            Event::Intent {
                summary: "split the login page".into(),
                paths: paths(&["web/login"]),
                expires: Some(1),
                lasts: Some(600),
            },
        );
        // Replayed from history: sent at 1000, by a sender whose clock said 1.
        presence.apply("#api", "beta", &intent, 1000, None);
        assert_eq!(presence.intents("#api", 1500)[0].expires, Some(1600));
        assert!(presence.intents("#api", 1600).is_empty());
    }
}

/// The client tag of a `TAGMSG` an app sends a channel on joining it, or on attaching to an
/// account that's always in it, asking each machine there to say what it's working on: a
/// working set goes as a `TAGMSG` ([`working_line`]), which no history keeps, and an app
/// attaching joins nothing, so nobody else would know to say it again.
pub const ASK_TAG: &str = "+zirc/presence";
const ASK: &str = "ask";

/// The `TAGMSG` that says `message`, a part of a machine's working set, in `channel`, kept
/// out of the channel's history: three lines in four of a busy channel's history were
/// working sets, and an app's 200 lines then went back 46 minutes. `None` when the message
/// can't be written.
#[must_use]
pub fn working_line(channel: &str, message: &Message) -> Option<Line> {
    let payload = zirc_proto::payload(message).ok()?;
    let mut line = Line::new("TAGMSG", [channel]);
    line.tags = vec![(zirc_proto::EVENT_TAG.to_owned(), Some(payload))];
    Some(line)
}

/// A `TAGMSG` carrying a live event ([`Message::is_live`]: debug, such as a working set)
/// as the PRIVMSG it stands for, its sender, id, time and [`zirc_proto::EVENT_TAG`] kept:
/// the tag marks it as live, which a working set in a plain PRIVMSG never is any more
/// ([`old_working`]). `None` for any other line.
#[must_use]
pub fn as_privmsg(line: &Line) -> Option<Line> {
    if line.command != "TAGMSG" {
        return None;
    }
    let message = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG)?).ok()?;
    if !message.is_live() {
        return None;
    }
    let target = line.params.first()?.clone();
    let body = zirc_proto::encode_up_to(&message, usize::MAX).ok()?;
    Some(Line {
        tags: line.tags.clone(),
        source: line.source.clone(),
        command: "PRIVMSG".to_owned(),
        params: vec![target, body],
    })
}

/// Whether `line` is a working set in a plain PRIVMSG, the form zircds before 0.1.24 sent
/// and history still holds: never what a machine is working on now, so it's left out.
#[must_use]
pub fn old_working(line: &Line) -> bool {
    line.command == "PRIVMSG"
        && line.tag(zirc_proto::EVENT_TAG).is_none()
        && line.params.get(1).is_some_and(|body| {
            zirc_proto::decode(body)
                .ok()
                .flatten()
                .is_some_and(|message| matches!(message.event, Event::Working { .. }))
        })
}

/// The line `message`, said by `nick` in `channel`, reaches other clients as: a live event
/// ([`Message::is_live`]) as its `TAGMSG` ([`working_line`]), anything else as a PRIVMSG.
/// What tests feed a client to stand for a machine's message. `None` when the message
/// can't be written.
#[must_use]
pub fn line_from(nick: &str, channel: &str, message: &Message) -> Option<Line> {
    let mut line = if message.is_live() {
        working_line(channel, message)?
    } else {
        let body = zirc_proto::encode(message).ok()?;
        Line::new("PRIVMSG", [channel, body.as_str()])
    };
    line.source = Some(format!("{nick}!u@h"));
    Some(line)
}

/// The `TAGMSG` asking `channel`'s machines what they're working on ([`ASK_TAG`]).
#[must_use]
pub fn ask(channel: &str) -> Line {
    let mut line = Line::new("TAGMSG", [channel]);
    line.tags = vec![(ASK_TAG.to_owned(), Some(ASK.to_owned()))];
    line
}

/// Whether `line` asks what machines are working on ([`ask`]).
#[must_use]
pub fn asks(line: &Line) -> bool {
    line.command == "TAGMSG" && line.tag(ASK_TAG) == Some(ASK)
}

#[cfg(test)]
mod tagmsg_tests {
    use super::*;

    fn working() -> Message {
        Message::new(
            "cc-1",
            Event::Working {
                paths: vec!["oss/a.rs".into()],
                continued: false,
                worktree: Some("one".into()),
                branch: Some("b".into()),
                part: Some([1, 1]),
                base: None,
                committed: Vec::new(),
            },
        )
    }

    #[test]
    fn a_working_set_goes_as_a_tagmsg_and_reads_as_its_privmsg() {
        let mut line = working_line("#zirc", &working()).unwrap();
        assert_eq!(line.command, "TAGMSG");
        line.source = Some("ada-mbp!u@h".into());
        line.tags.push(("msgid".into(), Some("m1".into())));
        let read = as_privmsg(&line).unwrap();
        assert_eq!(read.command, "PRIVMSG");
        assert_eq!(read.source.as_deref(), Some("ada-mbp!u@h"));
        assert_eq!(read.tag("msgid"), Some("m1"));
        assert!(read.tag(zirc_proto::EVENT_TAG).is_some());
        assert!(!old_working(&read));
        assert_eq!(read.params[0], "#zirc");
        let message = zirc_proto::decode(&read.params[1]).unwrap().unwrap();
        assert_eq!(message.event, working().event);
    }

    #[test]
    fn only_debug_is_read_from_a_tagmsg() {
        // zircd's own intent is debug, so it comes live and reads as its PRIVMSG.
        let auto = Message {
            level: Some(zirc_proto::Level::Debug),
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
        let read = as_privmsg(&line_from("mbp", "#zirc/agents", &auto).unwrap()).unwrap();
        assert_eq!(read.command, "PRIVMSG");
        let text = Message::new("cc-1", Event::Text { body: "hi".into() });
        let mut line = Line::new("TAGMSG", ["#zirc"]);
        line.tags = vec![(
            zirc_proto::EVENT_TAG.to_owned(),
            Some(zirc_proto::payload(&text).unwrap()),
        )];
        assert!(as_privmsg(&line).is_none());
        let mut react = Line::new("TAGMSG", ["#zirc"]);
        react.tags = vec![("+draft/react".into(), Some("👍".into()))];
        assert!(as_privmsg(&react).is_none());
        assert!(!asks(&react));
        assert!(asks(&ask("#zirc")));
    }

    #[test]
    fn a_working_set_in_a_plain_privmsg_is_the_old_form() {
        let body = zirc_proto::encode(&working()).unwrap();
        let old = Line::new("PRIVMSG", ["#zirc", body.as_str()]);
        assert!(old_working(&old));
        let text =
            zirc_proto::encode(&Message::new("cc-1", Event::Text { body: "hi".into() })).unwrap();
        assert!(!old_working(&Line::new(
            "PRIVMSG",
            ["#zirc", text.as_str()]
        )));
        assert!(!old_working(&Line::new("PRIVMSG", ["#zirc", "hello"])));
    }
}
