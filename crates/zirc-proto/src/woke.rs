//! What agents do, said where people see it (the user's decisions, 23 and 24 Nov 2026):
//! each wake of a session's watch as one line ([`Woke`], [`KIND`]: `main-fix woke: build 612
//! failed on main`), and each overlap two sessions settled as one line in the agents' room
//! ([`Settled`], [`SETTLED_KIND`]), which the session that agreed says with `zirc settle`.
//! zircd says a wake for the session it woke, at info, so history keeps it, and where
//! [`Woke::chat`] says (the user's choices, 26 Nov 2026): a wake a chat's message caused, in
//! that chat alone, where apps draw it in among what's said; any other, in the room the
//! session's posts go to, where apps show it only in the session's own conversation. Either
//! way it's nothing unread, nothing pushed and wakes no one.
//!
//! Every word of both lines is decided here, and which of [`Tone`]'s colours a wake's edge
//! has; who is named, as people read them, is the apps' to say ([`Woke::words`]'s `who`).
//! A wake says what happened, never what anyone said: an app adds the words of the line
//! that woke it ([`Woke::line`]) only where its reader can read that line anyway.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Event;
use crate::card::State;
use crate::state::Level;

/// The kind of a wake's event.
pub const KIND: &str = "x-zirc/woke";

/// The kind of a settled overlap's event.
pub const SETTLED_KIND: &str = "x-zirc/overlap-settled";

/// What a wake's line says the agent did, after its name and before why and what
/// ([`Woke::heading`]): `main-fix woke: …` (the user's choice, 25 Nov 2026: who, why, then
/// what).
pub const WOKE: &str = "woke";

/// What a settled overlap's line is headed by.
pub const SETTLED_LABEL: &str = "Overlap settled";

/// Whether `event` is a wake's line: what a room keeps to Everything, never counts unread
/// and never pushes.
#[must_use]
pub fn is_woke(event: &Event) -> bool {
    matches!(event, Event::Custom { kind, .. } if kind == KIND)
}

/// Whether `kind` is one of these: neither wakes any session's watch.
#[must_use]
pub fn is_kind(kind: &str) -> bool {
    kind == KIND || kind == SETTLED_KIND
}

/// Whose doing a wake was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum By {
    /// CI, or how something in the repository stands.
    Ci,
    /// Another agent session.
    Agent,
    /// A person.
    Person,
    /// zirc itself, or one a newer zircd says that this reader doesn't know.
    #[serde(other)]
    Zirc,
}

/// What happened, that woke the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum What {
    /// CI said how a build, a landing or a deploy went ([`Woke::build`], [`Woke::state`]).
    Build,
    /// How something the team shares stands changed, such as the default branch broken.
    State,
    /// Someone wrote to it directly.
    Wrote,
    /// Someone named it in a channel.
    Mentioned,
    /// Someone replied to it in a channel.
    Replied,
    /// Another agent asked it something.
    Asked,
    /// Another agent answered what it asked.
    Answered,
    /// Another agent sent it a note.
    Noted,
    /// Someone reacted to what it said.
    Reacted,
    /// Its person answered its request.
    Decided,
    /// Another session is about to change its files.
    Intends,
    /// Another session is changing the same files.
    Overlap,
    /// Another's push changes the same lines as its work.
    Conflict,
    /// Changes to its files landed, or went onto its branch.
    Landed,
    /// Another session built on its branch.
    StackedOn,
    /// Another session committed to files it changes.
    Committed,
    /// A post in a topic it joined, or that names its files.
    Posted,
    /// Something was reported to the role it holds ([`crate::role_report`]), or an item of
    /// it is still open ([`Woke::reminder`]).
    Reported,
    /// A tool zirc doesn't know posted to the role's address ([`crate::inbound`]): what it
    /// sent arrives as it was sent ([`Woke::excerpt`] begins it).
    Webhook,
    /// One a newer zircd says that this reader doesn't know.
    #[serde(other)]
    Other,
}

/// The colour of a wake line's edge, which each app's theme has a role for: the outcome
/// when CI said one, else whose doing it was (the user's choice, 24 Nov 2026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    /// Red: a build failed, or the default branch broke.
    Failed,
    /// Green: a build passed, a branch landed, the default branch is fine again.
    Passed,
    /// Amber: a person wrote, replied, reacted or answered.
    Person,
    /// Purple: another agent asked, answered, or is changing the same files.
    Agent,
    /// Grey: anything else.
    Quiet,
}

impl Tone {
    /// Every tone.
    pub const ALL: [Self; 5] = [
        Self::Failed,
        Self::Passed,
        Self::Person,
        Self::Agent,
        Self::Quiet,
    ];

    /// The theme role its edge is drawn in, as `zirc-theme` names them.
    #[must_use]
    pub const fn role(self) -> &'static str {
        match self {
            Self::Failed => "error",
            Self::Passed => "ok",
            Self::Person => "warning",
            Self::Agent => "agent",
            Self::Quiet => "muted",
        }
    }
}

/// The chat whose message woke a session ([`Woke::chat`]), where its line is said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chat<'a> {
    /// A channel, as it's called on the server: `#general`.
    Channel(&'a str),
    /// The direct messages with someone, by their nick: the person who wrote.
    Direct(&'a str),
}

/// A session's watch woke: what an [`Event::Custom`] of [`KIND`] carries, from the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Woke {
    /// What happened.
    pub what: What,
    /// Whose doing it was.
    pub by: By,
    /// Who: the nick it came from, a person's or a machine's. `None` for CI and zirc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The agent session on [`Self::from`] it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The channel the line that woke it was said in, as it's called on the server; `None`
    /// for what was sent to it directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// That line's id on the server, for an app to add its words where its reader has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// For CI's line: the build's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// For CI's line, how it stands ([`crate::card::State`], as [`crate::ci::Data::state`]
    /// says it).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::lenient::known::deserialize"
    )]
    pub state: Option<State>,
    /// For how something the team shares stands, its level.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::lenient::known::deserialize"
    )]
    pub level: Option<Level>,
    /// The branch it's about: the one CI built or that landed, or the state's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// For CI's line, whether it's a landing's ([`crate::ci::LANDING_KIND`]): one that
    /// passed landed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub landing: bool,
    /// The wake's id ([`crate::activity::Wake::id`]), which joins the line to its record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<String>,
    /// The role ([`crate::roles`]) the session was woken as, when it was: its line ends
    /// with it, `main-fix woke as the fixer: master is broken`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// For a report: the item it opened ([`crate::role_report::OpenItem::id`]) and what was
    /// reported, which its line says, as CI's build is. One of a kind this reader doesn't
    /// know is none, and the wake still wakes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::lenient::known::deserialize"
    )]
    pub report: Option<Box<Reported>>,
    /// For a landing that closed items of the role it was woken as (`Fixes: report <id>`):
    /// how many, which its line says after the landing, `· the item is closed`.
    #[serde(default, skip_serializing_if = "crate::is_zero")]
    pub closed: u32,
    /// For a post from a tool zirc doesn't know, how what it sent begins
    /// ([`EXCERPT_MAX`] characters), which its line says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

/// The most characters of an unknown tool's post a wake's line says.
pub const EXCERPT_MAX: usize = 120;

/// What a report's wake says of it ([`What::Reported`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Reported {
    /// The item's id.
    pub id: String,
    /// What was reported.
    pub report: crate::role_report::RoleReport,
    /// Whether it's a reminder of an item still open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reminder: bool,
    /// How many times it's been reported so far, for a tool's: `37 events`.
    #[serde(default, skip_serializing_if = "crate::is_zero")]
    pub events: u32,
}

impl Woke {
    /// A wake by `what`, `by` someone, with nothing more said.
    #[must_use]
    pub fn new(what: What, by: By) -> Self {
        Self {
            what,
            by,
            from: None,
            session: None,
            channel: None,
            line: None,
            build: None,
            state: None,
            level: None,
            branch: None,
            landing: false,
            wake: None,
            role: None,
            report: None,
            excerpt: None,
            closed: 0,
        }
    }

    /// A wake for `role`, whose `closed` items a landing of `branch` in `build` closed, as it
    /// passed: `CI landed ops-watch · build 1502 · the item is closed`.
    #[must_use]
    pub fn closed_by_landing(
        role: &str,
        build: Option<String>,
        branch: Option<String>,
        closed: u32,
    ) -> Self {
        Self {
            role: Some(role.to_owned()),
            build,
            branch,
            state: Some(State::Passed),
            landing: true,
            closed,
            ..Self::new(What::Build, By::Ci)
        }
    }

    /// A wake for what a tool zirc doesn't know posted to a role's address, `text` as it
    /// was sent.
    #[must_use]
    pub fn webhook(text: &str) -> Self {
        Self {
            excerpt: Some(crate::clip_line(text, EXCERPT_MAX)),
            ..Self::new(What::Webhook, By::Zirc)
        }
    }

    /// A wake for an item reported to `role`: new, or a reminder of one still open.
    #[must_use]
    pub fn reported(item: &crate::role_report::OpenItem, reminder: bool) -> Self {
        Self {
            role: Some(item.report.role.clone()),
            build: item.report.build.clone(),
            report: Some(Box::new(Reported {
                id: item.id.clone(),
                report: item.report.clone(),
                reminder,
                events: item.reports,
            })),
            ..Self::new(What::Reported, By::Zirc)
        }
    }

    /// The wake `event` says, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// The chat whose message woke it, which its line is said in and drawn in, alone (the
    /// user's choices, 26 Nov 2026): the channel a line that named it, replied to it or
    /// posted where it listens was said in, or a reaction to what it said was given in; a
    /// person's direct message, to it or to the role it holds, the chat with that person.
    /// `None` for what no chat caused, said in the room its posts go to: CI, wherever it
    /// spoke, and zirc itself, how something
    /// stands, a report, an overlap, another session's intent, commit or landing, a request
    /// answered, and what another agent sent it directly, a chat no person reads.
    #[must_use]
    pub fn chat(&self) -> Option<Chat<'_>> {
        // CI's word, or zirc's, is no chat's, wherever it was said.
        if matches!(self.by, By::Ci | By::Zirc) {
            return None;
        }
        match self.what {
            What::Mentioned | What::Replied | What::Posted | What::Reacted => {
                match self.channel.as_deref() {
                    Some(channel) if crate::names::is_channel(channel) => {
                        Some(Chat::Channel(channel))
                    }
                    _ => self.person_wrote(),
                }
            }
            What::Wrote => self.person_wrote(),
            _ => None,
        }
    }

    /// The chat with the person it came from directly, when a person it was.
    fn person_wrote(&self) -> Option<Chat<'_>> {
        (self.by == By::Person)
            .then_some(self.from.as_deref())
            .flatten()
            .map(Chat::Direct)
    }

    /// Whether its line, said in the conversation called `name`, is said in its own chat
    /// ([`Woke::chat`]), so drawn there in among what's said: a channel's, said in that
    /// channel, or one said to us directly. Else it's the session's, shown only in its own
    /// conversation, as an older zircd says every wake.
    #[must_use]
    pub fn in_its_chat(&self, name: &str) -> bool {
        match self.chat() {
            Some(Chat::Channel(channel)) => channel.eq_ignore_ascii_case(name),
            Some(Chat::Direct(_)) => !crate::names::is_channel(name),
            None => false,
        }
    }

    /// Its event, whose text plain IRC clients show: its words with no one named, so it
    /// names no session, branch or path for another's watch to wake on.
    ///
    /// # Panics
    /// Never: a wake always serialises.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a wake serialises"),
            text: {
                let why = self
                    .role
                    .as_ref()
                    .map(|role| format!(" as the {role}"))
                    .unwrap_or_default();
                let what = Self {
                    branch: None,
                    ..self.clone()
                }
                .words(None);
                format!("{WOKE}{why}: {what}")
            },
        }
    }

    /// The colour of its edge.
    #[must_use]
    pub fn tone(&self) -> Tone {
        // An unknown tool's post is amber, as a person's words are (mock `PhoneOnCall`); a
        // report's, its level's: red for an error, as for a failed build, amber for a warning.
        if self.what == What::Webhook {
            return Tone::Person;
        }
        if let Some(reported) = &self.report {
            return match reported.report.kind.level() {
                crate::Level::Error => Tone::Failed,
                crate::Level::Warn => Tone::Person,
                crate::Level::Info | crate::Level::Debug => Tone::Quiet,
            };
        }
        match (self.by, self.state, self.level) {
            (By::Ci, Some(State::Failed), _) | (By::Ci, _, Some(Level::Broken)) => Tone::Failed,
            (By::Ci, Some(State::Passed), _) | (By::Ci, _, Some(Level::Ok)) => Tone::Passed,
            (By::Person, ..) => Tone::Person,
            (By::Agent, ..) => Tone::Agent,
            _ => Tone::Quiet,
        }
    }

    /// What its line starts with, set in bold: the agent that woke, `agent` as the reader
    /// knows it, and why when a role was why: `main-fix woke:`, `ops-watch woke as the
    /// oncall:` (the user's choice, 25 Nov 2026: who, why, then what).
    #[must_use]
    pub fn heading(&self, agent: &str) -> String {
        match &self.role {
            Some(role) => format!("{agent} {WOKE} as the {role}:"),
            None => format!("{agent} {WOKE}:"),
        }
    }

    /// What its line says after its [`Woke::heading`]: what happened, and who did it,
    /// `who` being whoever did it as people read them (`Rajesh Iyer`, `replay-index`),
    /// which the reader's app knows: `CI landed its branch main-fix · build 613`, `Rajesh
    /// Iyer replied in #agents`. With no `who`, whose doing it was in a word: `a person`,
    /// `another agent`. Never a report's id, which only the holder's own watch shows.
    #[must_use]
    pub fn words(&self, who: Option<&str>) -> String {
        let who = who.unwrap_or(match self.by {
            By::Ci => "CI",
            By::Agent => "another agent",
            By::Person => "a person",
            By::Zirc => crate::NAME,
        });
        let place = self
            .channel
            .as_deref()
            .map(|channel| format!(" in {channel}"))
            .unwrap_or_default();
        let branch = self.branch.as_deref();
        // CI's or zirc's own word that isn't how a build went or how something stands.
        if matches!(self.by, By::Ci | By::Zirc)
            && !matches!(
                self.what,
                What::Build | What::State | What::Landed | What::Reported | What::Webhook
            )
        {
            return format!("{who} has news for it");
        }
        match self.what {
            What::Build => match self.closed {
                0 => self.build_words(),
                1 => format!("{} · the item is closed", self.build_words()),
                n => format!("{} · {n} items are closed", self.build_words()),
            },
            What::State => {
                let name = branch.unwrap_or("the default branch");
                match self.level {
                    Some(Level::Broken) => format!("{name} is broken"),
                    Some(Level::Ok) => format!("{name} is fine again"),
                    Some(Level::Warning) => format!("{name} needs a look"),
                    None => format!("{name} changed"),
                }
            }
            What::Wrote => format!("{who} wrote to it"),
            What::Mentioned => format!("{who} named it{place}"),
            What::Replied => format!("{who} replied{place}"),
            What::Asked => format!("{who} asked it"),
            What::Answered => format!("{who} answered"),
            What::Noted => format!("{who} sent it a note"),
            What::Reacted => format!("{who} reacted to what it said"),
            What::Decided => format!("{who} answered its request"),
            What::Intends => format!("{who} is about to change its files"),
            What::Overlap => format!("{who} is changing the same files"),
            What::Conflict => format!("{who} pushed changes to the same lines"),
            What::Landed => match branch {
                Some(branch) => format!("{branch} landed changes to its files"),
                None => "changes to its files landed".to_owned(),
            },
            What::StackedOn => format!("{who} built on its branch"),
            What::Committed => format!("{who} committed to its files"),
            What::Posted => format!("{who} posted{place}"),
            What::Reported => match self.report.as_deref() {
                Some(Reported {
                    report,
                    reminder,
                    events,
                    ..
                }) => match report.tool {
                    Some(_) => crate::inbound::woke_words(report, *events, *reminder),
                    None => report.found(*reminder),
                },
                None => "something was reported".to_owned(),
            },
            What::Webhook => crate::inbound::webhook_words(self.excerpt.as_deref().unwrap_or("")),
            What::Other => "something concerned it".to_owned(),
        }
    }

    /// CI's line in words: `build 612 failed on main`, `CI landed main-fix · build 613`,
    /// `build 614 passed on main-fix`.
    fn build_words(&self) -> String {
        let build = self
            .build
            .as_deref()
            .map_or_else(|| "a build".to_owned(), |build| format!("build {build}"));
        let on = self
            .branch
            .as_deref()
            .map(|branch| format!(" on {branch}"))
            .unwrap_or_default();
        match (self.state, self.branch.as_deref()) {
            (Some(State::Passed), Some(branch)) if self.landing => {
                format!("CI landed its branch {branch} · {build}")
            }
            (Some(State::Passed), None) if self.landing => format!("CI landed {build}"),
            (Some(State::Passed), _) => format!("{build} passed{on}"),
            (Some(State::Failed), _) => format!("{build} failed{on}"),
            (Some(State::Cancelled), _) => format!("{build} was cancelled{on}"),
            (Some(State::Warning), _) => format!("{build}{on} needs a look"),
            (Some(State::Pending), _) => format!("{build}{on} is waiting"),
            (Some(State::Running), _) => format!("{build}{on} is running"),
            (Some(State::Info) | None, _) => format!("CI has news of {build}{on}"),
        }
    }
}

/// One side of a settled overlap: an agent session and the machine it runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    /// Its machine's nick.
    pub machine: String,
    /// The session's id.
    pub session: String,
    /// What it's called ([`crate::Message::name`]), when its machine says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Side {
    /// It as its event's text names it, for plain clients: its name, else its id.
    fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.session)
    }

    /// Whether it's `session` on `machine`.
    #[must_use]
    pub fn is(&self, machine: &str, session: &str) -> bool {
        self.machine.eq_ignore_ascii_case(machine) && self.session == session
    }
}

/// What two sessions agreed over files both change (the user's choices, 25 Nov 2026: theirs
/// to agree without asking; one taking the other's whole task over is their users').
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Agreed {
    /// Who lands first: the second updates onto the first.
    #[default]
    Order,
    /// The second leaves the files to the first, and takes its own changes out of them.
    Leaves,
    /// The second builds on the first's branch, and lands after it.
    BuildsOn,
}

impl Agreed {
    /// Whether it's only an order, as every settle was before there were others: left out of
    /// what's sent.
    #[must_use]
    pub fn is_order(&self) -> bool {
        *self == Self::Order
    }
}

/// Two sessions settled files both change: what an [`Event::Custom`] of [`SETTLED_KIND`]
/// carries, from the session that says so (`zirc settle`). Whatever they agreed
/// ([`Agreed`]), the first is the one whose changes land first and the second the one that
/// follows, so a reader from before there was more than an order reads it rightly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settled {
    /// The session whose changes land first: the one the files are left to, or whose branch
    /// is built on.
    pub first: Side,
    /// The one that updates onto them: the one that leaves the files, or builds on the
    /// first's branch.
    pub second: Side,
    /// The files both change, from the repository's root.
    pub paths: Vec<String>,
    /// Whether they change the same lines of them, so the second resolves a conflict;
    /// else different parts, which merge by themselves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub same_lines: bool,
    /// What they agreed, when it's more than the order.
    #[serde(default, skip_serializing_if = "Agreed::is_order")]
    pub agreed: Agreed,
}

/// How many of a settled overlap's files its line names before `and N more`: few, as they're
/// part of one sentence. An event's paths listed one a line fold at zirc-chat's
/// `SHOWN_PATHS` instead, with words of their own; a change to how either says "more" is one
/// to make at both.
pub const SETTLED_PATHS: usize = 3;

impl Settled {
    /// The settled overlap `event` says, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == SETTLED_KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }

    /// Its event, whose text says it for plain clients and `zirc watch`.
    ///
    /// # Panics
    /// Never: it always serialises.
    #[must_use]
    pub fn event(&self) -> Event {
        let (first, second, files) = (self.first.label(), self.second.label(), self.files());
        // Short, as an event's words go in its line twice: once for plain clients.
        let text = match self.agreed {
            Agreed::Order => format!(
                "overlap settled: {first} lands first, {second} rebases onto it: {files} ({})",
                if self.same_lines {
                    "same lines"
                } else {
                    "different lines, no conflict"
                }
            ),
            Agreed::Leaves => format!("overlap settled: {second} leaves {files} to {first}"),
            Agreed::BuildsOn => {
                format!("overlap settled: {second} builds on {first}'s branch: {files}")
            }
        };
        Event::Custom {
            kind: SETTLED_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a settled overlap serialises"),
            text,
        }
    }

    /// Whether it's between `a` and `b`, each a `(machine, session)`, either way round.
    #[must_use]
    pub fn between(&self, a: (&str, &str), b: (&str, &str)) -> bool {
        (self.first.is(a.0, a.1) && self.second.is(b.0, b.1))
            || (self.first.is(b.0, b.1) && self.second.is(a.0, a.1))
    }

    /// The files it names, `and N more` after the first [`SETTLED_PATHS`]: each a part of
    /// its own, for an app to set the paths in monospace.
    #[must_use]
    pub fn listed(&self) -> (Vec<&str>, Option<String>) {
        let shown: Vec<&str> = self
            .paths
            .iter()
            .take(SETTLED_PATHS)
            .map(String::as_str)
            .collect();
        let more = self.paths.len().saturating_sub(shown.len());
        (shown, (more > 0).then(|| format!("{more} more")))
    }

    /// Its line in parts, `first` and `second` being the two sessions as people read them:
    /// `tls-rotation and replay-metrics both change `, the files, `. Agreed: tls-rotation
    /// lands first; replay-metrics rebases onto it. `, and `Different lines, no conflict.`
    /// The second part is [`Self::files`]; apps set the names in bold and the files in
    /// monospace, and read no other meaning into where the parts are cut.
    #[must_use]
    pub fn sentences(&self, first: &str, second: &str) -> [String; 4] {
        let (agreed, then) = match self.agreed {
            Agreed::Order => (
                format!(". Agreed: {first} lands first; {second} rebases onto it. "),
                if self.same_lines {
                    format!("Same lines: {second} resolves the conflict.")
                } else {
                    "Different lines, no conflict.".to_owned()
                },
            ),
            Agreed::Leaves => (
                format!(". Agreed: {second} leaves them to {first}. "),
                format!("{first} carries on there."),
            ),
            Agreed::BuildsOn => (
                format!(". Agreed: {second} builds on {first}'s branch. "),
                format!("{first} lands first."),
            ),
        };
        [
            format!("{second} and {first} both change "),
            self.files(),
            agreed,
            then,
        ]
    }

    /// Its files as one phrase: `a.rs`, `a.rs and b.rs`, `a.rs, b.rs, c.rs and 2 more`.
    #[must_use]
    pub fn files(&self) -> String {
        let (shown, more) = self.listed();
        let mut parts: Vec<String> = shown.into_iter().map(str::to_owned).collect();
        parts.extend(more);
        match parts.split_last() {
            None => "the same files".to_owned(),
            Some((only, [])) => only.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        }
    }
}

/// What the button that puts a one-time note away says.
pub const GOT_IT: &str = "Got it";

/// A one-time note's words ([`crate::onboarding::Note`]), shown once to each person under
/// their first wake or their first settled overlap: its title, set in bold, and what
/// follows it. For a wake, `agent` is the session it woke as the reader's app names it,
/// and `what` the wake's own words ([`Woke::words`]): `Build 612 failed on main, so Zirc
/// woke tls-rotation: …`.
#[must_use]
pub fn note_words(
    note: crate::onboarding::Note,
    agent: &str,
    what: &str,
) -> (&'static str, String) {
    match note {
        crate::onboarding::Note::FirstWake => {
            let mut what = what.to_owned();
            if let Some(first) = what.get(..1) {
                let upper = first.to_uppercase();
                what.replace_range(..1, &upper);
            }
            (
                "This is a wake.",
                format!(
                    "{what}, so {} woke {agent}: no polling, no terminal. From now on it \
                     happens by itself, and you hear what it did here.",
                    crate::NAME
                ),
            )
        }
        crate::onboarding::Note::FirstOverlap => (
            "Your first overlap.",
            format!(
                "{} saw two agents about to change the same files, and they agreed an order \
                 before either pushed. You don't need to do anything: every overlap gets one \
                 line like the one above.",
                crate::NAME
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(state: State, landing: bool) -> Woke {
        Woke {
            build: Some("612".into()),
            state: Some(state),
            branch: Some("main-fix".into()),
            landing,
            ..Woke::new(What::Build, By::Ci)
        }
    }

    #[test]
    fn a_wake_is_said_in_the_chat_whose_message_caused_it_and_any_other_in_the_room() {
        let said = |what, by, channel: Option<&str>| Woke {
            from: Some("rajesh".into()),
            channel: channel.map(str::to_owned),
            ..Woke::new(what, by)
        };
        let named = said(What::Mentioned, By::Person, Some("#general"));
        assert_eq!(named.chat(), Some(Chat::Channel("#general")));
        assert!(named.in_its_chat("#General"));
        assert!(!named.in_its_chat("#agents"));
        assert!(!named.in_its_chat("m3-mac/cc-1"));
        for what in [What::Replied, What::Posted, What::Reacted] {
            assert_eq!(
                said(what, By::Agent, Some("#agents")).chat(),
                Some(Chat::Channel("#agents"))
            );
        }
        // A reaction given in a direct conversation, and a person's direct message.
        assert_eq!(
            said(What::Reacted, By::Person, Some("m3-mac")).chat(),
            Some(Chat::Direct("rajesh"))
        );
        let wrote = said(What::Wrote, By::Person, None);
        assert_eq!(wrote.chat(), Some(Chat::Direct("rajesh")));
        assert!(wrote.in_its_chat("m3-mac/cc-1"));
        assert!(wrote.in_its_chat("fixer"));
        assert!(!wrote.in_its_chat("#agents"));
    }

    #[test]
    fn a_wake_no_chat_caused_is_said_in_the_room() {
        let said = |what, by, channel: Option<&str>| Woke {
            from: Some("rajesh".into()),
            channel: channel.map(str::to_owned),
            ..Woke::new(what, by)
        };
        // What no chat caused, CI's and zirc's word, and what another agent sent it directly.
        for woke in [
            build(State::Failed, true),
            said(What::Asked, By::Agent, None),
            said(What::Noted, By::Agent, None),
            said(What::Reacted, By::Agent, Some("m3-mac")),
            said(What::Mentioned, By::Ci, Some("#general")),
            said(What::Posted, By::Zirc, Some("#general")),
            said(What::Overlap, By::Agent, Some("#agents")),
            said(What::Intends, By::Agent, Some("#agents")),
            said(What::Committed, By::Agent, Some("#agents")),
            said(What::Decided, By::Person, Some("#decisions")),
            Woke::new(What::Reported, By::Zirc),
        ] {
            assert_eq!(woke.chat(), None, "{woke:?}");
            assert!(!woke.in_its_chat("#agents"));
        }
    }

    #[test]
    fn ci_says_how_the_build_went_and_colours_by_outcome() {
        let failed = build(State::Failed, true);
        assert_eq!(failed.words(None), "build 612 failed on main-fix");
        assert_eq!(failed.tone(), Tone::Failed);
        let landed = build(State::Passed, true);
        assert_eq!(
            landed.words(None),
            "CI landed its branch main-fix · build 612"
        );
        assert_eq!(landed.tone(), Tone::Passed);
        let passed = build(State::Passed, false);
        assert_eq!(passed.words(None), "build 612 passed on main-fix");
        assert_eq!(build(State::Running, true).tone(), Tone::Quiet);
        let named = Woke::new(What::Mentioned, By::Ci);
        assert_eq!(named.words(None), "CI has news for it");
        let broken = Woke {
            level: Some(Level::Broken),
            branch: Some("master".into()),
            ..Woke::new(What::State, By::Ci)
        };
        assert_eq!(broken.words(None), "master is broken");
        assert_eq!(broken.tone(), Tone::Failed);
    }

    #[test]
    fn names_whoever_did_it_as_the_reader_knows_them() {
        let replied = Woke {
            from: Some("rajesh".into()),
            channel: Some("#agents".into()),
            ..Woke::new(What::Replied, By::Person)
        };
        assert_eq!(
            replied.words(Some("Rajesh Iyer")),
            "Rajesh Iyer replied in #agents"
        );
        assert_eq!(replied.tone(), Tone::Person);
        let asked = Woke::new(What::Asked, By::Agent);
        assert_eq!(asked.words(Some("replay-index")), "replay-index asked it");
        assert_eq!(asked.tone(), Tone::Agent);
    }

    #[test]
    fn its_event_reads_back_and_its_text_names_no_one() {
        let woke = Woke {
            from: Some("ada-mbp".into()),
            session: Some("cc-2".into()),
            branch: Some("login".into()),
            wake: Some("w1".into()),
            ..Woke::new(What::Landed, By::Agent)
        };
        let event = woke.event();
        assert_eq!(Woke::of(&event), Some(woke));
        let Event::Custom { text, .. } = &event else {
            panic!("a wake is a custom event");
        };
        assert_eq!(text, "woke: changes to its files landed");
        assert_eq!(Settled::of(&event), None);
    }

    #[test]
    fn a_newer_wake_still_reads() {
        let woke: Woke =
            serde_json::from_value(serde_json::json!({"what": "sang", "by": "choir", "later": 1}))
                .unwrap();
        assert_eq!((woke.what, woke.by), (What::Other, By::Zirc));
        // A state it doesn't know is none, and the rest still reads.
        let later: Woke = serde_json::from_value(
            serde_json::json!({"what": "build", "by": "ci", "state": "paused", "build": "7"}),
        )
        .unwrap();
        assert_eq!((later.state, later.build.as_deref()), (None, Some("7")));
        assert_eq!(woke.words(None), "Zirc has news for it");
        assert_eq!(woke.tone(), Tone::Quiet);
    }

    #[test]
    fn the_first_wakes_note_says_what_happened() {
        use crate::onboarding::Note;
        let (title, body) = note_words(
            Note::FirstWake,
            "tls-rotation",
            &build(State::Failed, false).words(None),
        );
        assert_eq!(title, "This is a wake.");
        assert!(
            body.starts_with("Build 612 failed on main-fix, so Zirc woke tls-rotation: no polling"),
            "{body}"
        );
        assert_eq!(
            note_words(Note::FirstOverlap, "", "").0,
            "Your first overlap."
        );
    }

    fn side(session: &str, name: &str) -> Side {
        Side {
            machine: "ada-mbp".into(),
            session: session.into(),
            name: Some(name.into()),
        }
    }

    #[test]
    fn a_settled_overlap_says_who_lands_first_and_whether_it_conflicts() {
        let mut settled = Settled {
            first: side("cc-1", "tls-rotation"),
            second: side("cc-2", "replay-metrics"),
            paths: vec!["server/src/config.rs".into()],
            same_lines: false,
            agreed: Agreed::Order,
        };
        let event = settled.event();
        assert_eq!(Settled::of(&event), Some(settled.clone()));
        let Event::Custom { text, .. } = &event else {
            panic!("a settled overlap is a custom event");
        };
        assert_eq!(
            text,
            "overlap settled: tls-rotation lands first, replay-metrics rebases onto it: \
             server/src/config.rs (different lines, no conflict)"
        );
        assert_eq!(
            settled.sentences("tls-rotation", "replay-metrics").concat(),
            "replay-metrics and tls-rotation both change server/src/config.rs. Agreed: \
             tls-rotation lands first; replay-metrics rebases onto it. Different lines, no \
             conflict."
        );
        assert!(settled.between(("ada-mbp", "cc-2"), ("ADA-MBP", "cc-1")));
        assert!(!settled.between(("ada-mbp", "cc-2"), ("bob-mbp", "cc-1")));
        settled.same_lines = true;
        settled.paths = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        assert_eq!(settled.files(), "a, b, c and 2 more");
        assert_eq!(
            settled.sentences("one", "two")[3],
            "Same lines: two resolves the conflict."
        );
    }

    #[test]
    fn a_report_says_what_was_reported_and_its_item() {
        let report = crate::role_report::RoleReport {
            build: Some("612".into()),
            ..crate::role_report::RoleReport::new(
                "fixer",
                crate::role_report::Subject::Warning,
                "unused import",
            )
        };
        let item = crate::role_report::OpenItem::opened(report, 0);
        let woke = Woke::reported(&item, false);
        // Who found what, then the build; never its id, which only the holder's own watch
        // shows (the user's choice, 25 Nov 2026).
        assert_eq!(
            woke.words(None),
            "CI found a new warning, unused import · build 612"
        );
        assert_eq!(woke.heading("main-fix"), "main-fix woke as the fixer:");
        let Event::Custom { text, .. } = woke.event() else {
            panic!("a wake's event");
        };
        assert_eq!(
            text,
            "woke as the fixer: CI found a new warning, unused import · build 612"
        );
        assert!(
            Woke::reported(&item, true)
                .words(None)
                .starts_with("still open: ")
        );
        assert_eq!(Woke::of(&woke.event()), Some(woke));
    }

    #[test]
    fn what_was_agreed_beyond_an_order_is_said_and_left_out_when_its_only_that() {
        let side = |session: &str| Side {
            machine: "ada-mbp".into(),
            session: session.into(),
            name: None,
        };
        let mut settled = Settled {
            first: side("cc-1"),
            second: side("cc-2"),
            paths: vec!["a.rs".into()],
            same_lines: true,
            agreed: Agreed::Order,
        };
        assert!(
            serde_json::to_value(&settled)
                .unwrap()
                .get("agreed")
                .is_none()
        );
        settled.agreed = Agreed::Leaves;
        assert_eq!(
            settled.event().human_text(),
            "overlap settled: cc-2 leaves a.rs to cc-1"
        );
        assert_eq!(
            settled.sentences("one", "two").concat(),
            "two and one both change a.rs. Agreed: two leaves them to one. one carries on there."
        );
        assert_eq!(Settled::of(&settled.event()), Some(settled.clone()));
        settled.agreed = Agreed::BuildsOn;
        assert_eq!(
            settled.event().human_text(),
            "overlap settled: cc-2 builds on cc-1's branch: a.rs"
        );
        assert_eq!(
            settled.sentences("one", "two")[2..].concat(),
            ". Agreed: two builds on one's branch. one lands first."
        );
    }
}
