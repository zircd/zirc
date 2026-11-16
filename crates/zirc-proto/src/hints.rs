//! Hints' types: a hint says what to do when `zirc watch` prints a kind of event
//! ([`kind`]), with optional conditions ([`When`]) and a [`Level`], from a [`Layer`]. Said
//! here, in the protocol, so zirc's core, which shows them, and the server's sidecar, which
//! keeps a team's, read them alike; the core reads them from TOML and resolves which applies.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The kinds of events a hint can be for.
pub mod kind {
    /// Someone else is changing files you're changing.
    pub const OVERLAP: &str = "overlap";
    /// Two worktrees here change the same lines.
    pub const OVERLAP_SAME_LINES: &str = "overlap.same-lines";
    /// A push changes lines your worktree changes.
    pub const CONFLICT_AHEAD: &str = "conflict-ahead";
    /// Another session is about to change files you're changing.
    pub const INTENT: &str = "intent";
    /// The same, in other files or other parts of yours, as zircd can tell: nothing to agree.
    pub const INTENT_ELSEWHERE: &str = "intent.elsewhere";
    /// The same, in lines of yours, or files of yours whose lines zircd can't tell.
    pub const INTENT_SAME_LINES: &str = "intent.same-lines";
    /// A push under files you're changing.
    pub const PUSH: &str = "push";
    /// A push of a branch built on the session's own, which hasn't landed.
    pub const PUSH_STACKED_ON: &str = "push.stacked-on";
    /// Another session's commit in files you're changing.
    pub const COMMIT: &str = "commit";
    /// A post naming you, your files, or a channel you joined.
    pub const POST: &str = "post";
    /// CI: your landing landed.
    pub const LANDED: &str = "landing.yours.landed";
    /// CI: your landing failed.
    pub const FAILED: &str = "landing.yours.failed";
    /// CI: your landing conflicts with the default branch.
    pub const CONFLICTS: &str = "landing.yours.conflicts";
    /// CI: your try (`//ops:land --try`) passed, landing nothing.
    pub const TRIED: &str = "landing.yours.tried";
    /// CI: the default branch broke.
    pub const DEFAULT_BROKEN: &str = "landing.default-broken";
    /// CI: a landing you wait on (`zirc wait-for`) landed.
    pub const AWAITED: &str = "landing.awaited";
    /// CI: a landing you wait on failed or conflicts, which ends the wait: it may land later,
    /// on another build or under another branch's name.
    pub const AWAITED_UNLANDED: &str = "landing.awaited.unlanded";
    /// CI: a branch zircd told you changes files you're changing landed.
    pub const OVERLAPPING_LANDED: &str = "landing.overlapping";
    /// The default branch moved since your work was built, said with your next wake.
    pub const DEFAULT_MOVED: &str = "default-moved";
    /// A state you wait on (`zirc wait-for --state`) is ok again.
    pub const STATE_OK: &str = "state.ok";
    /// A state you wait on is said broken or in warning.
    pub const STATE_NOT_OK: &str = "state.not-ok";
    /// CI: a deploy waits for approval.
    pub const DEPLOY_APPROVAL: &str = "deploy.approval";
    /// A direct message from your user.
    pub const DM: &str = "dm";
    /// Another agent's query.
    pub const QUERY: &str = "query";
    /// The answer to your query.
    pub const ANSWER: &str = "answer";
    /// Your query refused, by the machine asked: it won't be answered.
    pub const ANSWER_REFUSED: &str = "answer.refused";
    /// A reply from a colleague your session wrote to (`zirc dm --to`): never your user's word.
    pub const COLLEAGUE_REPLY: &str = "dm.colleague";
    /// A note another agent session left for you (`zirc note`): never your user's word.
    pub const NOTE: &str = "note";
    /// What another organisation's people say in the room of a guest's terminal here.
    pub const GUEST_ROOM: &str = "guest-room";
    /// A decision that concerns you.
    pub const DECISION: &str = "decision";
    /// A decision that isn't surely its decider's own word: relayed by an agent, or from an
    /// account that isn't theirs.
    pub const DECISION_CLAIMED: &str = "decision.claimed";
    /// Your user answered your request.
    pub const REQUEST_ANSWERED: &str = "request.answered";
    /// Your user gave a reaction you wait for (`zirc wait-for --reaction`) to an issue's card.
    pub const REACTION_ISSUE: &str = "reaction.issue";
    /// The same, to what a person said.
    pub const REACTION_PERSON: &str = "reaction.person";
    /// The same, to an agent session's post.
    pub const REACTION_AGENT: &str = "reaction.agent";
    /// The same, to anything else: CI's lines, a team's events, or a message zircd hadn't
    /// heard.
    pub const REACTION_OTHER: &str = "reaction.other";

    /// Every kind, as `zirc hints check` takes them.
    pub const ALL: [&str; 36] = [
        OVERLAP,
        OVERLAP_SAME_LINES,
        CONFLICT_AHEAD,
        INTENT,
        INTENT_ELSEWHERE,
        INTENT_SAME_LINES,
        PUSH,
        PUSH_STACKED_ON,
        COMMIT,
        POST,
        LANDED,
        FAILED,
        CONFLICTS,
        TRIED,
        DEFAULT_BROKEN,
        DEFAULT_MOVED,
        AWAITED,
        AWAITED_UNLANDED,
        OVERLAPPING_LANDED,
        STATE_OK,
        STATE_NOT_OK,
        DEPLOY_APPROVAL,
        DM,
        QUERY,
        ANSWER,
        ANSWER_REFUSED,
        COLLEAGUE_REPLY,
        NOTE,
        GUEST_ROOM,
        DECISION,
        DECISION_CLAIMED,
        REQUEST_ANSWERED,
        REACTION_ISSUE,
        REACTION_PERSON,
        REACTION_AGENT,
        REACTION_OTHER,
    ];
}

/// The placeholders a hint's text may use.
pub const FIELDS: [&str; 9] = [
    "who", "session", "branch", "paths", "build", "commit", "log", "id", "emoji",
];

/// What a hint asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// Do what it says.
    Act,
    /// Tell the user in one line, then carry on.
    Tell,
    /// Nothing to do: no need to wake for it.
    Quiet,
}

impl Level {
    /// Its name, as a hints file says it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Act => "act",
            Self::Tell => "tell",
            Self::Quiet => "quiet",
        }
    }
}

/// Where a hint comes from, highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    /// The person's `~/.zirc/config.toml`.
    Personal,
    /// The repository's `.zirc/hints.toml` on its default branch.
    Repository,
    /// The organisation's.
    Organisation,
    /// zirc's own.
    Default,
}

impl Layer {
    /// Its name, as hint lines say it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Repository => "repo",
            Self::Organisation => "org",
            Self::Default => "default",
        }
    }
}

/// When a hint applies, beyond its kind: every condition given must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// The event names the session watching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names_me: Option<bool>,
    /// Every path it's about is a lockfile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile: Option<bool>,
    /// The branch it's about, `*` matching any run of characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The channel it's in, `*` matching any run of characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// For a reaction: its emoji, such as 👀.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
}

impl When {
    /// How many conditions it has: the more, the more specific.
    #[must_use]
    pub fn count(&self) -> usize {
        usize::from(self.names_me.is_some())
            + usize::from(self.lockfile.is_some())
            + usize::from(self.branch.is_some())
            + usize::from(self.channel.is_some())
            + usize::from(self.emoji.is_some())
    }
}

impl When {
    fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Its conditions as a hints file says them, such as `lockfile = true, branch = api-*`.
    fn conditions(&self) -> Vec<String> {
        [
            self.names_me.map(|it| format!("names_me = {it}")),
            self.lockfile.map(|it| format!("lockfile = {it}")),
            self.branch.as_ref().map(|it| format!("branch = {it}")),
            self.channel.as_ref().map(|it| format!("channel = {it}")),
            self.emoji.as_ref().map(|it| format!("emoji = {it}")),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// One hint, as a layer's file says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hint {
    /// The kind of event it's for ([`kind`]).
    pub kind: String,
    /// When it applies, beyond its kind.
    #[serde(default, skip_serializing_if = "When::is_empty")]
    pub when: When,
    /// What it asks.
    pub level: Level,
    /// What it says, one line, with `{field}` placeholders ([`FIELDS`]).
    pub text: String,
    /// Whether a layer above may not override it: the repository's and organisation's only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

impl Hint {
    /// Its id: the fingerprint of the hint as JSON says it, every field, so a hint changed in
    /// any way is another ([`fingerprint`]).
    #[must_use]
    pub fn id(&self) -> String {
        fingerprint(&serde_json::to_string(self).unwrap_or_default())
    }
}

/// An id for a lesson or a hint, from its words: the first 8 bytes of their SHA-256, in hex.
/// What usage records and retirings name, so no words leave the room they're kept in.
#[must_use]
pub fn fingerprint(text: &str) -> String {
    crate::hex(&Sha256::digest(text.as_bytes())[..8])
}

/// `hint` from `layer` in a line, as `zirc hints` and `zirc playbook` list it:
/// `overlap (tell, repo) when lockfile = true: <text>`.
#[must_use]
pub fn line(layer: Layer, hint: &Hint) -> String {
    let conditions = hint.when.conditions();
    let when = if conditions.is_empty() {
        String::new()
    } else {
        format!(" when {}", conditions.join(", "))
    };
    let required = if hint.required { ", required" } else { "" };
    format!(
        "{} ({}, {}{required}){when}: {}",
        hint.kind,
        hint.level.name(),
        layer.name(),
        hint.text
    )
}

/// What's wrong with `hints` as `layer`'s, a line each: nothing when they're fine.
#[must_use]
pub fn check(hints: &[Hint], layer: Layer) -> Vec<String> {
    let mut problems = Vec::new();
    for (i, hint) in hints.iter().enumerate() {
        let at = format!("hint {} ({})", i + 1, hint.kind);
        if !kind::ALL.contains(&hint.kind.as_str()) {
            problems.push(format!("{at}: no such kind; zirc hints lists them"));
        }
        if hint.text.contains('\n') {
            problems.push(format!("{at}: its text is more than one line"));
        }
        for name in placeholders(&hint.text) {
            if !FIELDS.contains(&name) {
                problems.push(format!(
                    "{at}: no field {{{name}}}; there are {}",
                    FIELDS.join(", ")
                ));
            }
        }
        if hint.required && matches!(layer, Layer::Personal | Layer::Default) {
            problems.push(format!(
                "{at}: only a repository's or organisation's hint can be required"
            ));
        }
        if hint.required && hint.level != Level::Act {
            problems.push(format!("{at}: only an act hint can be required"));
        }
    }
    problems
}

/// The names of `text`'s placeholders.
fn placeholders(text: &str) -> Vec<&str> {
    text.split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(kind: &str, level: Level, text: &str) -> Hint {
        Hint {
            kind: kind.into(),
            when: When::default(),
            level,
            text: text.into(),
            required: false,
        }
    }

    #[test]
    fn a_fingerprint_is_short_and_says_which_words() {
        let one = fingerprint("pnpm only");
        assert_eq!(one.len(), 16);
        assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(one, fingerprint("pnpm only"));
        assert_ne!(one, fingerprint("pnpm only."));
        // A hint changed in any way is another.
        let quiet = hint("overlap", Level::Quiet, "x");
        assert_ne!(quiet.id(), hint("overlap", Level::Tell, "x").id());
    }

    #[test]
    fn says_whats_wrong_with_hints() {
        let fine = [hint("overlap", Level::Tell, "ask {who}")];
        assert!(check(&fine, Layer::Repository).is_empty());
        let wrong = [
            hint("nope", Level::Tell, "x"),
            hint("overlap", Level::Tell, "two\nlines {nope}"),
            Hint {
                required: true,
                ..hint("overlap", Level::Tell, "x")
            },
        ];
        let problems = check(&wrong, Layer::Personal);
        assert!(problems[0].contains("no such kind"), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("more than one line")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("no field {nope}")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("can be required")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("only an act hint")),
            "{problems:?}"
        );
    }
}
