//! What conversations are called, and making groups.

use serde::Serialize;
use ts_rs::TS;

use super::{
    Chat, Kind, Line, SenderKind, YOUR_AGENTS, agent, agent_name, fold, group, is_channel,
    is_group, machine_label, sender_kind,
};

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

impl Chat {
    /// What to show in place of the conversation shown, when that's the server's own: as when
    /// nothing's joined yet. Its notices are no one's to read as a conversation or to write to.
    #[must_use]
    pub fn nothing_joined(&self) -> Option<NothingJoined> {
        (self.selected().kind == Kind::Server).then_some(NOTHING_JOINED)
    }

    /// The conversation called `name` as every app shows it in its list: a channel as named,
    /// a person by the name they show as, `Carol Danvers`, as their messages are headed
    /// ([`Chat::shown_name`]), one of our machines as its owner and its own name,
    /// `mattias · mbp`; one of our agent sessions by what it goes by, `checkout`, and a
    /// teammate's by that and whose, `checkout · Maya Chen`; a guest as [`Chat::guest_name`] names them; and, to its guest, their
    /// room of their own as the public channel beside it ([`Chat::our_room_beside`]).
    #[must_use]
    pub fn title(&self, name: &str) -> String {
        if let Some(parent) = self.our_room_beside(name) {
            return parent;
        }
        if self.is_guest_conversation(name) {
            return self.guest_name(name);
        }
        let machine = |nick: &str| match self.machine(nick) {
            // Our own machine's thread holds what we said to its agents, never a person: it
            // reads as ours, not as a conversation with ourselves.
            Some((_, machine)) if self.ours(nick) => format!("{YOUR_AGENTS} · {machine}"),
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
                    self.session_label(session)
                } else {
                    format!(
                        "{} · {}",
                        self.session_label(session),
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

    /// The conversation called `name` as a composer names it, `Message <short title>`: a
    /// channel as named, an agent session as it goes by ([`Chat::session_label`]), a group
    /// by who's in it, a guest as [`Chat::guest_name`] names them, a machine as its owner and
    /// its own name, and a person by the name they show as. Every app's composer says this.
    #[must_use]
    pub fn short_title(&self, name: &str) -> String {
        if let Some(parent) = self.our_room_beside(name) {
            return parent;
        }
        if self.is_guest_conversation(name) {
            return self.guest_name(name);
        }
        match agent(name) {
            Some((_, session)) => self.session_label(session),
            None if is_group(name) => self.group_title(name),
            None if is_channel(name) => name.to_owned(),
            None if sender_kind(name, None) == SenderKind::Integration => {
                self.sender_name(name, None)
            }
            None => match self.machine(name) {
                // `Message every agent on matt`: what a message to our machine reaches.
                Some((_, machine)) if self.ours(name) => format!("every agent on {machine}"),
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
