//! The conversation list as every app orders it: channels apart from direct messages, the
//! person's starred ones first, then the rest by when something was last said, which the
//! server tells for every conversation at once on connecting (`CHATHISTORY TARGETS`), so the
//! order is right before any history is fetched. The apps keep each person's stars, as
//! [`Chat::stars`] gives them, and hand them back with [`Chat::set_stars`]. (A pin is a
//! message pinned in a conversation, which is another thing.)
//!
//! And for an agent session on one of our machines, what a list row shows of it: its short
//! name and what it said it's taking on.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::conversation::{EntryKind, Kind, agent, default_threshold, fold, is_channel};
use super::{Chat, is_group};
use zirc_proto::Level;

/// One choice of a conversation's "Show" menu ([`Chat::show_choices`]), as every app's
/// room menu lists them: the least a line asks of its readers to be shown there. Ready to
/// draw and send as it is: every app reads this record, the phones through `UniFFI` and web
/// as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ShowChoice {
    /// The threshold it picks ([`Chat::set_threshold`]): `error`, `warn`, `info` or `debug`
    /// as it's sent.
    pub level: Level,
    /// What the menu says: `Errors`, `Warnings`, `Info` or `Everything`.
    pub label: String,
    /// Whether it's the one the conversation has now.
    pub checked: bool,
}

/// What a room's menu calls its "Show" choices, said once for every app.
pub const SHOW_LABEL: &str = "Show";

/// Which part of the sidebar a conversation is in: every app's sidebar shows one at a time,
/// under a `Channels | Messages` switch ([`Chat::tab`]), as Discord keeps a server's channels
/// apart from the direct messages. [`SidebarSection::of`] tells them apart by name alone.
/// Agent sessions and machines' threads are direct messages, among people's (the user's
/// choice, 15 Nov 2026).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum SidebarSection {
    /// The server's channels, with their rooms and their guests' rooms.
    #[default]
    Channels,
    /// Direct messages: with people, groups ([`is_group`]), guests, agent sessions
    /// ([`agent`]) and machines' threads.
    Direct,
}

/// One half of the sidebar's `Channels | Messages` switch, as every app draws it
/// ([`Chat::tabs`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SidebarTab {
    /// The section it shows, which [`Chat::set_tab`] takes.
    pub section: SidebarSection,
    /// What it says: `Channels`, `Messages`.
    pub label: String,
    /// Whether it's the one shown.
    pub chosen: bool,
    /// Whether anything in it is unread: its label goes bold.
    pub unread: bool,
    /// What its conversations count ([`Chat::badge`]), folded away or not: a badge beside its
    /// label when it isn't 0.
    pub badge: u32,
    /// The badge's words ([`super::badge_label`]): `3`, `99+`; `None` when it counts nothing.
    pub badge_label: Option<String>,
}

impl SidebarSection {
    /// Each, in the order the switch shows them.
    pub const ALL: [Self; 2] = [Self::Channels, Self::Direct];

    /// What its switch says, in every app: `Channels`, `Messages`. Where the sidebar is too
    /// narrow for the words, apps draw each one's icon: `#`, a speech bubble.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Channels => "Channels",
            Self::Direct => "Messages",
        }
    }

    /// The parts of the sidebar under it.
    #[must_use]
    pub fn has(self, part: Shelf) -> bool {
        match part {
            // Starred is in both: what's starred shows in its own section's.
            Shelf::Starred => true,
            Shelf::Channels => self == Self::Channels,
            Shelf::Direct => self == Self::Direct,
        }
    }

    /// The section the conversation called `name` is in.
    #[must_use]
    pub fn of(name: &str) -> Self {
        if is_channel(name) && !is_group(name) {
            Self::Channels
        } else {
            Self::Direct
        }
    }
}

/// An agent session on a machine, as a row of the list or the people list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// Its conversation's name: `<machine>/<session>`.
    pub name: String,
    /// The machine's nick, `mattias-mbp`.
    pub machine: String,
    /// The machine's own name, without its owner's, `mbp`: what the session is grouped under.
    pub machine_name: String,
    /// Its short name: the session, `cc-1531f8`.
    pub short: String,
    /// What it said it's taking on, in its latest `taking:` post, until it said it's done
    /// or dropped it.
    pub task: Option<String>,
    /// When something was last said with it, or it last said what it's taking on.
    pub latest: Option<DateTime<Utc>>,
    /// What it goes by, `zirc-chat`, or else its id ([`Chat::session_label`]).
    pub label: String,
    /// Its avatar, under its owner in the people list: see [`super::Avatar::Agent`].
    pub avatar: super::Avatar,
}

/// An agent at work, as the Active now strip shows it, Discord's way: who it is, whose,
/// on which machine, what it's taking on and since when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCard {
    /// The session, as a row shows it: its conversation, label, task and avatar.
    pub agent: Agent,
    /// Whose it is: the machine's owner, by the name they show as, `Bob Lee`
    /// ([`Chat::owner_name`]).
    pub owner: String,
    /// Whether it's ours.
    pub mine: bool,
    /// What it's taking on.
    pub task: String,
    /// When it said so.
    pub since: DateTime<Utc>,
}

/// The room under a repository's channel where its agents' lines go, `#<repo>/agents`,
/// unless the team's config names another (zirc-core's `team`): leaving the repository's
/// channel as its catch-all. What's said there counts as the repository's ([`home_channel`]),
/// and apps join it with the repository's channel.
pub const AGENTS_ROOM: &str = zirc_proto::AGENTS_ROOM;

/// The agents' room of the repository channel `channel`: `#api/agents` for `#api`.
#[must_use]
pub fn agents_room(channel: &str) -> String {
    format!("{channel}/{AGENTS_ROOM}")
}

/// The channel what's said in `name` counts as: a repository's for its agents' room, `#api`
/// for `#api/agents`, and anything else itself. Who's changing what, about to, and taking
/// what on, said in the room, is the repository's.
#[must_use]
pub fn home_channel(name: &str) -> &str {
    match room_parts(name) {
        Some((channel, room)) if room.eq_ignore_ascii_case(AGENTS_ROOM) => channel,
        _ => name,
    }
}

/// A room's channel name, `#zirc/fix-login`, in its two parts: its repository's channel
/// and its own name, `("#zirc", "fix-login")`; `None` for a channel that isn't a room.
/// Every split of a room's name is this one.
#[must_use]
pub fn room_parts(name: &str) -> Option<(&str, &str)> {
    name.split_once('/')
        .filter(|(channel, room)| !channel.is_empty() && !room.is_empty())
}

/// The most a badge counts before it says [`badge_label`]'s `99+`.
pub const BADGE_MOST: usize = 99;

/// What a badge says for `count`, on a row, a tab or the app's icon: the number, up to
/// [`BADGE_MOST`], then `99+`, as Slack and Discord cap theirs.
#[must_use]
pub fn badge_label(count: usize) -> String {
    if count > BADGE_MOST {
        format!("{BADGE_MOST}+")
    } else {
        count.to_string()
    }
}

/// What every app shows for a room, a thread under its repository's channel, by its own
/// name read as words: `fix login` for `#zirc/fix-login`, `login times out` for
/// `#api/login-times-out`; `None` for a channel that isn't a room.
#[must_use]
pub fn room_label(name: &str) -> Option<String> {
    room_parts(name).map(|(_, room)| room.replace(['-', '_'], " "))
}

/// A thread of a channel, as the channel header's "Threads" list shows it: a room under it
/// (`#api/<name>`), a git branch's or one a person started ([`Chat::threads`]). Ready to
/// draw and send as it is: every app reads this record, web as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Thread {
    /// Its channel, `#api/login-times-out`.
    pub name: String,
    /// Its title, without the `/` ([`room_label`]): `login times out`.
    pub title: String,
    /// When something was last said in it, in milliseconds since 1970.
    pub latest_ms: Option<i64>,
    /// What's unread in it.
    pub unread: usize,
    /// Whether its row shows bold, as having something unread, by every row's rule
    /// ([`Chat::is_unread`]): never muted, nor while we're not in it.
    pub bold: bool,
    /// Of that, what mentions us.
    pub mentions: usize,
    /// What its row counts ([`Chat::badge`]).
    pub badge: usize,
    /// That count as its badge says it, `3`, `99+` ([`badge_label`]); `None` for none.
    pub badge_label: Option<String>,
    /// The newest thing said in it ([`Chat::preview`]), or [`super::NO_MESSAGES`] once its
    /// history came back with nothing said ([`Thread::placeholder`]).
    pub preview: Option<String>,
    /// Whether [`Thread::preview`] is [`super::NO_MESSAGES`], drawn faint.
    pub placeholder: bool,
    /// When that was, as the list says it ([`super::ago_short`]): `5m`, `3h`.
    pub when: Option<String>,
    /// The server's id for the message in the channel it was started from, when one was
    /// ([`Chat::start_thread`]); none for a git branch's or a topic's.
    pub of: Option<String>,
    /// How many replies it has, as far as what's loaded of it says ([`Chat::replies`]).
    pub replies: usize,
}

impl Chat {
    /// When something was last said in the conversation called `name`, by anyone, us
    /// included: the time its row shows ([`super::Conversation::last_said`]), or, before
    /// any of its history is here, what the server's `CHATHISTORY TARGETS` said. Never the
    /// later of the two: `TARGETS` counts what no row shows, such as a card's live update, and
    /// a row sorted by it showed `5h` above one showing `3m` (Android, 19 Nov 2026). Anything
    /// newer it knows of comes in the history it has us ask for.
    #[must_use]
    pub fn last_said(&self, name: &str) -> Option<DateTime<Utc>> {
        self.conversation(name)
            .and_then(super::Conversation::last_said)
            .or_else(|| self.active.get(&fold(name)).copied())
    }

    /// Where the conversations of `section` are in [`Chat::conversations`], in the order
    /// every app lists them: starred ones first, in the order they were starred, then the rest
    /// by when something was last said, newest first, and those with nothing said by what
    /// they show ([`Chat::title`]).
    #[must_use]
    pub fn list(&self, section: SidebarSection) -> Vec<usize> {
        let mut listed = self.listed_in(section);
        listed.sort_by_cached_key(|&at| {
            let name = &self.conversations[at].name;
            let starred = self
                .stars
                .iter()
                .position(|star| star.eq_ignore_ascii_case(name));
            let latest = self.last_said(name);
            (
                starred.is_none(),
                starred,
                std::cmp::Reverse(latest),
                fold(&self.title(name)),
            )
        });
        listed
    }

    /// Where the conversations of `section` that [`Chat::list`] lists are, in no order.
    fn listed_in(&self, section: SidebarSection) -> Vec<usize> {
        (0..self.conversations.len())
            .filter(|&at| self.is_listed(at, section))
            .collect()
    }

    /// Whether [`Chat::list`] lists the conversation at `at` in `section`.
    fn is_listed(&self, at: usize, section: SidebarSection) -> bool {
        let conversation = &self.conversations[at];
        conversation.kind != super::Kind::Server
            && conversation.standing != super::Attendance::Left
            && !(conversation.kind == super::Kind::Query && self.is_blocked(&conversation.name))
            && SidebarSection::of(&conversation.name) == section
    }

    /// Where the conversation called `channel` and the rooms under its repository's
    /// channel are, in no order ([`Chat::rooms`]): what one channel's group is made of.
    pub(super) fn under(&self, channel: &str) -> impl Iterator<Item = usize> {
        let repository = room_parts(channel).map_or(channel, |(repository, _)| repository);
        self.position(channel).into_iter().chain(
            self.rooms
                .get(&fold(repository))
                .into_iter()
                .flatten()
                .copied(),
        )
    }

    /// Stars the conversation called `name`, after those starred already, or unstars it.
    pub fn star(&mut self, name: &str, starred: bool) {
        self.stars.retain(|star| !star.eq_ignore_ascii_case(name));
        if starred {
            self.stars.push(name.to_owned());
        }
    }

    /// Whether the conversation called `name` is starred.
    #[must_use]
    pub fn is_starred(&self, name: &str) -> bool {
        self.stars
            .iter()
            .any(|star| star.eq_ignore_ascii_case(name))
    }

    /// The starred conversations' names, in order: what an app keeps for the person.
    #[must_use]
    pub fn stars(&self) -> &[String] {
        &self.stars
    }

    /// The least a line asks of its readers for the person to see it in the conversation
    /// called `name` ([`Conversation::threshold`]): what they picked, or its default.
    #[must_use]
    pub fn threshold(&self, name: &str) -> Level {
        self.conversation(name).map_or_else(
            || {
                self.thresholds
                    .get(&fold(name))
                    .copied()
                    .unwrap_or_else(|| default_threshold(name))
            },
            |conversation| conversation.threshold,
        )
    }

    /// Shows in the conversation called `name` only lines at or above `level` (and what's
    /// addressed to the person, always), as they picked in its menu: its rows and what's
    /// unread are counted again. [`Level::Debug`] shows everything, what's only true now
    /// among it, from when it's picked. Kept for a conversation not yet open, too. The
    /// app keeps it per device and organisation, as stars ([`Chat::thresholds`]).
    pub fn set_threshold(&mut self, name: &str, level: Level) {
        if level == default_threshold(name) {
            self.thresholds.remove(&fold(name));
        } else {
            self.thresholds.insert(fold(name), level);
        }
        let me = self.me.clone();
        if let Some(at) = self.position(name) {
            let conversation = &mut self.conversations[at];
            if conversation.threshold != level {
                conversation.threshold = level;
                conversation.recount(&me);
                conversation.revision += 1;
                conversation.reordered += 1;
            }
        }
    }

    /// The conversation called `name`'s "Show" menu, most to least shown: `Errors`,
    /// `Warnings`, `Info`, `Everything`, the one it has now checked.
    #[must_use]
    pub fn show_choices(&self, name: &str) -> Vec<ShowChoice> {
        let now = self.threshold(name);
        [
            (Level::Error, "Errors"),
            (Level::Warn, "Warnings"),
            (Level::Info, "Info"),
            (Level::Debug, "Everything"),
        ]
        .into_iter()
        .map(|(level, label)| ShowChoice {
            level,
            label: label.to_owned(),
            checked: level == now,
        })
        .collect()
    }

    /// The thresholds the person picked that aren't the default, by conversation, for the
    /// app to keep: [`Chat::set_thresholds`] puts them back.
    #[must_use]
    pub fn thresholds(&self) -> BTreeMap<String, Level> {
        self.thresholds.clone()
    }

    /// Puts back thresholds an app kept ([`Chat::thresholds`]), in place of any picked
    /// before.
    pub fn set_thresholds(&mut self, thresholds: impl IntoIterator<Item = (String, Level)>) {
        let before: Vec<String> = self.thresholds.keys().cloned().collect();
        for name in before {
            self.set_threshold(&name, default_threshold(&name));
        }
        for (name, level) in thresholds {
            self.set_threshold(&name, level);
        }
    }

    /// Stars `stars`, in order, in place of those starred before: as an app kept them.
    pub fn set_stars(&mut self, stars: impl IntoIterator<Item = String>) {
        self.stars.clear();
        for star in stars {
            self.star(&star, true);
        }
    }

    /// The agent session the conversation called `name` is with, on one of our machines,
    /// as a row shows it: its short name and what it's taking on.
    #[must_use]
    pub fn agent(&self, name: &str) -> Option<Agent> {
        let (nick, session) = agent(name)?;
        let (_, machine_name) = self.machine(nick)?;
        Some(self.session_row(nick, session, machine_name))
    }

    /// The agents at work, ours and our teammates', for the Active now strip: each taking
    /// something on, from the agent list every app shares. Ours first, then the newest.
    #[must_use]
    pub fn active_now(&self) -> Vec<ActiveCard> {
        let mut cards: Vec<ActiveCard> = self
            .sessions(|nick| self.whose(nick).is_some())
            .into_iter()
            .filter_map(|name| {
                let (nick, session) = agent(&name)?;
                let (owner, machine_name) = self.whose(nick)?;
                if self.session_live(nick, session) == Some(false) {
                    return None;
                }
                let row = self.session_row(nick, session, machine_name);
                let (task, since) = self.claim(nick, session)?;
                Some(ActiveCard {
                    task,
                    since,
                    mine: owner.eq_ignore_ascii_case(&self.me),
                    owner: self.owner_name(&owner),
                    agent: row,
                })
            })
            .collect();
        cards.sort_by_key(|card| (!card.mine, std::cmp::Reverse(card.since)));
        cards
    }

    /// What the session `session` on the machine `nick` last said it's taking on, and when,
    /// until it said it's done or dropped it.
    pub(super) fn claim(&self, nick: &str, session: &str) -> Option<(String, DateTime<Utc>)> {
        let (_, task, since) = self
            .presence
            .claims_of(nick, session)
            .filter(|(channel, _, _)| {
                self.conversation(channel)
                    .is_some_and(|c| c.kind == super::Kind::Channel)
            })
            .max_by_key(|&(_, _, since)| since)?;
        let since = DateTime::from_timestamp(i64::try_from(since).ok()?, 0)?;
        Some((task.to_owned(), since))
    }

    /// The session `session` on the machine `nick`, whose own name is `machine_name`, as a
    /// row shows it.
    pub(super) fn session_row(&self, nick: &str, session: &str, machine_name: String) -> Agent {
        let name = format!("{nick}{}{session}", super::SESSION_MARK);
        let (task, claimed) = self.claim(nick, session).unzip();
        let owner = self
            .whose(nick)
            .map_or_else(|| nick.to_owned(), |(owner, _)| owner);
        Agent {
            latest: self.last_said(&name).max(claimed),
            label: self.session_label(session),
            avatar: super::Avatar::Agent {
                tool: self.session_tools.get(session).cloned(),
                picture: self.session_pictures.get(session).cloned(),
                hue: super::nick_degrees(session),
                owner,
                session: Some(session.to_owned()),
            },
            name,
            machine: nick.to_owned(),
            machine_name,
            short: session.to_owned(),
            task,
        }
    }

    /// Takes in when something was last said in `target`, as `CHATHISTORY TARGETS` says.
    pub(super) fn said_at(&mut self, target: &str, at: DateTime<Utc>) {
        let latest = self.active.entry(fold(target)).or_insert(at);
        *latest = (*latest).max(at);
    }
}

/// A person as a member list shows them: one row, with their machines under it, collapsed,
/// and each machine's agent sessions under that, the same tree as the sidebar's agents.
/// What anyone is changing stays off it: that's on a session's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonRow {
    /// The person: as they're here, or, when only their machines are, made from what's
    /// known of them, never an operator.
    pub person: super::Person,
    /// Whether the person themselves is here, not only their machines.
    pub here: bool,
    /// Their machines here, by name.
    pub machines: Vec<MachineRow>,
}

/// One of zirc's apps in a conversation, CI or Alerts, as the Apps part of the people list
/// shows it: by its label and glyph, never with an online dot, as an app is no one who's
/// around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRow {
    /// Its nick, `zirc-alerts`.
    pub nick: String,
    /// What it's called, `CI`.
    pub label: String,
    /// Its avatar: [`super::Avatar::App`].
    pub avatar: super::Avatar,
}

/// One of zirc's apps in a conversation as the people list's Apps part draws it, ready to
/// draw and send as it is: [`AppRow`] with its face. Every app reads this record, the
/// phones through `UniFFI` and web as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AppMember {
    /// Its nick, `zirc-alerts`, whose card a click opens.
    pub nick: String,
    /// What it's called, `CI`.
    pub label: String,
    /// Its avatar: its glyph on a square, [`super::Face::App`].
    pub face: super::Face,
}

impl Chat {
    /// zirc's apps in the conversation called `name`, as the people list's Apps part draws
    /// them ([`Chat::app_rows`]).
    #[must_use]
    pub fn app_members(&self, name: &str) -> Vec<AppMember> {
        self.app_rows(name)
            .into_iter()
            .map(|app| AppMember {
                face: self.face(&app.avatar),
                nick: app.nick,
                label: app.label,
            })
            .collect()
    }

    /// zirc's apps in the conversation called `name`, by label: the people list's Apps part,
    /// apart from [`Chat::people_rows`]. Each is called what its latest post there says it
    /// is, as its rows do, `CI`; before it's posted, by its own account, `Alerts`.
    #[must_use]
    pub fn app_rows(&self, name: &str) -> Vec<AppRow> {
        let entries = self
            .conversation(name)
            .map_or(&[][..], |conversation| &conversation.entries[..]);
        // What each nick posted, newest first, in one pass rather than one a person.
        let mut posts: HashMap<String, Vec<&zirc_proto::Message>> = HashMap::new();
        for entry in entries.iter().rev() {
            if let EntryKind::Zirc(message) = &entry.kind {
                posts.entry(fold(&entry.from)).or_default().push(message);
            }
        }
        let mut rows: Vec<AppRow> = self
            .people(name)
            .into_iter()
            .filter_map(|person| {
                let posted = posts.get(&fold(&person.nick)).and_then(|posts| {
                    posts
                        .iter()
                        .find_map(|message| super::app_label(&person.nick, Some(message)))
                });
                let label = posted.or(person.app)?;
                Some(AppRow {
                    avatar: super::Avatar::App {
                        glyph: super::speaker::app_glyph(&label),
                        label: label.clone(),
                    },
                    label: super::speaker::app_name(&label),
                    nick: person.nick,
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            a.label
                .cmp(&b.label)
                .then_with(|| fold(&a.nick).cmp(&fold(&b.nick)))
        });
        rows
    }
}

/// One of a person's machines under their [`PersonRow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineRow {
    /// The machine, as it's here: `machine` names its owner and its own name.
    pub person: super::Person,
    /// Its agent sessions, by short name, each with what it's taking on, its status.
    pub sessions: Vec<Agent>,
}

impl PersonRow {
    /// How the person stands: gone when only their machines are here, else away or here
    /// ([`super::Standing`]).
    #[must_use]
    pub fn standing(&self) -> super::Standing {
        super::Standing::of(self.here, self.person.away.as_deref())
    }

    /// How many agent sessions are at work on their machines.
    #[must_use]
    pub fn agents(&self) -> usize {
        self.machines
            .iter()
            .map(|machine| machine.sessions.len())
            .sum()
    }

    /// The row's line: `alice`, `alice · 2 agents`, `alice · 1 agent`.
    #[must_use]
    pub fn label(&self) -> String {
        match self.agents() {
            0 => self.person.label(),
            1 => format!("{} · 1 agent", self.person.label()),
            n => format!("{} · {n} agents", self.person.label()),
        }
    }
}

impl MachineRow {
    /// Its own name, `mbp`; its nick when it has none, so a row is never nameless.
    #[must_use]
    pub fn name(&self) -> String {
        self.person
            .machine
            .as_ref()
            .map_or_else(|| self.person.nick.clone(), |(_, name)| name.clone())
    }

    /// Its dot ([`super::Presence`]).
    #[must_use]
    pub fn presence(&self) -> super::Presence {
        self.person.standing().presence()
    }

    /// Why it's away, said after its name ([`super::away_label`]), when it is.
    #[must_use]
    pub fn away_label(&self) -> Option<String> {
        self.person.away.as_deref().map(super::away_label)
    }
}

impl PersonRow {
    /// That they're away, said after their name: `away`, `away: lunch`
    /// ([`super::away_label`]); none when they aren't.
    #[must_use]
    pub fn away_label(&self) -> Option<String> {
        self.person.away.as_deref().map(super::away_label)
    }

    /// The pill after their name: `APP` for one of zirc's apps, `GUEST` for a guest, never
    /// the app's own label, which the name already says.
    #[must_use]
    pub fn pill(&self) -> Option<String> {
        if self.person.app.is_some() {
            Some(super::APP_TAG.to_owned())
        } else {
            self.person.guest.then(|| super::GUEST_PILL.to_owned())
        }
    }
}

impl Chat {
    /// What a session's row in the people list says under it: the files it changes that
    /// others do too ([`Chat::both_changing_caption`]), none for none.
    #[must_use]
    pub fn session_both(&self, session: &Agent) -> Option<super::Parts> {
        self.both_changing_caption(&self.both_changing_for(&session.short), &session.short)
    }
}

/// A repository's channel with its rooms, as a row of the channel list shows them: a
/// branch's or a topic's channel, `#api/login`, nests under `#api`, never on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelGroup {
    /// The repository's channel, `#api`: a conversation unless only its rooms are joined.
    pub name: String,
    /// Its rooms' channels, by what was said last, newest first: never a guest's room,
    /// which is in [`ChannelGroup::guests`].
    pub rooms: Vec<String>,
    /// Its guests' rooms, for staff: every kept guest's for owners and admins, and those we
    /// joined ([`super::GuestRooms`]).
    pub guests: super::GuestRooms,
    /// What's unread in it and its rooms.
    pub unread: usize,
    /// Of that, what mentions us.
    pub mentions: usize,
    /// What its row counts, as [`Chat::badge`] counts it and each of its rooms.
    pub badge: usize,
    /// When something was last said in it or its rooms.
    pub latest: Option<DateTime<Utc>>,
    /// The newest thing said in the repository's channel itself, as its row previews it
    /// ([`Chat::preview`]): its rooms preview their own, on their rows under it.
    pub preview: Option<String>,
    /// When that was, as the list says it ([`super::ago_short`]): `5m`, `3h`.
    pub when: Option<String>,
}

/// How long after an agent session was last active, by anything it or anyone said with it,
/// it counts as quiet, and leaves the sidebar unless its machine says it's live: see
/// [`DirectRow::folded`].
pub const ROOM_QUIET_AFTER: chrono::TimeDelta = chrono::TimeDelta::hours(1);

/// How long after an agent session ended, as its machine says, its direct-message row folds
/// away, unless something in it is unread (the user's choice, 15 Nov 2026).
pub const ENDED_FOLD_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(10);

/// How long after something was last said in a room under its channel it counts as an
/// active thread, as Discord archives a quiet thread after a day by default: see
/// [`Chat::is_quiet_thread`].
pub const THREAD_ACTIVE_FOR: chrono::TimeDelta = chrono::TimeDelta::hours(24);

/// What a direct-message row is with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum DirectKind {
    /// A person.
    Person,
    /// A group of people.
    Group,
    /// An agent session on someone's machine, ours or a teammate's.
    Agent,
    /// A machine itself.
    Machine,
}

/// A row of the direct messages: one conversation, whoever it's with, as every app lists
/// them, by the newest thing said, never grouped ([`Chat::direct_rows`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectRow {
    /// The conversation's name: a nick, a group's channel, `<machine>/<session>`.
    pub name: String,
    /// What it's with.
    pub kind: DirectKind,
    /// Its title: a person by the name they show as, an agent session by what it goes by
    /// ([`Chat::session_label`]), a group by who's in it, a machine by owner and name.
    pub title: String,
    /// For an agent session, whose it is, by the name they show as; `None` otherwise.
    pub owner: Option<String>,
    /// Whether it's with one of our own agents or machines.
    pub mine: bool,
    /// Its avatar: the agent's, or those it's with, a group's [`super::GROUP_FACES`]
    /// ([`Chat::faces_of`]).
    pub avatars: Vec<super::Avatar>,
    /// What's unread in it.
    pub unread: usize,
    /// Of that, what mentions us.
    pub mentions: usize,
    /// What its row counts ([`Chat::badge`]).
    pub badge: usize,
    /// When something was last said in it, which orders the list.
    pub latest: Option<DateTime<Utc>>,
    /// The newest thing said in it ([`Chat::preview`]).
    pub preview: Option<String>,
    /// When that was, as the list says it ([`super::ago_short`]): `5m`, `3h`.
    pub when: Option<String>,
    /// A person's dot ([`Chat::standing`]); `None` for the rest.
    pub presence: Option<super::Presence>,
    /// Whether it's an agent session quiet for [`ROOM_QUIET_AFTER`], with nothing unread,
    /// unstarred and not shown: the sidebar leaves it out. One its machine says is live
    /// ([`Chat::session_live`]) never folds, nor do people and groups. Whether it ended or
    /// its machine hasn't said yet doesn't matter, so a list drawn before the machine says
    /// shows what it shows after.
    pub folded: bool,
}

impl DirectRow {
    /// Its row's words on one line: a teammate's agent session by what it goes by and whose
    /// it is, `zirc-chat · Bob Lee`; one of ours by what it goes by alone, as only ours are
    /// listed among the direct messages; anyone else by its title.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.owner {
            Some(owner) if !self.mine => format!("{} · {owner}", self.title),
            _ => self.title.clone(),
        }
    }
}

/// The counts each part of the apps' lists shows on its tab, and in all: each
/// conversation's [`Chat::badge`], added up where the conversation is listed. Counts are
/// `u32`, as the phones' records carry them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Badges {
    /// The channels, and the rooms under them: what mentions us.
    pub channels: u32,
    /// The direct messages tab's: with people, groups and agent sessions, and with guests.
    pub direct: u32,
    /// Of [`Self::direct`], the conversations with guests.
    pub guests: u32,
    /// All of them: the app's icon, as [`Chat::badge_total`].
    pub total: u32,
    /// Activity's rows unread ([`Chat::activity_unread`]): its tab's, or the header inbox's.
    pub activity: u32,
}

impl Chat {
    /// The channel list's rows, in the order of [`Chat::list`]: each repository channel with
    /// its rooms under it, ordered by the row's star (the repository channel's) or the newest
    /// thing said anywhere in it.
    #[must_use]
    pub fn channel_groups(&self) -> Vec<ChannelGroup> {
        self.groups(self.list(SidebarSection::Channels), None)
    }

    /// The channel called `channel`'s group as [`Chat::channel_groups`] has it, but with its
    /// rooms in no order: what one channel's header asks for. Making every channel's group,
    /// each with its preview, to find one made a list of channels with their headers cost
    /// the square of how many there are.
    fn channel_group(&self, channel: &str) -> Option<ChannelGroup> {
        let listed = self
            .under(channel)
            .filter(|&at| self.is_listed(at, SidebarSection::Channels))
            .collect();
        self.groups(listed, Some(channel)).into_iter().next()
    }

    /// The groups of the channels `listed`, in its order, or only `only`'s.
    fn groups(&self, listed: Vec<usize>, only: Option<&str>) -> Vec<ChannelGroup> {
        let wanted = |parent: &str| only.is_none_or(|only| only.eq_ignore_ascii_case(parent));
        let mut groups: Vec<ChannelGroup> = Vec::new();
        let known = self.known_guest_rooms(only);
        let group_of = |groups: &mut Vec<ChannelGroup>, parent: &str| {
            groups
                .iter()
                .position(|g| g.name.eq_ignore_ascii_case(parent))
                .unwrap_or_else(|| {
                    groups.push(ChannelGroup {
                        name: parent.to_owned(),
                        rooms: Vec::new(),
                        guests: super::GuestRooms::default(),
                        unread: 0,
                        mentions: 0,
                        badge: 0,
                        latest: None,
                        preview: None,
                        when: None,
                    });
                    groups.len() - 1
                })
        };
        for at in listed {
            let name = &self.conversations[at].name;
            let parent = room_parts(name).map_or(name.as_str(), |(repo, _)| repo);
            if !wanted(parent) {
                continue;
            }
            let at = group_of(&mut groups, parent);
            let (unread, mentions, latest) = self.counts(name);
            let group = &mut groups[at];
            let guests_room = known
                .iter()
                .any(|(_, room)| room.name.eq_ignore_ascii_case(name));
            if !name.eq_ignore_ascii_case(parent) && !guests_room {
                group.rooms.push(name.clone());
            }
            group.unread += unread;
            group.mentions += mentions;
            group.badge += self.badge(name);
            group.latest = group.latest.max(latest);
        }
        // A channel with guests' rooms is listed for them, even while we aren't in it.
        for (parent, _) in known.iter().filter(|(parent, _)| wanted(parent)) {
            group_of(&mut groups, parent);
        }
        for group in &mut groups {
            group.guests = self.guest_rooms_of(&group.name, &known);
            // Its own: each room previews itself under it, so no line shows twice (the
            // user's choice, 15 Nov 2026).
            (group.preview, group.when) = self.row_preview([group.name.as_str()]);
        }
        self.order(&mut groups, |group| (&group.name, group.latest));
        groups
    }

    /// A row's preview of the newest thing said in the conversations called `names`, and
    /// when, as the list says it.
    fn row_preview<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> (Option<String>, Option<String>) {
        self.newest_preview(names).map_or((None, None), |preview| {
            let when = super::ago_short(preview.at, (self.clock)());
            (Some(preview.text), Some(when))
        })
    }

    /// Whether the conversation `name`, with `unread` and last said in at `latest`, is quiet
    /// enough to fold away: see [`DirectRow::folded`].
    fn quiet(&self, name: &str, unread: usize, latest: Option<DateTime<Utc>>) -> bool {
        let recent = latest.is_some_and(|at| (self.clock)() - at < ROOM_QUIET_AFTER);
        unread == 0
            && !recent
            && !self.is_starred(name)
            && !self.selected().name.eq_ignore_ascii_case(name)
    }

    /// Whether the room called `name`, under its channel, is a quiet thread: nothing unread
    /// in it, nothing said in it for [`THREAD_ACTIVE_FOR`], and it isn't the conversation
    /// shown. A phone lists only the others under their channel ([`super::ListRow::quiet`]).
    #[must_use]
    pub fn is_quiet_thread(&self, name: &str) -> bool {
        let recent = self
            .last_said(name)
            .is_some_and(|at| (self.clock)() - at < THREAD_ACTIVE_FOR);
        !self.is_unread(name) && !recent && !self.selected().name.eq_ignore_ascii_case(name)
    }

    /// The threads of the channel called `channel`, newest first: the rooms under it we know,
    /// quiet ones too, and those the server lists that we aren't in, which opening joins, for
    /// its header's "Threads" list. None for a channel without any.
    #[must_use]
    pub fn threads(&self, channel: &str) -> Vec<Thread> {
        let Some(group) = self.channel_group(channel) else {
            return Vec::new();
        };
        let mut rooms = group.rooms;
        for (name, _) in self.listed() {
            let under =
                room_parts(&name).is_some_and(|(parent, _)| parent.eq_ignore_ascii_case(channel));
            let known = rooms.iter().any(|room| room.eq_ignore_ascii_case(&name))
                || group
                    .guests
                    .rooms
                    .iter()
                    .any(|room| room.name.eq_ignore_ascii_case(&name));
            if under && !known && !super::is_guest_room(&name) {
                rooms.push(name);
            }
        }
        let started = self.started(channel);
        let mut threads: Vec<Thread> = rooms
            .into_iter()
            .map(|name| {
                let (unread, mentions, latest) = self.counts(&name);
                let (preview, when) = self.row_preview([name.as_str()]);
                let placeholder = preview.is_none() && self.nothing_said(&name);
                Thread {
                    of: started
                        .iter()
                        .find(|(_, thread)| thread.eq_ignore_ascii_case(&name))
                        .map(|(of, _)| of.clone()),
                    replies: self.replies(&name),
                    title: room_label(&name).unwrap_or_else(|| name.clone()),
                    badge: self.badge(&name),
                    badge_label: Some(self.badge(&name))
                        .filter(|&badge| badge > 0)
                        .map(badge_label),
                    bold: self.is_unread(&name),
                    latest_ms: latest.map(|at| at.timestamp_millis()),
                    unread,
                    mentions,
                    preview: preview.or_else(|| placeholder.then(|| super::NO_MESSAGES.to_owned())),
                    placeholder,
                    when,
                    name,
                }
            })
            .collect();
        threads.sort_by_key(|thread| std::cmp::Reverse(thread.latest_ms));
        threads
    }

    /// The direct messages, one row each, whoever they're with, newest first, starred first
    /// as every list orders them: never a person's agents rolled under them, nor our own
    /// agents in a group. An old private conversation with a guest is listed only while
    /// something in it is new, or it's shown: a guest is talked to in their room. A
    /// machine's own thread, where zircd writes to its owner, is one of them, online or not
    /// (the user's choice, 16 Nov 2026): its agents are still talked to one at a time, each
    /// in its own conversation.
    #[must_use]
    pub fn direct_rows(&self) -> Vec<DirectRow> {
        // When each session was last heard, once for all the rows.
        let heard = self.heard_all();
        let mut rows: Vec<DirectRow> = self
            .list(SidebarSection::Direct)
            .into_iter()
            .map(|at| self.conversations[at].name.clone())
            // A guest is talked to in their room: an old private conversation with one is
            // listed only while something in it is new, or it's shown.
            .filter(|name| {
                !self.is_guest_conversation(name)
                    || self.is_unread(name)
                    || self.selected().name.eq_ignore_ascii_case(name)
            })
            .map(|name| {
                self.direct_row_heard(name, &|nick, session| {
                    heard.get(&(fold(nick), session.to_owned())).copied()
                })
            })
            .collect();
        self.order(&mut rows, |row| (&row.name, row.latest));
        rows
    }

    /// What the conversation called `name` is with, when it's a direct message: an agent
    /// session, a group, a machine, or else a person. `None` for a channel. The one rule:
    /// the list's rows and a conversation's header both ask it.
    #[must_use]
    pub fn direct_kind(&self, name: &str) -> Option<DirectKind> {
        // A group's name is a channel's, `#dm-abc`: it's asked first.
        Some(if is_group(name) {
            DirectKind::Group
        } else if is_channel(name) || self.is_server(name) {
            // Nor is the Server channel anyone to talk to.
            return None;
        } else if agent(name).is_some() {
            DirectKind::Agent
        } else if self.whose(name).is_some() {
            DirectKind::Machine
        } else {
            DirectKind::Person
        })
    }

    /// The conversation called `name` as its direct-message row shows it: one row, for an
    /// app that needs only that one's, such as the open conversation's faces.
    #[must_use]
    pub fn direct_row(&self, name: String) -> DirectRow {
        self.direct_row_heard(name, &|nick, session| self.heard(nick, session))
    }

    /// [`Chat::direct_row`], when a session was last heard as `heard` says: [`Chat::heard`]
    /// for one row, or [`Chat::heard_all`]'s, made once, for many.
    fn direct_row_heard(
        &self,
        name: String,
        heard: &dyn Fn(&str, &str) -> Option<DateTime<Utc>>,
    ) -> DirectRow {
        let (unread, mentions, latest) = self.counts(&name);
        let (preview, when) = self.row_preview([name.as_str()]);
        let person = |nick: &str| super::Avatar::Person {
            nick: nick.to_owned(),
        };
        let mut row = DirectRow {
            kind: DirectKind::Person,
            title: self.title(&name),
            owner: None,
            mine: false,
            avatars: self
                .faces_of(&name)
                .iter()
                .map(|nick| person(nick))
                .collect(),
            unread,
            mentions,
            badge: self.badge(&name),
            latest,
            preview,
            when,
            presence: None,
            folded: false,
            name,
        };
        row.kind = self.direct_kind(&row.name).unwrap_or(DirectKind::Person);
        if let Some((nick, session_id)) = agent(&row.name) {
            let (owner, machine_name) = self
                .whose(nick)
                .unwrap_or_else(|| (nick.to_owned(), nick.to_owned()));
            let session = self.session_row(nick, session_id, machine_name);
            row.title = session.label;
            row.avatars = vec![session.avatar];
            // Its row sorts by its own conversation's time, which it shows; when the session
            // last claimed its task ([`Agent::latest`]) says it's active, not that it spoke.
            row.mine = owner.eq_ignore_ascii_case(&self.me);
            row.owner = Some(self.owner_name(&owner));
            // Active is anything it said, anywhere we are, or anything said with it.
            let active = row.latest.max(session.latest).max(heard(nick, session_id));
            row.folded = self.session_live(nick, session_id) != Some(true)
                && self.quiet(&row.name, row.unread, active);
            // Ended, it says so where its time was, and folds soon after.
            if let Some(ended) = self.session_ended_at(nick, session_id) {
                row.when = Some(super::live::ENDED.to_owned());
                row.folded = row.folded
                    || (self.quiet(&row.name, row.unread, None)
                        && (self.clock)() - ended >= ENDED_FOLD_AFTER);
            }
        } else if row.kind == DirectKind::Person {
            row.presence = Some(self.standing(&row.name).presence());
        }
        row
    }

    /// Whose machine `nick` is, and its own name: by its real name once the server has said
    /// it, or else by zirc's naming, `<owner>-<machine>`. For grouping rows only, never for
    /// trust: anyone can set a real name. A guest's `guest-…` is nobody's machine, nor is
    /// one of zirc's apps, `zirc-alerts`.
    pub(super) fn whose(&self, nick: &str) -> Option<(String, String)> {
        if self.is_guest(nick) || self.is_app(nick) {
            return None;
        }
        self.machine(nick)
    }

    /// What's unread in the conversation called `name`, what of it mentions us, and when
    /// something was last said there.
    fn counts(&self, name: &str) -> (usize, usize, Option<DateTime<Utc>>) {
        let (unread, mentions) = self
            .conversation(name)
            .map_or((0, 0), |c| (c.unread, c.mentions));
        (unread, mentions, self.last_said(name))
    }

    /// Orders `rows` as [`Chat::list`] does: starred first, in the order starred, then newest
    /// first, then by name.
    fn order<T>(&self, rows: &mut [T], key: impl Fn(&T) -> (&String, Option<DateTime<Utc>>)) {
        rows.sort_by_cached_key(|row| {
            let (name, latest) = key(row);
            let starred = self
                .stars
                .iter()
                .position(|star| star.eq_ignore_ascii_case(name));
            (
                starred.is_none(),
                starred,
                std::cmp::Reverse(latest),
                fold(name),
            )
        });
    }
}

/// A part of the sidebar, in the order every app shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum Shelf {
    /// What the person starred, in the order they starred it.
    Starred,
    /// Every channel, each with its threads, alphabetically: the feeds systems post to
    /// ([`Chat::is_feed`]) among them, whose rows never show bold.
    Channels,
    /// Direct messages: people, groups, agent sessions and machines' threads, newest first.
    Direct,
}

impl Shelf {
    /// The part's name, as every app heads it: `Starred`, `Channels`, `Direct messages`.
    /// Where room is short, such as a tab bar, direct messages are what the sidebar's switch
    /// says, [`SidebarSection::label`].
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starred => "Starred",
            Self::Channels => "Channels",
            Self::Direct => "Direct messages",
        }
    }
}

impl Chat {
    /// The repository channels, each with its rooms, in the order every app lists them, as
    /// [`Chat::sidebar_list`] does: those starred first, in the order they were starred, then the
    /// rest A–Z. For apps that draw a starred channel's rooms under it in Starred too.
    #[must_use]
    pub fn listed_channel_groups(&self) -> Vec<ChannelGroup> {
        let mut groups = self.channel_groups();
        groups.sort_by_cached_key(|group| {
            let star = self
                .stars
                .iter()
                .position(|star| star.eq_ignore_ascii_case(&group.name))
                .unwrap_or(usize::MAX);
            (star, fold(&group.name))
        });
        groups
    }

    /// Whether the conversation called `name` has anything unread, which every list shows
    /// bold: never a muted one, whose mentions still count, nor a channel we're not in.
    /// Every app's bold is this one.
    #[must_use]
    pub fn is_unread(&self, name: &str) -> bool {
        !self.is_muted(name)
            && self
                .conversation(name)
                .is_some_and(|c| c.unread > 0 && (c.kind != Kind::Channel || c.joined))
    }

    /// Folds a part of the sidebar away, or opens it: apps keep it per organisation, as
    /// [`Chat::collapsed`] gives it.
    pub fn set_collapsed(&mut self, part: Shelf, folded: bool) {
        if folded {
            self.collapsed.insert(part);
        } else {
            self.collapsed.remove(&part);
        }
    }

    /// Folds away exactly the parts `folded`, and opens the rest: an app's kept choice, in
    /// full.
    pub fn set_folded_parts(&mut self, folded: &[Shelf]) {
        self.collapsed = folded.iter().copied().collect();
    }

    /// Whether the channel called `name` is a feed, where systems post, rather than a room
    /// people made: a standing room under a repository's channel (`#api/agents`,
    /// [`zirc_proto::STANDING_ROOMS`]), the organisation's `#ops` or `#decisions`, or a
    /// repository's own channel, one with a standing room under it that we know of. Its
    /// traffic never shows bold, in every app's sidebar.
    #[must_use]
    pub fn is_feed(&self, name: &str) -> bool {
        let standing = |room: &str| {
            zirc_proto::STANDING_ROOMS
                .iter()
                .any(|kind| room.eq_ignore_ascii_case(kind))
        };
        if [zirc_proto::state::CHANNEL, zirc_proto::request::CHANNEL]
            .iter()
            .any(|channel| name.eq_ignore_ascii_case(channel))
        {
            return true;
        }
        if let Some((_, room)) = room_parts(name) {
            return standing(room);
        }
        is_channel(name)
            && self.conversations.iter().any(|c| {
                room_parts(&c.name).is_some_and(|(parent, room)| {
                    parent.eq_ignore_ascii_case(name) && standing(room)
                })
            })
    }

    /// The parts of the sidebar folded away, sorted.
    #[must_use]
    pub fn collapsed(&self) -> Vec<Shelf> {
        let mut shelves: Vec<Shelf> = self.collapsed.iter().copied().collect();
        shelves.sort();
        shelves
    }

    /// The section the sidebar shows: the one the person picked, or the one the conversation
    /// they opened last is in. Apps keep it per organisation.
    #[must_use]
    pub fn tab(&self) -> SidebarSection {
        self.tab
    }

    /// Shows `tab` in the sidebar ([`Chat::tab`]).
    pub fn set_tab(&mut self, tab: SidebarSection) {
        self.tab = tab;
    }

    /// The sidebar's switch: each section, in order, with what's new in it.
    #[must_use]
    pub fn tabs(&self) -> Vec<SidebarTab> {
        SidebarSection::ALL
            .into_iter()
            .map(|section| {
                let (unread, badge) = self
                    .conversations
                    .iter()
                    .filter(|c| c.kind != Kind::Server && SidebarSection::of(&c.name) == section)
                    .fold((false, 0usize), |(unread, badge), c| {
                        (
                            // A feed's traffic never makes its half bold: its mentions count.
                            unread || (self.is_unread(&c.name) && !self.is_feed(&c.name)),
                            badge + self.badge(&c.name),
                        )
                    });
                SidebarTab {
                    section,
                    label: section.label().to_owned(),
                    chosen: section == self.tab,
                    unread,
                    badge: u32::try_from(badge).unwrap_or(u32::MAX),
                    badge_label: (badge > 0).then(|| super::badge_label(badge)),
                }
            })
            .collect()
    }
}

/// What's new in the conversation shown, from its New divider on ([`Chat::divider`]): the
/// pill over the messages while the divider is above the screen, "↑ 28 new", which takes the
/// person up to it, the terminal's divider, "28 new messages since 13:58", and the count on
/// the button that takes the person down to the newest. Ready to draw and send as it is: every app reads this record, the
/// phones through `UniFFI` and web as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Unseen {
    /// The messages from the divider on.
    pub count: u32,
    /// When the first of them was said, in milliseconds since 1970.
    pub since_ms: i64,
    /// The bar's words, as Discord's: `28 new messages since 13:58`, `1 new message since
    /// Thu 09:12`, the time as a profile card says it.
    pub label: String,
    /// The pill's over the messages, while the divider is above the screen: `↑ 28 new`, a
    /// tap going up to the first of them. A conversation opens at its newest message, so
    /// what's new is above it. Empty once the New line has been on screen, or the pill
    /// closed ([`Chat::saw_divider`]): then there's no pill.
    pub above: String,
    /// The button's, which takes the person down to the newest: `↓ 28 new`.
    pub short: String,
}

impl Chat {
    /// What's new in the conversation shown, from its New divider on: none without one.
    /// `wall` puts a time on the app's clock, as message times are.
    #[must_use]
    pub fn unseen(&self, wall: impl Fn(DateTime<Utc>) -> NaiveDateTime) -> Option<Unseen> {
        let id = self.divider()?;
        let conversation = self.conversations.get(self.selected)?;
        let entries = &conversation.entries;
        let from = entries
            .iter()
            .position(|entry| entry.id.as_deref() == Some(id))?;
        let count = entries[from..]
            .iter()
            .filter(|entry| conversation.counts(entry, &self.me))
            .count();
        let since = entries[from].time;
        let messages = if count == 1 { "message" } else { "messages" };
        Some(Unseen {
            count: u32::try_from(count).unwrap_or(u32::MAX),
            since_ms: since.timestamp_millis(),
            label: format!(
                "{count} new {messages} since {}",
                super::card_time(wall(since), wall((self.clock)()))
            ),
            above: if self.divider_seen.as_deref() == Some(id) {
                String::new()
            } else {
                format!("↑ {count} new")
            },
            short: format!("↓ {count} new"),
        })
    }

    /// The id of the first message in the conversation called `name` the person hasn't
    /// read, where a view draws the "New" divider: the first since how far they've read it,
    /// when the server says, or else the first of those that arrived unread.
    #[must_use]
    pub fn first_unread(&self, name: &str) -> Option<String> {
        let conversation = self.conversation(name)?;
        if conversation.unread == 0 {
            return None;
        }
        let unread = conversation
            .entries
            .iter()
            .filter(|entry| conversation.counts(entry, &self.me));
        if let Some(read) = conversation.read {
            return unread
                .filter(|entry| entry.time > read)
                .find_map(|entry| entry.id.clone());
        }
        let counted: Vec<_> = unread.collect();
        let from = counted.len().checked_sub(conversation.unread)?;
        counted[from..].iter().find_map(|entry| entry.id.clone())
    }

    /// The count a row of the conversation called `name` shows, as Slack and Discord do:
    /// what mentions us in a channel, and everything unread in a direct message, which is
    /// meant for us. Unread that isn't counted shows only as a bold row. A muted channel,
    /// or one set to notify nothing, still counts what mentions us; a direct message set to
    /// notify nothing counts nothing.
    #[must_use]
    pub fn badge(&self, name: &str) -> usize {
        let quiet = self.level(name) == super::Level::Nothing;
        self.conversation(name)
            .map_or(0, |conversation| match conversation.kind {
                Kind::Server => 0,
                Kind::Channel if !is_group(name) => conversation.mentions,
                Kind::Channel | Kind::Query if quiet => 0,
                Kind::Channel | Kind::Query => conversation.unread,
            })
    }

    /// The one count for the whole organisation, which the tabs and the app's icon show
    /// alike: every conversation's [`Chat::badge`].
    #[must_use]
    pub fn badge_total(&self) -> usize {
        usize::try_from(self.badges().total).unwrap_or(usize::MAX)
    }

    /// What each part of the lists counts on its tab, and in all ([`Badges`]). Every app's
    /// tabs show these and count nothing of their own.
    #[must_use]
    pub fn badges(&self) -> Badges {
        let mut badges = Badges::default();
        for conversation in &self.conversations {
            let name = &conversation.name;
            let count = u32::try_from(self.badge(name)).unwrap_or(u32::MAX);
            match conversation.kind {
                Kind::Server => continue,
                _ if self.is_guest_conversation(name) => {
                    badges.guests = badges.guests.saturating_add(count);
                    badges.direct = badges.direct.saturating_add(count);
                }
                Kind::Channel if !is_group(name) => {
                    badges.channels = badges.channels.saturating_add(count);
                }
                Kind::Channel | Kind::Query => badges.direct = badges.direct.saturating_add(count),
            }
            badges.total = badges.total.saturating_add(count);
        }
        badges.activity = u32::try_from(self.activity_unread()).unwrap_or(u32::MAX);
        badges
    }
}
