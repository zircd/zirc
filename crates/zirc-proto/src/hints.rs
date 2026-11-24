//! Hints' types: a hint says what to do when `zirc watch` prints a kind of event
//! ([`kind`]), with optional conditions ([`When`]) and a [`Level`], from a [`Layer`]. Said
//! here, in the protocol, so zirc's core, which shows them, and the server's sidecar, which
//! keeps a team's, read them alike; the core reads them from TOML and resolves which applies.
//!
//! A team's own hints, for a repository or the whole organisation, are kept on its server
//! ([`Stored`], listed as a [`Listing`]): its people and their agents add and remove them,
//! each change logged ([`Change`]) and said as a [`Changed`] event by zirc's own account, in the
//! repository's channel or `#ops`, which tells every machine to read them again.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use ts_rs::TS;

/// The kinds of events a hint can be for.
pub mod kind {
    /// A session's first `zirc intend` of its work, shown once: what holds for all of it,
    /// such as what its user wants to be asked about ("Ask me about").
    pub const START: &str = "start";
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
    /// CI: a build failed on a branch you pushed, which zircd waits on for you.
    pub const BUILD_FAILED: &str = "build.yours.failed";
    /// CI: a test failure your user sent to see you wake (`zirc ci test-failure`).
    pub const BUILD_TEST: &str = "build.yours.test";
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
    pub const ALL: [&str; 38] = [
        START,
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
        BUILD_FAILED,
        BUILD_TEST,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "HintLevel")]
pub enum Level {
    /// Do what it says.
    Act,
    /// Tell the user in one line, then carry on.
    Tell,
    /// Nothing to do: no need to wake for it.
    Quiet,
}

impl Level {
    /// Every level, as a form offers them.
    pub const ALL: [Self; 3] = [Self::Act, Self::Tell, Self::Quiet];

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields, rename = "HintWhen")]
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

    /// The conditions `given` says, each `name=value` as `zirc hints add --when` takes them,
    /// such as `lockfile=true` or `branch=api-*`.
    ///
    /// # Errors
    ///
    /// When one isn't `name=value`, names no condition, or gives a yes-or-no one another value.
    pub fn parse(given: &[String]) -> Result<Self, String> {
        let mut when = Self::default();
        for condition in given {
            let Some((name, value)) = condition.split_once('=') else {
                return Err("a condition is name=value, such as lockfile=true".to_owned());
            };
            let yes = || match value {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(format!("{name} is true or false")),
            };
            match name {
                "names_me" => when.names_me = Some(yes()?),
                "lockfile" => when.lockfile = Some(yes()?),
                "branch" => when.branch = Some(value.to_owned()),
                "channel" => when.channel = Some(value.to_owned()),
                "emoji" => when.emoji = Some(value.to_owned()),
                _ => {
                    return Err(format!(
                        "no condition {name}: there are names_me, lockfile, branch, channel and emoji"
                    ));
                }
            }
        }
        Ok(when)
    }

    /// Its conditions in words, as a hint's line says them: `lockfile = true, branch = api-*`;
    /// empty without any.
    #[must_use]
    pub fn words(&self) -> String {
        self.conditions().join(", ")
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Hint {
    /// The kind of event it's for ([`kind`]).
    pub kind: String,
    /// When it applies, beyond its kind.
    #[serde(default, skip_serializing_if = "When::is_empty")]
    #[ts(as = "Option<When>", optional)]
    pub when: When,
    /// What it asks.
    pub level: Level,
    /// What it says, one line, with `{field}` placeholders ([`FIELDS`]).
    pub text: String,
    /// Whether a layer above may not override it: the repository's and organisation's only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
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
    let conditions = hint.when.words();
    let when = if conditions.is_empty() {
        String::new()
    } else {
        format!(" when {conditions}")
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

/// The scope of an organisation's own hints, rather than one repository's (`#<repo>`).
pub const ORG: &str = "org";

/// Whether `scope` is one hints are kept for: [`ORG`], or a repository's channel such as
/// `#api`, in lower case.
#[must_use]
pub fn is_scope(scope: &str) -> bool {
    scope == ORG
        || scope.strip_prefix('#').is_some_and(|name| {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c))
        })
}

/// The layer `scope`'s hints are: the organisation's, or a repository's.
#[must_use]
pub fn layer_of(scope: &str) -> Layer {
    if scope == ORG {
        Layer::Organisation
    } else {
        Layer::Repository
    }
}

/// ` [hint 0123abcd]`, the start of a hint's id ([`crate::short_id`]), to name it by: what
/// `zirc hints remove` takes.
#[must_use]
pub fn tag(id: &str) -> String {
    format!(" [hint {}]", crate::short_id(id))
}

/// Whether `session` could be an agent session's id, as a change to the hints names who asked:
/// letters, digits, dashes, underscores and dots, which need no escaping in an address.
#[must_use]
pub fn is_session(session: &str) -> bool {
    (1..=64).contains(&session.len())
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// One of a team's hints, as its server keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "StoredHint")]
pub struct Stored {
    /// Its id ([`Hint::id`]).
    pub id: String,
    /// What it's for: [`ORG`] or a repository's channel ([`is_scope`]).
    pub scope: String,
    /// The hint.
    pub hint: Hint,
    /// Who added it: the account, a person's or a machine's.
    pub by: String,
    /// The agent session that added it, when a machine's did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session: Option<String>,
    /// When, in RFC 3339.
    pub at: String,
}

impl Stored {
    /// Its layer.
    #[must_use]
    pub fn layer(&self) -> Layer {
        layer_of(&self.scope)
    }

    /// Whom it's for, in words: `the organisation`, or its repository's channel.
    #[must_use]
    pub fn whose(&self) -> &str {
        if self.scope == ORG {
            "the organisation"
        } else {
            &self.scope
        }
    }
}

/// A team's hints, as its server lists them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HintListing")]
pub struct Listing {
    /// How many changes there have been: a machine reads them again when it's told of a newer
    /// one ([`Changed`]).
    pub version: u64,
    /// The hints, oldest first.
    pub hints: Vec<Stored>,
}

impl Listing {
    /// The one whose id starts with `id`, as `[hint …]` shows it.
    ///
    /// # Errors
    ///
    /// When none does, or more than one, saying so.
    pub fn by_id(&self, id: &str) -> Result<&Stored, String> {
        let id = id
            .trim()
            .trim_start_matches("[hint ")
            .trim_end_matches(']')
            .trim();
        let found: Vec<&Stored> = self
            .hints
            .iter()
            .filter(|stored| !id.is_empty() && stored.id.starts_with(id))
            .collect();
        match found.as_slice() {
            [stored] => Ok(stored),
            [] => Err(format!(
                "the server keeps no hint with the id {id}: zirc hints lists them"
            )),
            _ => Err(format!(
                "{} hints' ids start {id}: say more of it",
                found.len()
            )),
        }
    }

    /// Those for `scope`.
    #[must_use]
    pub fn for_scope(&self, scope: &str) -> Vec<Hint> {
        self.hints
            .iter()
            .filter(|stored| stored.scope.eq_ignore_ascii_case(scope))
            .map(|stored| stored.hint.clone())
            .collect()
    }
}

/// What a change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Added the hint.
    Added,
    /// Removed it.
    Removed,
}

/// One change to a team's hints, as its server logs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// What it did.
    pub action: Action,
    /// The hint it added or removed.
    pub hint: Stored,
    /// Who did it, as [`Stored::by`].
    pub by: String,
    /// The agent session that did it, when a machine's did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// When, in RFC 3339.
    pub at: String,
    /// The version it made ([`Listing::version`]).
    pub version: u64,
}

/// The kind of the event that says a team's hints changed ([`Changed`]).
pub const CHANGED_KIND: &str = "x-zirc/hints-changed";

/// That a team's hints changed, said by zirc's own account
/// ([`crate::names::SYSTEM`]): what a machine takes, from that account alone, to read them again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changed {
    /// The version the change made.
    pub version: u64,
    /// What it did.
    pub action: Action,
    /// The hint's scope.
    pub scope: String,
    /// The hint's id.
    pub id: String,
    /// Who did it.
    pub by: String,
}

impl Changed {
    /// What `change` says.
    #[must_use]
    pub fn of_change(change: &Change) -> Self {
        Self {
            version: change.version,
            action: change.action,
            scope: change.hint.scope.clone(),
            id: change.hint.id.clone(),
            by: change.by.clone(),
        }
    }

    /// As its event, saying in words for people what changed.
    ///
    /// # Panics
    ///
    /// Never: it serializes to JSON.
    #[must_use]
    pub fn event(&self, change: &Change) -> crate::Event {
        let who = match &change.session {
            Some(session) => format!("{}/{session}", change.by),
            None => change.by.clone(),
        };
        let did = match change.action {
            Action::Added => "added",
            Action::Removed => "removed",
        };
        let hint = &change.hint;
        crate::Event::Custom {
            kind: CHANGED_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a change serializes"),
            text: format!(
                "{who} {did} a hint: {}{}",
                line(hint.layer(), &hint.hint),
                tag(&hint.id)
            ),
        }
    }

    /// The change `event` says, if it says one.
    #[must_use]
    pub fn of(event: &crate::Event) -> Option<Self> {
        match event {
            crate::Event::Custom { kind, data, .. } if kind == CHANGED_KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }
}

/// The TypeScript declarations of what zirc.dev's Playbook page reads of a team's hints.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Level::decl(config),
        When::decl(config),
        Hint::decl(config),
        Stored::decl(config),
        Listing::decl(config),
        format!("const HINTS_ORG_SCOPE = {ORG:?};"),
        format!("const HINT_KINDS: readonly string[] = {:?};", kind::ALL),
        format!(
            "const HINT_LEVELS: readonly HintLevel[] = {:?};",
            Level::ALL.map(Level::name)
        ),
        format!("const HINT_FIELDS: readonly string[] = {FIELDS:?};"),
    ]
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

    #[test]
    fn a_scope_is_the_organisation_or_a_repositorys_channel() {
        assert!(is_scope(ORG) && is_scope("#api") && is_scope("#web-app"));
        assert!(!is_scope("#") && !is_scope("api") && !is_scope("#API") && !is_scope("#a b"));
    }

    #[test]
    fn a_change_says_what_it_did_and_round_trips() {
        let stored = Stored {
            id: "0123456789abcdef".into(),
            scope: "#api".into(),
            hint: hint("overlap", Level::Tell, "ask first"),
            by: "ada-mbp".into(),
            session: None,
            at: "2026-11-16T10:00:00Z".into(),
        };
        let change = Change {
            action: Action::Added,
            hint: stored.clone(),
            by: "ada-mbp".into(),
            session: Some("cc-1".into()),
            at: stored.at.clone(),
            version: 3,
        };
        let changed = Changed::of_change(&change);
        let event = changed.event(&change);
        assert_eq!(Changed::of(&event), Some(changed));
        let crate::Event::Custom { text, .. } = &event else {
            unreachable!()
        };
        assert_eq!(
            text,
            "ada-mbp/cc-1 added a hint: overlap (tell, repo): ask first [hint 01234567]"
        );
        let listing = Listing {
            version: 3,
            hints: vec![stored],
        };
        assert_eq!(listing.for_scope("#API").len(), 1);
        assert_eq!(listing.by_id("[hint 0123]").map(Stored::whose), Ok("#api"));
        assert!(
            listing
                .by_id("ffff")
                .unwrap_err()
                .contains("no hint with the id")
        );
        assert!(listing.by_id("").is_err());
        assert!(is_session("cc-1a2b3c") && !is_session("") && !is_session("a b"));
        let when = When::parse(&["lockfile=true".into(), "branch=api-*".into()]).unwrap();
        assert_eq!(when.words(), "lockfile = true, branch = api-*");
        assert!(
            When::parse(&["nope=1".into()])
                .unwrap_err()
                .contains("no condition nope")
        );
        assert!(When::parse(&["lockfile=maybe".into()]).is_err());
        assert!(listing.for_scope(ORG).is_empty());
    }
}
