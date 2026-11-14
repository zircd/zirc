//! A conversation as zirc's apps show it: what each line *is*, so every app shows the same
//! lines and only styles them. Days are divided, talk from one sender is grouped into runs,
//! zirc's events are messages too, in their sender's runs, and people coming and going are
//! one quiet summary, with a machine's own reconnects left out. What the connection is doing never
//! shows here: that's [`super::Status`].

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use zirc_proto::card::{Card, State};
use zirc_proto::decision::Decision;
use zirc_proto::{Event, File, Message};

use super::body::strip_formatting;
use super::conversation::{Conversation, Entry, EntryKind, fold};

/// What marks a zirc event, in every app.
pub const EVENT_MARK: &str = "◆";

/// A nick that leaves and comes back this soon has only reconnected: neither shows.
pub const RECONNECT_GAP: TimeDelta = TimeDelta::minutes(1);

/// One line of a conversation, as every app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item<'a> {
    /// A day starts. [`day_label`] puts it in words.
    Day(NaiveDate),
    /// Someone said something.
    Message {
        /// What it came from, for its time, sender, id and whether it mentions us.
        entry: &'a Entry,
        /// What they said.
        text: &'a str,
        /// Whether it carries on a run from the same sender, so its sender can be left out.
        continues: bool,
        /// The zirc app it was sent from, such as `desktop`, when the sender's app said.
        sent_from: Option<&'a str>,
        /// Whether to show [`Item::Message::sent_from`]: see [`shows_sent_from`].
        show_sent_from: bool,
    },
    /// A notice: a message that expects no answer.
    Notice {
        /// What it came from.
        entry: &'a Entry,
        /// What it says.
        text: &'a str,
        /// Whether it carries on a run from the same sender.
        continues: bool,
    },
    /// An action, as `/me` sends: `alice waves`.
    Action {
        /// What it came from.
        entry: &'a Entry,
        /// The action, after the sender's nick.
        text: &'a str,
        /// Whether it carries on a run from the same sender, as a message would.
        continues: bool,
    },
    /// A file someone shared, which apps draw inline: an image, a video's poster or a card.
    /// [`super::file`] has what they need to draw it.
    File {
        /// What it came from.
        entry: &'a Entry,
        /// The file.
        file: &'a File,
        /// Who shared it: `nick/session`, or the nick alone.
        from: String,
        /// Whether it carries on a run from the same sender.
        continues: bool,
    },
    /// A zirc event, or several people changing the same files: one line such as
    /// `mattias-mbp/cc-1: committed on main: Fix`, which apps mark with [`EVENT_MARK`].
    Event {
        /// What it came from.
        entry: &'a Entry,
        /// The line, whole, for copying and plain views.
        text: String,
        /// The line in parts, for a view that draws an icon and one line of text with no
        /// header, as Slack draws system rows.
        parts: Parts,
        /// Whether it carries on a run of the same sender's talk, events included, as a
        /// message does ([`Item::Message::continues`]), so its header can be left out.
        continues: bool,
    },
    /// People who came and went, one after another.
    Membership(Membership),
    /// How a landing went, as CI says (`x-zirc/landing`): landed, failed or in conflict.
    Landing(Landing<'a>),
    /// A quiet line: a topic, a new nick, someone removed, or what this client says, such as
    /// an error, when the entry is an [`EntryKind::Info`].
    Note {
        /// What it came from.
        entry: &'a Entry,
        /// The line.
        text: String,
    },
}

impl Item<'_> {
    /// Whether apps highlight it in `conversation` as naming us
    /// ([`super::Conversation::highlights`]): never in a one-to-one conversation, where every
    /// line is for us. Only what someone said or did can; folds and days never do.
    #[must_use]
    pub fn mentions_me(&self, conversation: &super::Conversation) -> bool {
        match self {
            Self::Message { entry, .. }
            | Self::Notice { entry, .. }
            | Self::Action { entry, .. }
            | Self::Event { entry, .. }
            | Self::File { entry, .. } => conversation.highlights(entry),
            _ => false,
        }
    }

    /// What someone said or did, for a message or an action: what can be corrected
    /// ([`super::editable`]).
    #[must_use]
    pub fn said_entry(&self) -> Option<&Entry> {
        match self {
            Self::Message { entry, .. } | Self::Action { entry, .. } => Some(entry),
            _ => None,
        }
    }

    /// When it happened; `None` for a day.
    #[must_use]
    pub fn time(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Day(_) => None,
            Self::Membership(membership) => Some(membership.time),
            Self::Landing(landing) => Some(landing.entry.time),
            Self::Message { entry, .. }
            | Self::Notice { entry, .. }
            | Self::Action { entry, .. }
            | Self::Event { entry, .. }
            | Self::File { entry, .. }
            | Self::Note { entry, .. } => Some(entry.time),
        }
    }
}

/// A zirc event's line: a file as itself, anything else as one line of text.
fn zirc_item<'a>(
    entry: &'a Entry,
    message: &'a Message,
    continues: bool,
    owner_name: &dyn Fn(&str) -> String,
) -> Item<'a> {
    let from = super::sender_text(
        &entry.from,
        entry.owner.as_deref(),
        Some(message),
        owner_name,
    );
    let parts = event_parts(
        &entry.from,
        entry.owner.as_deref(),
        message,
        entry.time,
        owner_name,
    );
    match &message.event {
        // A decision is said as a message is, in Markdown: its title, quote and bullets.
        Event::Custom { text, .. } if Decision::of(&message.event).is_some() => Item::Message {
            entry,
            text,
            continues,
            sent_from: None,
            show_sent_from: false,
        },
        Event::File(file) => Item::File {
            entry,
            file,
            from,
            continues,
        },
        event => {
            let colon = if matches!(event, Event::Text { .. }) {
                ":"
            } else {
                ""
            };
            let text = strip_formatting(&event.human_text());
            Item::Event {
                entry,
                text: format!("{from}{colon} {text}"),
                parts,
                continues,
            }
        }
    }
}

/// Who sends something, for the pill beside their name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderKind {
    /// A person.
    Person,
    /// One of a person's machines, its agents: `<owner>-<machine>`.
    Agent,
    /// One of zirc's own accounts, an app: CI, zirc-alerts. The handle `zirc` is zirc's
    /// alone, so only zirc's accounts are `zirc` or `zirc-…`.
    Integration,
    /// A guest, from outside the organisation: only [`super::Chat::sender_kind`] tells,
    /// as it knows who has no account.
    Guest,
}

impl SenderKind {
    /// The pill apps show beside the sender's name, if any: an agent has none, as its name
    /// line says whose agent it is ([`super::Speaker::detail`]).
    #[must_use]
    pub fn pill(self) -> Option<&'static str> {
        match self {
            Self::Person => Some("PERSON"),
            Self::Agent => None,
            Self::Integration => Some("APP"),
            Self::Guest => Some(super::GUEST_PILL),
        }
    }
}

impl super::Chat {
    /// What `nick` is: a guest when the server let them in as one ([`super::Chat::is_guest`]),
    /// never by their nick alone; otherwise as [`sender_kind`] says, by whose machine its host
    /// says it is ([`super::Chat::machine_owner`]).
    #[must_use]
    pub fn sender_kind(&self, nick: &str) -> SenderKind {
        if self.is_guest(nick) {
            SenderKind::Guest
        } else if self.is_app(nick) {
            SenderKind::Integration
        } else {
            sender_kind(nick, self.machine_owner(nick))
        }
    }
}

/// What `nick` is: zirc's own accounts are integrations, by their names; a machine, whose
/// `owner` its host says (`machine/<owner>`), is an agent; anyone else, a guest among them, is
/// a person. [`super::Chat::sender_kind`] tells guests.
#[must_use]
pub fn sender_kind(nick: &str, owner: Option<&str>) -> SenderKind {
    let lower = nick.to_ascii_lowercase();
    if lower == "zirc" || lower.starts_with("zirc-") {
        return SenderKind::Integration;
    }
    if lower.starts_with(super::GUEST_PREFIX) {
        return SenderKind::Person;
    }
    if owner.is_some() {
        SenderKind::Agent
    } else {
        SenderKind::Person
    }
}

/// The label zirc's own accounts go by, `zirc` itself and its system account
/// ([`zirc_proto::names::SYSTEM`]), which [`super::app_name`] names zirc
/// ([`zirc_proto::NAME`]) in every app.
pub const ZIRC_LABEL: &str = zirc_proto::PROGRAM;

/// Whether `nick` is zirc's system account ([`zirc_proto::names::SYSTEM`]), whose notices and
/// direct messages nobody answers.
#[must_use]
pub fn is_system(nick: &str) -> bool {
    nick.eq_ignore_ascii_case(zirc_proto::names::SYSTEM)
}

/// The short name apps show for `nick` when it's one of zirc's integrations, in place of a
/// generic app badge (`[ci]`, a `CI` badge): the one its `message` gives, or else that
/// message's session, or else its nick's own after `zirc-`. zirc's system account is zirc
/// itself, [`ZIRC_LABEL`], whatever it says. `None` for anyone else.
#[must_use]
pub fn app_label(nick: &str, message: Option<&Message>) -> Option<String> {
    if sender_kind(nick, None) != SenderKind::Integration {
        return None;
    }
    if is_system(nick) {
        return Some(ZIRC_LABEL.to_owned());
    }
    let own = nick
        .get(4..)
        .map_or("", |rest| rest.trim_start_matches('-'));
    message
        .and_then(|message| message.app.as_deref())
        .and_then(zirc_proto::app_label)
        .or_else(|| {
            message
                .map(|message| message.session.as_str())
                .filter(|session| *session != zirc_proto::NO_SESSION)
                .and_then(zirc_proto::app_label)
        })
        .or_else(|| zirc_proto::app_label(own))
        .or_else(|| Some(ZIRC_LABEL.to_owned()))
}

/// What sort of system row it is, for its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum Icon {
    /// A push.
    Push,
    /// A commit.
    Commit,
    /// Uncommitted changes.
    Working,
    /// What a session is about to change.
    Intent,
    /// What a session took on (`taking:`).
    Taking,
    /// A landing that landed.
    Landed,
    /// A landing whose checks failed.
    Failed,
    /// A landing in conflict.
    Conflict,
    /// A file two or more are changing at once ([`super::Chat::both_changing`]).
    Overlap,
    /// Someone came.
    Join,
    /// Someone went.
    Leave,
    /// An agent saying something, or another of zirc's events.
    Post,
}

impl Icon {
    /// The theme role every app colours it in, by zirc-theme's name for it: how a landing
    /// went in `ok` or `error`, a conflict and a file two are changing in `warning`, and
    /// the rest in `muted`. An app looks the role up; it never decides it from the icon.
    #[must_use]
    pub fn tone(self) -> &'static str {
        match self {
            Self::Landed => "ok",
            Self::Failed => "error",
            Self::Conflict | Self::Overlap => "warning",
            Self::Push
            | Self::Commit
            | Self::Working
            | Self::Intent
            | Self::Taking
            | Self::Join
            | Self::Leave
            | Self::Post => "muted",
        }
    }

    /// What every app draws for it, one character wide. A conflict is a warning, `⚠`.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Push | Self::Commit => "●",
            Self::Working => "…",
            Self::Intent => "✎",
            Self::Taking => "⟶",
            Self::Landed => "✓",
            Self::Failed => "✗",
            Self::Conflict => "⚠",
            Self::Overlap => "!",
            Self::Join => "+",
            Self::Leave => "−",
            Self::Post => EVENT_MARK,
        }
    }

    /// The word for it, `intent`, `push`, for where kinds are listed or named. Never a row's
    /// header: that's a message's, who, whose agent and the time, and the marker and verb
    /// say the kind once, in the body ([`Parts::lead`]; the design lead's `STEP3b`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::Commit => "commit",
            Self::Working => "working",
            Self::Intent => "intent",
            Self::Taking => "taking",
            Self::Landed => "landed",
            Self::Failed => "failed",
            Self::Conflict => "conflict",
            Self::Overlap => "overlap",
            Self::Join => "joined",
            Self::Leave => "left",
            Self::Post => "post",
        }
    }
}

/// A system row in parts: an icon, who, what they did and the paths it's about, and when.
/// Apps draw it as a message: a header exactly a message's, the actor, whose agent it is
/// ([`Parts::actor_detail`]) and the time, with no kind word and no AGENT tag; then the
/// body, [`Parts::lead`] (the marker and the verb) and [`Parts::details`], wrapped, never
/// cut; the paths under them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    /// Its icon.
    pub icon: Icon,
    /// Who, as people read it: a person's nick; an agent session by its name, `zirc-chat`,
    /// or else its id, `cc-598aa1`; a machine itself as its owner's agent, `dana's agent`.
    /// Never the device.
    pub actor: String,
    /// For an agent session, its id, `cc-598aa1`, which apps show on hover.
    pub session: Option<String>,
    /// For an agent, whose it is, `dana`: apps draw their photo with a robot badge.
    pub owner: Option<String>,
    /// What follows [`Parts::actor`], muted: `dana's agent` for an agent session, as
    /// [`super::agent_name`] puts it, with the owner's nick. [`super::Chat::speaker`] says
    /// it with their profile's name.
    pub actor_detail: Option<String>,
    /// Whether apps draw it as one quiet line: only people coming and going are. Every agent
    /// and system event is drawn as a message is, its actor's avatar, name and time heading
    /// a run of their talk, the verb and details as its body, wrapped, never cut with `…`
    /// (the user's decision, 5 Nov). A landing's card has its own layout. Apps draw this
    /// and don't decide it themselves.
    pub compact: bool,
    /// For a post addressed to an agent session (`cc-598aa1: …`), that session's id: apps
    /// show `→ zirc-chat` ([`super::Chat::session_label`]), and [`Parts::details`] holds
    /// the text after it.
    pub to: Option<String>,
    /// What they did, in a few words: `is about to change`, `committed`, `pushed login to
    /// abc1234`, or nothing for a post.
    pub verb: String,
    /// What it was, under the header: an intent's summary, a commit's subject and where,
    /// a post's text; empty for a short event.
    pub details: String,
    /// The paths it's about, when it's about any.
    pub paths: Vec<String>,
    /// Words in [`Parts::verb`] and [`Parts::details`] that are links, as `(word,
    /// address)`: a commit's or push's branch and commit, to their pages on the
    /// repository's forge. Apps draw them with [`super::link_words`].
    pub links: Vec<(String, String)>,
    /// When it happened: the first of a fold.
    pub time: DateTime<Utc>,
    /// When the last of a fold happened; the same as [`Parts::time`] for one row.
    pub until: DateTime<Utc>,
}

impl Parts {
    /// What its body starts with, as every app draws it: its marker ([`Icon::glyph`]) and
    /// what was done, `✎ is about to change`; the marker alone when nothing was, and
    /// nothing for a post, which is its words.
    #[must_use]
    pub fn lead(&self) -> String {
        match (self.icon, self.verb.is_empty()) {
            (Icon::Post, _) => String::new(),
            (icon, true) => icon.glyph().to_owned(),
            (icon, false) => format!("{} {}", icon.glyph(), self.verb),
        }
    }
}

/// Who `nick` is as a system row says it: one of zirc's apps by its name ([`super::app_name`]),
/// `CI`; a machine, whose `owner` its host says, as its owner's agent, the owner as
/// `owner_name` says ([`super::Chat::owner_name`]); anyone else, a guest among them, by
/// their nick.
#[must_use]
pub fn actor(nick: &str, owner: Option<&str>, owner_name: &dyn Fn(&str) -> String) -> String {
    match (sender_kind(nick, owner), owner) {
        (SenderKind::Integration, _) => {
            super::app_name(&app_label(nick, None).unwrap_or_else(|| "zirc".to_owned()))
        }
        (SenderKind::Agent, Some(owner)) => super::agent_name(None, &owner_name(owner)).0,
        _ => nick.to_owned(),
    }
}

/// A commit's or push's branch and short commit, each to its page on the repository's
/// forge, when the event says which repository.
fn event_links(event: &Event) -> Vec<(String, String)> {
    let (repo, branch, sha) = match event {
        Event::Commit {
            repo: Some(repo),
            branch,
            sha,
            ..
        } => (repo, branch, sha),
        Event::Push {
            repo: Some(repo),
            branch,
            head,
            ..
        } => (repo, branch, head),
        _ => return Vec::new(),
    };
    vec![
        (
            super::describe::short(sha).to_owned(),
            super::commit_link(repo, sha),
        ),
        (branch.clone(), super::branch_link(repo, branch)),
    ]
}

/// The agent that said `message`, as `nick`, whose `owner` its host says: its owner and, for
/// one of its sessions, the session's id and what to call it, its name or else its id.
fn agent_of(
    nick: &str,
    owner: Option<&str>,
    message: &Message,
) -> Option<(String, Option<(String, String)>)> {
    let owner = owner?;
    if sender_kind(nick, Some(owner)) != SenderKind::Agent {
        return None;
    }
    let named = super::agent_session(nick, message).map(|session| {
        let name = message
            .name
            .as_deref()
            .and_then(zirc_proto::session_name)
            .unwrap_or_else(|| session.to_owned());
        (session.to_owned(), name)
    });
    Some((owner.to_owned(), named))
}

/// `message`'s parts, said by `nick` at `time`, an agent's owner as `owner_name` says.
pub(super) fn event_parts(
    nick: &str,
    owner: Option<&str>,
    message: &Message,
    time: DateTime<Utc>,
    owner_name: &dyn Fn(&str) -> String,
) -> Parts {
    let event = &message.event;
    let described = super::describe::describe(event);
    let said = |text: &str| (String::new(), strip_formatting(text));
    let mut paths = described.paths;
    let (icon, (verb, details)) = match event {
        Event::Custom { .. } if super::both::row_words(event).is_some() => {
            let (verb, details, path) = super::both::row_words(event).unwrap_or_default();
            paths = vec![path];
            (Icon::Overlap, (verb, details))
        }
        Event::Commit { .. } => (Icon::Commit, ("committed".to_owned(), described.headline)),
        // Several commits: its lead, which says how many, then their subjects, oldest first,
        // one a line, as its details (the user's decision, 11 Nov); one says its subject
        // inline, as zirc_proto::push_line does.
        Event::Push {
            branch,
            head,
            forced,
            subjects,
            ..
        } if subjects.len() > 1 => (
            Icon::Push,
            (
                zirc_proto::push_lead(branch, head, *forced, subjects.len()),
                subjects
                    .iter()
                    .map(|subject| strip_formatting(subject))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ),
        Event::Push { .. } => (Icon::Push, (described.headline, String::new())),
        Event::Working { .. } => (Icon::Working, (described.headline, String::new())),
        Event::Intent { .. } => (
            Icon::Intent,
            ("is about to change".to_owned(), described.headline),
        ),
        Event::Text { body } => match body.strip_prefix("taking:") {
            Some(task) => (
                Icon::Taking,
                ("is taking".to_owned(), strip_formatting(task.trim())),
            ),
            None => (Icon::Post, said(body)),
        },
        _ => (Icon::Post, said(&event.human_text())),
    };
    // A post addressed to a session says so apart from its text.
    let (to, details) = match (event, super::addressed(&details)) {
        (Event::Text { .. }, Some((session, rest))) => (Some(session.to_owned()), rest.to_owned()),
        _ => (None, details),
    };
    let (actor, session, owner) = match agent_of(nick, owner, message) {
        Some((owner, Some((session, name)))) => (name, Some(session), Some(owner)),
        Some((whose, None)) => (actor(nick, owner, owner_name), None, Some(whose)),
        None => (actor(nick, owner, owner_name), None, None),
    };
    let links = event_links(event);
    let actor_detail = owner
        .as_deref()
        .filter(|_| session.is_some())
        .and_then(|owner| super::agent_name(Some(&actor), &owner_name(owner)).1);
    Parts {
        icon,
        actor,
        session,
        actor_detail,
        owner,
        to,
        compact: false,
        verb,
        details,
        paths,
        links,
        time,
        until: time,
    }
}

impl<'a> Landing<'a> {
    /// Whether it's an open request to `me`, which a 👍 or a 👎 on it answers.
    #[must_use]
    pub fn asks(&self, me: &str) -> bool {
        self.asking
            .as_deref()
            .is_some_and(|by| fold(by) == fold(me))
    }

    /// What tells this version of its card apart from the next, so an app shows what
    /// pressing one of its buttons came to until the integration posts the card again:
    /// its sender, id, state, note and how many buttons it has. Empty with no card.
    #[must_use]
    pub fn card_version(&self) -> String {
        self.card
            .as_ref()
            .map(|card| {
                format!(
                    "{} {} {:?} {:?} {}",
                    fold(&self.entry.from),
                    card.id,
                    card.state,
                    card.note,
                    card.actions.len()
                )
            })
            .unwrap_or_default()
    }

    /// What it says without the session it names in front, which apps show dimmed at the
    /// end of the line instead ([`Landing::session`]).
    #[must_use]
    pub fn said(&self) -> &str {
        self.session
            .and_then(|session| self.text.strip_prefix(session))
            .and_then(|rest| rest.strip_prefix(": "))
            .unwrap_or(self.text)
    }

    /// What it says, as rich text ([`super::rich`]), for apps to draw. A landing's card is
    /// its line, `fix-login · build 771 · landed · ✓ production updated` (its state's mark is
    /// the row's circle), the build its
    /// page's link, and under it what CI's latest event says below its first line; anything
    /// else is what it said without the session, its addresses links.
    #[must_use]
    pub fn rich(&self) -> Vec<super::Block> {
        let links = self.links();
        if let Some(card) = &self.card {
            return card_blocks(card, self.card_line().unwrap_or_default());
        }
        // Apps draw the landing's state in the row's circle, so its words leave their mark
        // out: `fix-login · build 771 · landed · ✓ production updated`.
        let Some(line) = self.line(true) else {
            return super::link_blocks(super::rich(self.said()), &links);
        };
        let details = self.said().split_once('\n').map(|(_, rest)| rest.trim());
        std::iter::once(super::Block::Paragraph(line))
            .chain(super::link_blocks(
                details.into_iter().flat_map(super::rich).collect(),
                &links,
            ))
            .collect()
    }

    /// A landing's card's line: its branch, its build (a link to its page) and how it
    /// went, landing then deploy, each the latest said. `None` for a row of its own.
    #[must_use]
    pub fn card_line(&self) -> Option<Vec<super::Span>> {
        self.line(false)
    }

    /// [`Landing::card_line`], or with `circled` the landing's own state without its mark
    /// (`landed`, not `✓ landed`), for apps that draw that mark in the row's circle
    /// ([`Parts::icon`]); the deploy's keeps its own. The plain-text line, the terminal's
    /// and what's copied, keeps both.
    fn line(&self, circled: bool) -> Option<Vec<super::Span>> {
        if let Some(card) = &self.card {
            let mut line = vec![super::Span {
                text: card.title.clone(),
                bold: true,
                ..super::Span::default()
            }];
            if let Some(state) = state_label(card.state) {
                line.push(super::Span {
                    text: format!(" {state}"),
                    ..super::Span::default()
                });
            }
            return Some(line);
        }
        self.landing?;
        let text = |text: String| super::Span {
            text,
            ..super::Span::default()
        };
        let mut line = Vec::new();
        let part = |span: super::Span, line: &mut Vec<super::Span>| {
            if !line.is_empty() {
                line.push(text(" · ".to_owned()));
            }
            line.push(span);
        };
        if let Some(branch) = self.branch {
            part(text(branch.to_owned()), &mut line);
            line = super::link_words(line, &self.links());
        }
        if let Some(build) = self.build {
            let build = format!("build {build}");
            part(
                match self.page.clone() {
                    Some(url) => super::Span {
                        text: build,
                        kind: super::SpanKind::Link(url),
                        ..super::Span::default()
                    },
                    None => text(build),
                },
                &mut line,
            );
        }
        let landed = self.stages.iter().rev().find(|stage| stage.lands());
        let deployed = self.stages.iter().rev().find(|stage| !stage.lands());
        let circle = circled && self.outcome != Outcome::Other;
        for stage in landed.into_iter().chain(deployed) {
            let words = if circle && stage.lands() {
                stage.words()
            } else {
                stage.label()
            };
            part(text(words), &mut line);
        }
        Some(line)
    }

    /// Takes in a later event of the same landing: the card says what it says now.
    fn absorb(&mut self, later: Landing<'a>) {
        if later.stages.iter().any(Stage::lands) {
            self.outcome = later.outcome;
        }
        self.stages.extend(later.stages);
        self.text = later.text;
        self.commit = self.commit.or(later.commit);
        self.url = self.url.or(later.url);
        self.page = self.page.take().or(later.page);
        self.build = self.build.or(later.build);
        self.branch = self.branch.or(later.branch);
        self.repo = self.repo.or(later.repo);
        self.sha = self.sha.or(later.sha);
        if !later.failed.is_empty() {
            self.failed = later.failed;
        }
        // A card is the whole card every time.
        if later.card.is_some() {
            self.outcome = later.outcome;
            self.card = later.card;
            self.asking = later.asking;
            self.session = later.session.or(self.session);
        }
    }

    /// What its later events share, to fold into its card: a card's sender and id, as the
    /// same id from the same nick replaces a card; else a landing's id. `None` for a row of
    /// its own.
    fn key(&self) -> Option<String> {
        match &self.card {
            Some(card) => Some(format!("card {} {}", fold(&self.entry.from), card.id)),
            None => self.landing.map(str::to_owned),
        }
    }

    /// Its failed targets, each to its test results; `build N` to its build's page, when CI
    /// said it; and its branch and commit, each to its page on the repository's forge: as
    /// words its text says. The forge's only when CI said which repository.
    #[must_use]
    pub fn links(&self) -> Vec<(String, String)> {
        let mut links = self.target_links();
        if let (Some(build), Some(page)) = (self.build, &self.page) {
            links.push((format!("build {build}"), page.clone()));
        }
        let Some(repo) = self.repo else {
            return links;
        };
        if let Some(sha) = self.sha.or(self.commit) {
            let page = super::commit_link(repo, sha);
            links.push((super::describe::short(sha).to_owned(), page.clone()));
            if let Some(commit) = self.commit.filter(|commit| sha.starts_with(*commit)) {
                links.push((commit.to_owned(), page));
            }
        }
        if let Some(branch) = self.branch {
            links.push((branch.to_owned(), super::branch_link(repo, branch)));
        }
        links.dedup_by(|a, b| a.0 == b.0);
        links
    }

    /// Each failed target its line names, to its test results: as the line writes it, a
    /// long one cut short with `...` or `…` included.
    fn target_links(&self) -> Vec<(String, String)> {
        let mut links = Vec::new();
        for (target, results) in &self.failed {
            links.push(((*target).to_owned(), (*results).to_owned()));
            let cut = self.said().split_whitespace().find_map(|word| {
                let word = word.trim_end_matches([',', ':', ';']);
                let start = word
                    .strip_suffix("...")
                    .or_else(|| word.strip_suffix('…'))?;
                (start.len() > 2 && target.starts_with(start)).then_some(word)
            });
            if let Some(cut) = cut {
                links.push((cut.to_owned(), (*results).to_owned()));
            }
        }
        links
    }

    /// The post in parts: a landing's outcome's icon, the integration, and what it said
    /// without the session, which is [`Landing::session`].
    #[must_use]
    pub fn parts(&self) -> Parts {
        let icon = match self.outcome {
            Outcome::Landed => Icon::Landed,
            Outcome::Failed => Icon::Failed,
            Outcome::Conflict => Icon::Conflict,
            Outcome::Other => Icon::Post,
        };
        // A card whose deploy failed failed, however its landing went.
        let icon = if self.stages.contains(&Stage::DeployFailed)
            && self.stages.last() == Some(&Stage::DeployFailed)
        {
            Icon::Failed
        } else {
            icon
        };
        Parts {
            icon,
            actor: self.entry.from.clone(),
            session: None,
            owner: None,
            actor_detail: None,
            to: None,
            compact: false,
            verb: String::new(),
            details: self.said().to_owned(),
            paths: Vec::new(),
            links: self.links(),
            time: self.entry.time,
            until: self.entry.time,
        }
    }
}

/// A zirc message that can't be read: the words it carries.
fn unreadable_item<'a>(
    entry: &'a Entry,
    text: &str,
    owner_name: &dyn Fn(&str) -> String,
) -> Item<'a> {
    Item::Event {
        entry,
        text: format!("{}: {text}", entry.from),
        parts: Parts {
            icon: Icon::Post,
            actor: actor(&entry.from, entry.owner.as_deref(), owner_name),
            session: None,
            owner: None,
            actor_detail: None,
            to: None,
            compact: false,
            verb: String::new(),
            details: text.to_owned(),
            paths: Vec::new(),
            links: Vec::new(),
            time: entry.time,
            until: entry.time,
        },
        continues: false,
    }
}

/// The kind of zirc event CI says how a landing went with.
pub use zirc_proto::ci::LANDING_KIND;

/// How a landing went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It's on the default branch.
    Landed,
    /// Its checks failed.
    Failed,
    /// It conflicts with the default branch, or it kept moving.
    Conflict,
    /// Anything else CI says, such as a deploy waiting for approval.
    Other,
}

/// A step in a landing's way to production, as CI says each: one landing's events, from
/// its landing to its deploy, fold into one card that shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// It's on the default branch.
    Landed,
    /// Its checks failed: in this step, when CI says.
    Failed(Option<String>),
    /// It conflicts with the default branch, or it kept moving.
    Conflict,
    /// Its deploy waits for someone to approve it.
    Approval,
    /// It's deploying to production.
    Deploying,
    /// Production runs it.
    Deployed,
    /// Its deploy or promotion failed.
    DeployFailed,
}

impl Stage {
    /// The stage CI's words `said`, in an event of `kind`, tell: `None` for anything else.
    fn of(kind: &str, said: &str) -> Option<Self> {
        let first = said.lines().next().unwrap_or_default();
        if kind == LANDING_KIND {
            if first.starts_with("landed ") {
                return Some(Self::Landed);
            }
            if first.contains("conflicts with") || first.contains("kept moving") {
                return Some(Self::Conflict);
            }
            if first.contains(" failed") {
                let step = first
                    .split_once(" failed in ")
                    .map(|(_, rest)| rest.split([':', ' ']).next().unwrap_or(rest))
                    .filter(|step| !step.is_empty())
                    .map(str::to_owned);
                return Some(Self::Failed(step));
            }
            return None;
        }
        if first.contains("approve for production") {
            Some(Self::Approval)
        } else if first.contains("deploying to production") {
            Some(Self::Deploying)
        } else if first.contains("failed") {
            Some(Self::DeployFailed)
        } else if first.contains("production runs") || first.contains("deployed to production") {
            Some(Self::Deployed)
        } else {
            None
        }
    }

    /// What its card says of it: `✓ landed`, `✗ failed in lint`, `✓ production updated`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Landed => "✓ landed".to_owned(),
            Self::Failed(Some(step)) => format!("✗ failed in {step}"),
            Self::Failed(None) => "✗ failed".to_owned(),
            Self::Conflict => "⚠ conflicts".to_owned(),
            Self::Approval => "waiting for approval".to_owned(),
            Self::Deploying => "deploying".to_owned(),
            Self::Deployed => "✓ production updated".to_owned(),
            Self::DeployFailed => "✗ promote failed".to_owned(),
        }
    }

    /// [`Stage::label`] without its mark: `landed`, `failed in lint`, `conflicts`.
    #[must_use]
    pub fn words(&self) -> String {
        let label = self.label();
        match label.split_once(' ') {
            Some((mark, rest)) if ["✓", "✗", "⚠"].contains(&mark) => rest.to_owned(),
            _ => label,
        }
    }

    /// Whether it's how the landing went, rather than how its deploy did.
    fn lands(&self) -> bool {
        matches!(self, Self::Landed | Self::Failed(_) | Self::Conflict)
    }
}

/// A landing's row, from CI's `x-zirc/landing` event: one card for each landing, its
/// events folded into it as they come ([`Landing::stages`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing<'a> {
    /// What it came from.
    pub entry: &'a Entry,
    /// Its kind of event, such as `x-zirc/landing` or `x-zirc/deploy`.
    pub kind: &'a str,
    /// How it went: for a landing; [`Outcome::Other`] for anything else.
    pub outcome: Outcome,
    /// Whether it carries on a run of the same integration's posts, so its sender can be
    /// left out, as for any sender's messages.
    pub continues: bool,
    /// The session it was landed for, when CI named one.
    pub session: Option<&'a str>,
    /// The commit, short, when it says.
    pub commit: Option<&'a str>,
    /// The build's number.
    pub build: Option<&'a str>,
    /// The build's page.
    pub url: Option<&'a str>,
    /// What CI said, in its latest event about the landing.
    pub text: &'a str,
    /// The integration's short name, shown in place of a generic app badge: `ci`, or
    /// whatever it calls itself. See [`app_label`].
    pub app: Option<String>,
    /// Which landing it's of, `<pipeline>/<build>`, the same in every event of one
    /// landing's build, which fold into one card: `None` for an older or a hand deploy's,
    /// which stays a row of its own.
    pub landing: Option<&'a str>,
    /// The branch it landed.
    pub branch: Option<&'a str>,
    /// Its build's page: the data's, else the first address CI's words gave for a build,
    /// kept as later events, which may not say it, fold in.
    pub page: Option<String>,
    /// What each of its events said of it, in order.
    pub stages: Vec<Stage>,
    /// The repository's web address, which its branch and commit link into.
    pub repo: Option<&'a str>,
    /// The commit's full id, when CI's data gives it.
    pub sha: Option<&'a str>,
    /// The failed targets its line names, each with its test results' page, as CI's data
    /// gives them (`failed`): `//web:e2e` and its link.
    pub failed: Vec<(&'a str, &'a str)>,
    /// The card its event carries ([`zirc_proto::card`]), as its latest event has it: any
    /// sender's, which apps draw as a card with its buttons. Its title, fields, links and
    /// note are in [`Landing::rich`]; its actions are the app's to draw.
    pub card: Option<Card>,
    /// Whom an open request (`x-zirc/request`) asks: they answer it with a 👍 or a 👎 on it,
    /// which apps offer them as buttons ([`Landing::asks`]).
    pub asking: Option<String>,
}

/// An integration's post's row ([`SenderKind::Integration`]): CI saying how a landing or a
/// deploy went, zirc-alerts, and any `x-zirc/landing` event; and anyone's event carrying a
/// card.
fn landing<'a>(entry: &'a Entry, message: &'a Message, continues: bool) -> Option<Landing<'a>> {
    let Event::Custom { kind, data, text } = &message.event else {
        return None;
    };
    let card = Card::of(data);
    if card.is_none()
        && kind != LANDING_KIND
        && sender_kind(&entry.from, entry.owner.as_deref()) != SenderKind::Integration
    {
        return None;
    }
    let field = |key: &str| data.get(key).and_then(|value| value.as_str());
    // CI's words: "<session>: landed <branch> as <commit> (build N)", "<session>: build N
    // failed in …", "<session>: <branch> conflicts with main: …".
    let (session, said) = match zirc_proto::addressed(text) {
        Some((who, rest)) => (Some(who), rest),
        None => (None, text.as_str()),
    };
    // A card says whom it's about.
    let session = data
        .get(zirc_proto::card::KEY)
        .and_then(|card| card.get("about"))
        .and_then(|about| about.as_str())
        .filter(|_| card.is_some())
        .or(session);
    let outcome = if let Some(card) = &card {
        match card.state {
            State::Passed => Outcome::Landed,
            State::Failed => Outcome::Failed,
            State::Warning => Outcome::Conflict,
            State::Info | State::Pending | State::Running | State::Cancelled => Outcome::Other,
        }
    } else if kind != LANDING_KIND {
        Outcome::Other
    } else if said.starts_with("landed ") {
        Outcome::Landed
    } else if said.contains(" failed") {
        Outcome::Failed
    } else if said.contains("conflicts with") || said.contains("kept moving") {
        Outcome::Conflict
    } else {
        Outcome::Other
    };
    let commit = said
        .strip_prefix("landed ")
        .and_then(|rest| rest.split(" as ").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .or_else(|| field("commit"));
    Some(Landing {
        entry,
        kind,
        outcome,
        continues,
        session,
        commit,
        build: field("build"),
        url: field("url"),
        text,
        app: app_label(&entry.from, Some(message)),
        landing: field("landing"),
        branch: field("branch"),
        page: field("url").map(str::to_owned).or_else(|| build_page(text)),
        stages: Stage::of(kind, said).into_iter().collect(),
        repo: field("repo"),
        sha: field("commit"),
        failed: data
            .get("failed")
            .and_then(|failed| failed.as_array())
            .map(|failed| {
                failed
                    .iter()
                    .filter_map(|one| {
                        Some((one.get("target")?.as_str()?, one.get("results")?.as_str()?))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        card,
        asking: zirc_proto::request::Request::of(&message.event)
            .filter(|request| request.state == zirc_proto::request::State::Open)
            .map(|request| request.by),
    })
}

/// `landing`'s item: a card of its own, or `None` when it's a later event of a landing or
/// card whose card is in `items` already, which takes it in. `landings` says where each
/// card is in `items`, by [`Landing::key`].
fn card<'a>(
    items: &mut [Item<'a>],
    landings: &mut HashMap<String, usize>,
    landing: Landing<'a>,
) -> Option<Item<'a>> {
    let key = landing.key();
    if let Some(at) = key.as_ref().and_then(|key| landings.get(key).copied()) {
        if let Some(Item::Landing(card)) = items.get_mut(at) {
            card.absorb(landing);
        }
        return None;
    }
    if let Some(key) = key {
        landings.insert(key, items.len());
    }
    Some(Item::Landing(landing))
}

/// A card's state in words, after its title, `web · deploy 42 failed`: none for
/// [`State::Info`]. Its mark, `✗`, is the row's glyph alone, so the state shows once.
fn state_label(state: State) -> Option<&'static str> {
    Some(match state {
        State::Info => return None,
        State::Pending => "pending",
        State::Running => "running",
        State::Passed => "passed",
        State::Failed => "failed",
        State::Cancelled => "cancelled",
        State::Warning => "needs a look",
    })
}

/// A card as rich text: `line`, its title and state; each field, its label bold and its
/// value as Markdown; its links in a row; and its note, in italics.
fn card_blocks(card: &Card, line: Vec<super::Span>) -> Vec<super::Block> {
    let span = |text: String| super::Span {
        text,
        ..super::Span::default()
    };
    let mut blocks = vec![super::Block::Paragraph(line)];
    for field in &card.fields {
        let mut spans = vec![super::Span {
            text: format!("{}: ", field.label),
            bold: true,
            ..super::Span::default()
        }];
        for block in super::rich(&field.value) {
            if let super::Block::Paragraph(value) = block {
                spans.extend(value);
            }
        }
        blocks.push(super::Block::Paragraph(spans));
    }
    if !card.links.is_empty() {
        let mut spans = Vec::new();
        for link in &card.links {
            if !spans.is_empty() {
                spans.push(span(" · ".to_owned()));
            }
            spans.push(super::Span {
                text: link.label.clone(),
                kind: super::SpanKind::Link(link.url.clone()),
                ..super::Span::default()
            });
        }
        blocks.push(super::Block::Paragraph(spans));
    }
    if let Some(note) = &card.note {
        blocks.push(super::Block::Paragraph(vec![super::Span {
            text: note.clone(),
            italic: true,
            ..super::Span::default()
        }]));
    }
    blocks
}

/// The first address `text` gives for a build's page.
fn build_page(text: &str) -> Option<String> {
    super::rich(text)
        .into_iter()
        .flat_map(|block| match block {
            super::Block::Paragraph(spans) => spans,
            _ => Vec::new(),
        })
        .find_map(|span| match span.kind {
            super::SpanKind::Link(url) if url.contains("/builds/") => Some(url),
            _ => None,
        })
}

/// Whether `entry` is a system row, never part of a run of talk: people coming and going,
/// and what this client says, none of which breaks a run around it. zirc's
/// events are talk, in their sender's runs as messages are.
fn system(entry: &Entry) -> bool {
    matches!(
        entry.kind,
        EntryKind::Join
            | EntryKind::Part(_)
            | EntryKind::Quit(_)
            | EntryKind::Unreadable(_)
            | EntryKind::Support(_)
            | EntryKind::Info(_)
    )
}

/// A push of a commit an agent session just said it made, folded into that commit's row:
/// `committed and pushed`, one row rather than two. Looks back over the events just before
/// the push, from the same machine and session.
fn commits_pushed(items: Vec<Item<'_>>) -> Vec<Item<'_>> {
    let mut out: Vec<Item<'_>> = Vec::with_capacity(items.len());
    for item in items {
        if let Item::Event { entry, .. } = &item
            && let EntryKind::Zirc(message) = &entry.kind
            && let Event::Push { head, branch, .. } = &message.event
        {
            let same = |sha: &str| {
                !sha.is_empty() && (head.starts_with(sha) || sha.starts_with(head.as_str()))
            };
            let commit = out
                .iter_mut()
                .rev()
                .take_while(|item| matches!(item, Item::Event { .. }))
                .find_map(|earlier| {
                    let Item::Event {
                        entry: made, parts, ..
                    } = earlier
                    else {
                        return None;
                    };
                    let EntryKind::Zirc(said) = &made.kind else {
                        return None;
                    };
                    let Event::Commit { sha, .. } = &said.event else {
                        return None;
                    };
                    (made.from.eq_ignore_ascii_case(&entry.from)
                        && said.session == message.session
                        && same(sha))
                    .then_some(parts)
                });
            if let Some(parts) = commit {
                parts.verb = format!("committed and pushed to {branch}");
                parts.until = entry.time;
                continue;
            }
        }
        out.push(item);
    }
    out
}

/// Whether `entry` is an integration's post: an event from zirc's own accounts, which are
/// ordinary messages from an app.
fn integration_post(entry: &Entry) -> bool {
    matches!(entry.kind, EntryKind::Zirc(_))
        && sender_kind(&entry.from, entry.owner.as_deref()) == SenderKind::Integration
}

/// Whether `next` carries on `previous`'s run: one sender's talk, or one integration's
/// posts, within [`super::describe::RUN_GAP`].
fn carries_on(previous: &Entry, next: &Entry) -> bool {
    if integration_post(previous) || integration_post(next) {
        return integration_post(previous)
            && integration_post(next)
            && previous.from.eq_ignore_ascii_case(&next.from)
            && next.time - previous.time < super::describe::RUN_GAP;
    }
    let (Some(said), Some(says)) = (talker(previous), talker(next)) else {
        return false;
    };
    // Another agent session on the same machine is another speaker.
    said.eq_ignore_ascii_case(says)
        && session_of(previous) == session_of(next)
        && next.time - previous.time < super::describe::RUN_GAP
}

/// Who's talking in `entry`, when it's drawn as talk: a message, notice or action, or any
/// zirc message, an event as much as a file or a post.
fn talker(entry: &Entry) -> Option<&str> {
    let talk = matches!(
        entry.kind,
        EntryKind::Message(_) | EntryKind::Notice(_) | EntryKind::Action(_) | EntryKind::Zirc(_)
    );
    talk.then_some(entry.from.as_str())
}

/// Joins, parts and quits in a row, as one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    /// When the first of them happened.
    pub time: DateTime<Utc>,
    /// Who joined, in order, each once.
    pub joined: Vec<String>,
    /// Who left the channel or the server, in order, each once.
    pub left: Vec<String>,
}

impl Membership {
    /// The row in parts: who came or went, as [`Membership::text`] says it.
    #[must_use]
    pub fn parts(&self) -> Parts {
        let icon = if self.joined.is_empty() {
            Icon::Leave
        } else {
            Icon::Join
        };
        Parts {
            icon,
            actor: String::new(),
            session: None,
            owner: None,
            actor_detail: None,
            to: None,
            compact: true,
            verb: self.text(),
            details: String::new(),
            paths: Vec::new(),
            links: Vec::new(),
            time: self.time,
            until: self.time,
        }
    }

    /// The line: `alice joined`, `3 joined, 2 left`.
    #[must_use]
    pub fn text(&self) -> String {
        let say = |nicks: &[String], did: &str| match nicks {
            [] => None,
            [one] => Some(format!("{one} {did}")),
            many => Some(format!("{} {did}", many.len())),
        };
        [say(&self.joined, "joined"), say(&self.left, "left")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl super::Chat {
    /// `conversation`'s lines, oldest first, with the day of each time as `day` reckons it:
    /// in the view's own time zone. Agents' owners are named as [`super::Chat::owner_name`]
    /// says, as every row's head names them.
    #[must_use]
    pub fn items<'a>(
        &self,
        conversation: &'a Conversation,
        day: impl Fn(DateTime<Utc>) -> NaiveDate,
    ) -> Vec<Item<'a>> {
        items(conversation, day, &|owner| self.owner_name(owner))
    }
}

/// A message's row, saying where it was sent from when that's news: `apps` keeps each
/// sender's last message's app, by folded nick.
fn message_item<'a>(
    entry: &'a Entry,
    text: &'a str,
    continues: bool,
    apps: &mut HashMap<String, Option<&'a str>>,
) -> Item<'a> {
    let sent_from = entry.sent_from.as_deref();
    let before = apps.insert(fold(&entry.from), sent_from).flatten();
    Item::Message {
        entry,
        text,
        continues,
        sent_from,
        show_sent_from: shows_sent_from(sent_from, continues, before),
    }
}

/// Whether `message` starts a thread on a message that's among `entries`: then it's the line
/// under that message ([`super::Chat::thread_links`]), not a row of its own.
fn starts_thread_here(entries: &[Entry], message: &zirc_proto::Message) -> bool {
    zirc_proto::thread::Started::of(&message.event).is_some_and(|thread| {
        entries
            .iter()
            .any(|other| other.id.as_deref() == Some(thread.of.as_str()))
    })
}

/// Someone coming or going, `entry`: added to the run of them that `items` ends with,
/// unless it's a `new_day`, or else a run of its own to add.
fn membership<'a>(items: &mut [Item<'a>], entry: &Entry, new_day: bool) -> Option<Item<'a>> {
    let joined = entry.kind == EntryKind::Join;
    if !new_day && let Some(Item::Membership(run)) = items.last_mut() {
        add(run, &entry.from, joined);
        return None;
    }
    let mut run = Membership {
        time: entry.time,
        joined: Vec::new(),
        left: Vec::new(),
    };
    add(&mut run, &entry.from, joined);
    Some(Item::Membership(run))
}

/// `conversation`'s lines, as [`super::Chat::items`] says, its agents' owners named as
/// `owner_name` says.
pub(crate) fn items<'a>(
    conversation: &'a Conversation,
    day: impl Fn(DateTime<Utc>) -> NaiveDate,
    owner_name: &dyn Fn(&str) -> String,
) -> Vec<Item<'a>> {
    let entries = &conversation.entries;
    let hidden = reconnects(entries);
    let mut items = Vec::new();
    let mut previous: Option<&Entry> = None;
    // The last talk that isn't a system row, which a run carries on from.
    let mut talk: Option<&Entry> = None;
    // Each sender's last message's app, by folded nick.
    let mut apps: HashMap<String, Option<&str>> = HashMap::new();
    let mut landings: HashMap<String, usize> = HashMap::new();
    for (at, entry) in entries.iter().enumerate() {
        if hidden.contains(&at) || entry.is_session_card() || !conversation.shows(entry) {
            continue;
        }
        let date = day(entry.time);
        let new_day = previous.is_none_or(|p| day(p.time) != date);
        if new_day {
            items.push(Item::Day(date));
            talk = None;
        }
        let system_row = system(entry);
        let continues = !system_row && talk.is_some_and(|p| carries_on(p, entry));
        previous = Some(entry);
        if !system_row {
            talk = Some(entry);
        }
        let from = &entry.from;
        let with_reason = |text: String, reason: &Option<String>| match reason {
            Some(reason) => format!("{text} ({reason})"),
            None => text,
        };
        let item = match &entry.kind {
            EntryKind::Message(text) => message_item(entry, text, continues, &mut apps),
            EntryKind::Notice(text) => Item::Notice {
                entry,
                text,
                continues,
            },
            EntryKind::Action(text) => Item::Action {
                entry,
                text,
                continues,
            },
            // A thread's start is the line under its message ([`super::Chat::thread_links`]),
            // not a row of its own, once that message is here.
            EntryKind::Zirc(message) if starts_thread_here(entries, message) => continue,
            EntryKind::Zirc(message) => match landing(entry, message, continues) {
                Some(landing) => match card(&mut items, &mut landings, landing) {
                    Some(item) => item,
                    None => continue,
                },
                None => zirc_item(entry, message, continues, owner_name),
            },
            EntryKind::Unreadable(text) => unreadable_item(entry, text, owner_name),
            EntryKind::Join | EntryKind::Part(_) | EntryKind::Quit(_) => {
                match membership(&mut items, entry, new_day) {
                    Some(run) => run,
                    None => continue,
                }
            }
            EntryKind::Kick { nick, reason } => Item::Note {
                entry,
                text: with_reason(format!("{from} removed {nick}"), reason),
            },
            EntryKind::Nick(new) => Item::Note {
                entry,
                text: format!("{from} is now {new}"),
            },
            EntryKind::Topic(topic) => Item::Note {
                entry,
                text: format!("{from} set the topic: {topic}"),
            },
            EntryKind::Info(text) => Item::Note {
                entry,
                text: text.clone(),
            },
            EntryKind::Support(event) => Item::Note {
                entry,
                text: support_note(from, event, &conversation.name),
            },
        };
        items.push(item);
    }
    commits_pushed(items)
}

/// `items` with only what people said, as a view's "People only" filter shows them:
/// people's messages, and anything else that mentions `me`. Agents' posts and updates,
/// apps' posts and landings, and comings and goings go; a day stays only before
/// something that does.
#[must_use]
pub fn people_only<'a>(items: Vec<Item<'a>>, me: &str) -> Vec<Item<'a>> {
    let mentions_me = |text: &str| crate::names::mentions(text, me);
    let by_person = |entry: &Entry| {
        !matches!(
            sender_kind(&entry.from, entry.owner.as_deref()),
            SenderKind::Agent | SenderKind::Integration
        )
    };
    let mut kept: Vec<Item<'a>> = Vec::with_capacity(items.len());
    for item in items {
        let keep = match &item {
            Item::Day(_) | Item::Note { .. } => true,
            Item::Message { entry, text, .. }
            | Item::Notice { entry, text, .. }
            | Item::Action { entry, text, .. } => by_person(entry) || mentions_me(text),
            Item::File { entry, .. } => by_person(entry),
            Item::Event { entry, .. } => match &entry.kind {
                EntryKind::Zirc(message) => mentions_me(&message.event.human_text()),
                _ => false,
            },
            Item::Membership(_) | Item::Landing(_) => false,
        };
        if !keep {
            continue;
        }
        if matches!(item, Item::Day(_)) && matches!(kept.last(), Some(Item::Day(_))) {
            kept.pop();
        }
        kept.push(item);
    }
    if matches!(kept.last(), Some(Item::Day(_))) {
        kept.pop();
    }
    kept
}

/// A support event's system row, in the conversation called `name`: a guest named as
/// [`super::guest_label`] does, never by their nick.
fn support_note(from: &str, event: &zirc_proto::support::Support, name: &str) -> String {
    let named = |nick: &str| {
        if nick.to_ascii_lowercase().starts_with(super::GUEST_PREFIX) {
            super::guest_label(nick)
        } else {
            nick.to_owned()
        }
    };
    super::support_text(&named(from), event, &named(name))
}

/// The session a zirc message is from, if it's one.
fn session_of(entry: &Entry) -> Option<&str> {
    match &entry.kind {
        EntryKind::Zirc(message) => Some(message.session.as_str()),
        _ => None,
    }
}

/// Whether a message shows the app it was sent from, `sent_from`: on the first message of a
/// run, and where the sender's app changed since their previous message, `before`. Never
/// when no app said, as from plain IRC clients.
#[must_use]
pub fn shows_sent_from(sent_from: Option<&str>, continues: bool, before: Option<&str>) -> bool {
    sent_from.is_some() && (!continues || sent_from != before)
}

/// A day as a divider says it: `Today`, `Yesterday`, or `Thursday 29 October 2026`.
#[must_use]
pub fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        "Today".into()
    } else if today.pred_opt() == Some(date) {
        "Yesterday".into()
    } else {
        date.format("%A %-d %B %Y").to_string()
    }
}

fn add(run: &mut Membership, nick: &str, joined: bool) {
    let nicks = if joined {
        &mut run.joined
    } else {
        &mut run.left
    };
    if !nicks.iter().any(|n| n.eq_ignore_ascii_case(nick)) {
        nicks.push(nick.to_owned());
    }
}

/// Where the entries are that only say a nick reconnected: a quit or part, and the same
/// nick's join within [`RECONNECT_GAP`].
fn reconnects(entries: &[Entry]) -> HashSet<usize> {
    let mut hidden = HashSet::new();
    for (at, entry) in entries.iter().enumerate() {
        if entry.kind != EntryKind::Join {
            continue;
        }
        let nick = fold(&entry.from);
        let left = entries[..at]
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, e)| entry.time - e.time <= RECONNECT_GAP)
            .find(|(i, e)| {
                matches!(e.kind, EntryKind::Part(_) | EntryKind::Quit(_))
                    && fold(&e.from) == nick
                    && !hidden.contains(i)
            })
            .map(|(i, _)| i);
        if let Some(i) = left {
            hidden.insert(i);
            hidden.insert(at);
        }
    }
    hidden
}
