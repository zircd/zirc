//! The organisation the chat is in, as the channel list's header shows it, Discord's
//! server header: its name and how many people are in it.

use super::{Chat, SenderKind, fold};

/// The channel list's header.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct OrgHeader {
    /// The organisation's name, `Acme`: as the app signed in to it says
    /// ([`Chat::set_org_name`]), or else as the server calls its network, `acme`.
    pub name: String,
    /// What its mark shows without a picture: `A`, `ZL` ([`crate::orgs::org_initials`]).
    pub initials: String,
    /// The people in its channels, us included: never machines, agents or apps.
    pub members: u32,
    /// That, as every app words it: `1 member`, `3 members`.
    pub members_label: String,
    /// Whether it's as last known, offline ([`Chat::is_stale`]): apps dim it.
    pub stale: bool,
    /// `Mark all read`, while anything in it is unread: in the menu atop the list on the
    /// phones and web, as the desktops have it in their menu bar (the user's choice, 17 Nov
    /// 2026). None, and no menu, while everything's read.
    pub mark_all_read: Option<String>,
}

/// `count` people, as every app words a count of members: `1 member`, `3 members`.
#[must_use]
pub fn members_label(count: usize) -> String {
    match count {
        1 => "1 member".to_owned(),
        n => format!("{n} members"),
    }
}

/// `count` threads, as a channel's header counts them: `1 thread`, `2 threads`.
#[must_use]
pub fn threads_label(count: usize) -> String {
    match count {
        1 => "1 thread".to_owned(),
        n => format!("{n} threads"),
    }
}

impl Chat {
    /// What a channel's header says under its name, where its people, threads and the rest
    /// sit behind a tap on that name (Android, from 14 Nov 2026, the user's choice): how many
    /// are in it, its threads and its topic, `4 members · 2 threads · The web app`, each
    /// only when there's any. `None` for anything but a channel, and for one with none.
    #[must_use]
    pub fn channel_about(&self, name: &str) -> Option<String> {
        if !super::is_channel(name) || super::is_group(name) {
            return None;
        }
        let members = self.member_count(name);
        let threads = self.threads(name).len();
        let topic = self
            .conversation(name)
            .and_then(|channel| channel.topic.as_deref())
            .map(str::trim)
            .filter(|topic| !topic.is_empty());
        let parts: Vec<String> = [
            (members > 0).then(|| members_label(members)),
            (threads > 0).then(|| threads_label(threads)),
            topic.map(str::to_owned),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// How many people are in the conversation called `name`, as its header counts them:
    /// never machines, agents or apps, as [`Chat::org_header`] counts, but always us.
    #[must_use]
    pub fn member_count(&self, name: &str) -> usize {
        self.members_of(name).len()
    }

    /// The nicks of the people in the conversation called `name`, for a count of members.
    fn members_of(&self, name: &str) -> Vec<String> {
        self.people(name)
            .into_iter()
            .filter(|person| person.machine.is_none() && person.app.is_none())
            .filter(|person| person.me || self.sender_kind(&person.nick) == SenderKind::Person)
            .map(|person| person.nick)
            .collect()
    }

    /// Names the organisation as zirc.dev does, `Acme`, for [`Chat::org_header`].
    pub fn set_org_name(&mut self, name: Option<&str>) {
        self.org_name = name.map(str::to_owned);
    }

    /// The channel list's header: the organisation's name and its people.
    #[must_use]
    pub fn org_header(&self) -> OrgHeader {
        let mut people: std::collections::HashSet<String> = self
            .conversations
            .iter()
            .filter(|conversation| conversation.kind == super::Kind::Channel)
            .flat_map(|conversation| self.members_of(&conversation.name))
            .map(|nick| fold(&nick))
            .collect();
        people.insert(fold(&self.me));
        let name = self
            .org_name
            .clone()
            .or_else(|| self.network.clone())
            .unwrap_or_default();
        OrgHeader {
            initials: crate::orgs::org_initials(&name),
            name,
            members: u32::try_from(people.len()).unwrap_or(u32::MAX),
            members_label: members_label(people.len()),
            stale: self.is_stale(),
            mark_all_read: self
                .conversations
                .iter()
                .any(|conversation| conversation.unread > 0)
                .then(|| super::conversation_menu::MARK_ALL_READ.to_owned()),
        }
    }
}
