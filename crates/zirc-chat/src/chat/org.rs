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
    /// The people in its channels, us included: never machines, agents, apps or guests.
    pub members: u32,
    /// That, as every app words it: `1 member`, `3 members`.
    pub members_label: String,
    /// Whether it's as last known, offline ([`Chat::is_stale`]): apps dim it.
    pub stale: bool,
}

/// `count` people, as every app words a count of members: `1 member`, `3 members`.
#[must_use]
pub fn members_label(count: usize) -> String {
    match count {
        1 => "1 member".to_owned(),
        n => format!("{n} members"),
    }
}

impl Chat {
    /// How many people are in the conversation called `name`, as its header counts them:
    /// never machines, agents, apps or guests, as [`Chat::org_header`] counts.
    #[must_use]
    pub fn member_count(&self, name: &str) -> usize {
        self.members_of(name).len()
    }

    /// The nicks of the people in the conversation called `name`, for a count of members.
    fn members_of(&self, name: &str) -> Vec<String> {
        self.people(name)
            .into_iter()
            .filter(|person| person.machine.is_none() && person.app.is_none() && !person.guest)
            .filter(|person| self.sender_kind(&person.nick) == SenderKind::Person)
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
        OrgHeader {
            name: self
                .org_name
                .clone()
                .or_else(|| self.network.clone())
                .unwrap_or_default(),
            members: u32::try_from(people.len()).unwrap_or(u32::MAX),
            members_label: members_label(people.len()),
            stale: self.is_stale(),
        }
    }
}
