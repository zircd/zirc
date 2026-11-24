//! What conversations are called, and making groups.

use serde::Serialize;
use ts_rs::TS;

use super::{
    Chat, DirectKind, Kind, Line, SenderKind, agent, agent_name, fold, group, is_channel, is_group,
    machine_label, sender_kind,
};

/// What a press on a conversation's header title opens, in every app (the user's choice, 20
/// Nov 2026): its people, someone's card, or nothing ([`Chat::title_opens`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(rename_all = "camelCase", tag = "kind")]
pub enum TitleOpens {
    /// The people in it: a channel's, a group's.
    People,
    /// `nick`'s card: a person's in a direct message, or with `session` an agent session's
    /// on that machine.
    Card {
        /// Whose card: the person, or the agent's machine.
        nick: String,
        /// The agent session's id, for its own card.
        session: Option<String>,
    },
    /// Nothing: the server's conversation, or our machine's thread, whose sessions are at its
    /// top.
    Nothing,
}

/// What fills the window while nothing's joined and only the server's own conversation is
/// there: no conversation's header and no composer, which would say the server's host.
/// Ready to send as it is: web reads it as JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct NothingJoined {
    /// "You're not in any channel yet".
    pub title: &'static str,
    /// What to do about it.
    pub body: &'static str,
    /// Its button, which takes the person to joining one.
    pub action: &'static str,
}

/// What every app says while nothing's joined.
pub const NOTHING_JOINED: NothingJoined = NothingJoined {
    title: "You're not in any channel yet",
    body: "Join one to start talking with your team.",
    action: "Join a channel",
};

/// What the server's own conversation is called where every app lists it, at the top of the
/// sidebar: what the server and its services said, and where raw IRC lines are typed (the
/// user's choice, 16 Nov 2026).
pub const SERVER_TITLE: &str = "Server";

impl Chat {
    /// What to show in place of a conversation while none is open (the server's own is
    /// selected but not open as the Server channel, [`Chat::show_server`]) and no channel is
    /// joined: [`NOTHING_JOINED`]'s card. With a channel joined, [`Chat::pick_a_conversation`].
    #[must_use]
    pub fn nothing_joined(&self) -> Option<NothingJoined> {
        (self.none_open() && !self.any_joined()).then_some(NOTHING_JOINED)
    }

    /// What to show in place of a conversation while none is open but a channel is joined:
    /// [`super::PICK_A_CONVERSATION`] (the user's choice, 17 Nov 2026). With none joined,
    /// [`Chat::nothing_joined`].
    #[must_use]
    pub fn pick_a_conversation(&self) -> Option<&'static str> {
        (self.none_open() && self.any_joined()).then_some(super::PICK_A_CONVERSATION)
    }

    /// Whether no conversation is open: the server's own is selected, but not as the Server
    /// channel.
    pub(super) fn none_open(&self) -> bool {
        self.selected().kind == Kind::Server && !self.server_shown
    }

    /// Whether any channel is joined.
    fn any_joined(&self) -> bool {
        self.conversations
            .iter()
            .any(|c| c.kind == Kind::Channel && c.joined)
    }

    /// Whether the conversation called `name` is the server's own: the Server channel.
    #[must_use]
    pub fn is_server(&self, name: &str) -> bool {
        self.conversation(name)
            .is_some_and(|conversation| conversation.kind == Kind::Server)
    }

    /// The conversation called `name` as every app shows it in its list: a channel as named,
    /// a person by the name they show as, `Carol Danvers`, as their messages are headed
    /// ([`Chat::shown_name`]), one of our machines as its owner and its own name,
    /// `mattias · mbp`; one of our agent sessions by what it goes by, `checkout`, and a
    /// teammate's by that and whose, `checkout · Maya Chen`.
    #[must_use]
    pub fn title(&self, name: &str) -> String {
        if self.is_server(name) {
            return SERVER_TITLE.to_owned();
        }
        let machine = |nick: &str| match self.machine(nick) {
            Some((owner, machine)) => machine_label(&owner, &machine),
            None => nick.to_owned(),
        };
        match agent(name) {
            // An agent session by what it goes by: ours alone, `checkout`, as its AGENT tag
            // and our list say it's ours; a teammate's with whose, `checkout · Maya Chen`.
            Some((nick, session)) => {
                let owner = self
                    .machine(nick)
                    .map_or_else(|| self.owner_of(nick).to_owned(), |(owner, _)| owner);
                if owner.eq_ignore_ascii_case(&self.me) {
                    self.agent_label(nick, session)
                } else {
                    format!(
                        "{} · {}",
                        self.agent_label(nick, session),
                        self.owner_name(&owner)
                    )
                }
            }
            None if is_group(name) => self.group_title(name),
            None if is_channel(name) => name.to_owned(),
            // One of zirc's own, by the name its messages are headed with: zirc, CI.
            None if sender_kind(name, None) == SenderKind::Integration => {
                self.sender_name(name, None)
            }
            None if self.machine(name).is_some() => machine(name),
            None => self.shown_name(name),
        }
    }

    /// What a conversation says before anything's been said in it, as Discord's: `This is
    /// the start of #design.` and its topic, if it has one, as a second line; for a direct
    /// message, `This is the start of your conversation with Ada Lovelace.`. `None` once
    /// something has been said, and for the server's own conversation.
    #[must_use]
    pub fn intro(&self, name: &str) -> Option<Vec<String>> {
        let conversation = self.conversation(name)?;
        // The server's own conversation is its notices, not anyone's to start.
        if !conversation.entries.is_empty() || conversation.kind == Kind::Server {
            return None;
        }
        let mut lines = vec![if is_channel(name) && !is_group(name) {
            format!("This is the start of {}.", self.short_title(name))
        } else {
            format!(
                "This is the start of your conversation with {}.",
                self.short_title(name)
            )
        }];
        lines.extend(
            conversation
                .topic
                .as_deref()
                .map(str::trim)
                .filter(|topic| !topic.is_empty())
                .map(str::to_owned),
        );
        Some(lines)
    }

    /// What an open conversation's header says under its name, where a phone has two lines:
    /// for an agent session, whose agent it is, as its messages are headed ([`agent_name`]),
    /// `Maya Chen's agent` or `your agent`; `None` for any other conversation.
    #[must_use]
    pub fn subtitle(&self, name: &str) -> Option<String> {
        let (nick, session) = agent(name)?;
        let owner = self
            .machine(nick)
            .map_or_else(|| self.owner_of(nick).to_owned(), |(owner, _)| owner);
        agent_name(Some(session), &self.owner_name(&owner)).1
    }

    /// Whether the header of the conversation called `name` carries the AGENT tag: any agent
    /// session's, a teammate's too (the user's choice, 20 Nov 2026).
    #[must_use]
    pub fn shows_agent_tag(&self, name: &str) -> bool {
        agent(name).is_some()
    }

    /// What a press on the conversation `name`'s header title opens: a channel's or a group's
    /// people, a person's card, an agent session's card, or for the server's conversation and
    /// a machine's thread nothing. Every app reads it, none deciding by the conversation's
    /// kind.
    #[must_use]
    pub fn title_opens(&self, name: &str) -> TitleOpens {
        match self.direct_kind(name) {
            None if self.is_server(name) => TitleOpens::Nothing,
            None | Some(DirectKind::Group) => TitleOpens::People,
            Some(DirectKind::Agent) => {
                agent(name).map_or(TitleOpens::Nothing, |(nick, session)| TitleOpens::Card {
                    nick: nick.to_owned(),
                    session: Some(session.to_owned()),
                })
            }
            Some(DirectKind::Machine) => TitleOpens::Nothing,
            Some(DirectKind::Person) => TitleOpens::Card {
                nick: name.to_owned(),
                session: None,
            },
        }
    }

    /// What every app's header calls the conversation called `name`: its short title
    /// ([`Chat::short_title`]), an agent session by what it goes by (the user's choice, 20 Nov
    /// 2026).
    #[must_use]
    pub fn header_title(&self, name: &str) -> String {
        self.short_title(name)
    }

    /// The conversation called `name` as a composer names it, `Message <short title>`: a
    /// channel as named, an agent session as it goes by ([`Chat::session_label`]), a group
    /// by who's in it, a machine as its owner and
    /// its own name, and a person by the name they show as. Every app's composer says this.
    #[must_use]
    pub fn short_title(&self, name: &str) -> String {
        if self.is_server(name) {
            return SERVER_TITLE.to_owned();
        }
        match agent(name) {
            Some((nick, session)) => self.agent_label(nick, session),
            None if is_group(name) => self.group_title(name),
            None if is_channel(name) => name.to_owned(),
            None if sender_kind(name, None) == SenderKind::Integration => {
                self.sender_name(name, None)
            }
            None => match self.machine(name) {
                Some((owner, machine)) => machine_label(&owner, &machine),
                None => self.shown_name(name),
            },
        }
    }

    /// A group as it's shown: who else is in it, by name, or who we're letting in while
    /// they haven't joined yet.
    pub(super) fn group_title(&self, name: &str) -> String {
        let mut others: Vec<String> = self
            .conversation(name)
            .into_iter()
            .flat_map(|c| c.members.values().map(|member| member.nick.clone()))
            .chain(self.invited.get(&fold(name)).into_iter().flatten().cloned())
            .filter(|nick| !nick.eq_ignore_ascii_case(&self.me))
            .collect();
        others.sort_by_key(|nick| fold(nick));
        others.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        if others.is_empty() {
            "just you".to_owned()
        } else {
            others.join(", ")
        }
    }

    /// Makes a group direct message with `members`, shown once we've joined it ([`group`]).
    /// Returns what to send. Machines are left out, saying why: they never join, and their
    /// agents hear only their owner. Someone never seen here is let in all the same, saying
    /// they'll join once they're on.
    pub fn create_group(&mut self, members: &[String]) -> Vec<Line> {
        let members = self.group_members(members);
        if members.is_empty() {
            self.note("a group needs someone else in it: /group <nick> <nick>…".into());
            return Vec::new();
        }
        self.made += 1;
        let now = (self.clock)().timestamp_nanos_opt().unwrap_or_default();
        let name = group::new_name(self.made ^ now.cast_unsigned());
        self.invited.insert(fold(&name), members.clone());
        self.making.insert(fold(&name), members);
        self.join(&[name])
    }

    /// Who of `members` a group can have, saying why the others can't: not us, nor
    /// machines, which never join a channel by invitation and whose agents hear only their
    /// owner. Someone never seen here is kept, with a word that they'll join once they're on.
    pub(super) fn group_members(&mut self, members: &[String]) -> Vec<String> {
        let mut kept = Vec::new();
        for member in members {
            if member.eq_ignore_ascii_case(&self.me) {
                continue;
            }
            if self.is_machine(member) {
                self.note(format!(
                    "{member} is a machine: it never joins a group, and its agents hear only \
                     its owner. Add its owner, or message its agents in their conversation"
                ));
                continue;
            }
            let seen = self.realname(member).is_some()
                || self
                    .conversations
                    .iter()
                    .any(|c| c.has_member(member) || c.is(member));
            if !seen {
                self.note(format!(
                    "{member} isn't anywhere you are: they'll join once they're on, if that's \
                     someone's account"
                ));
            }
            kept.push(member.clone());
        }
        kept
    }

    /// Lets `members` into the group `name` and invites them. Only who made it can.
    pub fn add_to_group(&mut self, name: &str, members: &[String]) -> Vec<Line> {
        let members = self.group_members(members);
        self.invited
            .entry(fold(name))
            .or_default()
            .extend(members.iter().cloned());
        group::add(name, &members)
    }
}
