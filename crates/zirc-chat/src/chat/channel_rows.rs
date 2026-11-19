//! The channel list as one flat list: every channel the server lists, our private
//! conversations and groups, and the agent sessions on our own machines, each with whether
//! we're in it, what's unread and how many are in it. The sidebar's shelves
//! ([`super::Chat::sidebar_list`]) are built on more than this; what an app needs of the whole
//! list at once, such as the channels we're not in to join with a click, is here.

use std::collections::BTreeMap;

use super::{Chat, Kind, SidebarSection, fold};

/// A row of the channel list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    /// The channel's name, or the other nick's, or `<machine>/<session>` for an agent.
    pub name: String,
    /// What it shows, as every zirc app names it ([`Chat::title`]): a channel's name,
    /// `mattias · mbp`, `mattias · mbp / cc-1531f8`, or a group's other members.
    pub title: String,
    /// Which part of the list it's in.
    pub section: SidebarSection,
    /// Whether we're in it.
    pub joined: bool,
    /// Entries since it was last looked at.
    pub unread: usize,
    /// How many are in it, from the member list once joined, else from the server's list.
    pub users: usize,
}

impl Chat {
    /// The channel list: the server's channels, then direct messages, each sorted by what it
    /// shows. Never the Server channel, even while it's shown: Jump to is where it's found
    /// (the user's choice, 19 Nov 2026); [`Chat::shown_row`] is what's open. Direct messages are our private conversations and groups, and the agent
    /// sessions on our own machines, whether or not we've talked to them yet.
    #[must_use]
    pub fn channel_rows(&self) -> Vec<ChannelRow> {
        let row = |name: &str| ChannelRow {
            name: name.to_owned(),
            title: self.title(name),
            section: SidebarSection::of(name),
            joined: !super::is_channel(name),
            unread: 0,
            users: 0,
        };
        let mut rows: BTreeMap<String, ChannelRow> = self
            .listed()
            .iter()
            .map(|(name, users)| {
                let row = ChannelRow {
                    users: *users,
                    ..row(name)
                };
                (fold(name), row)
            })
            .chain(self.agents().iter().map(|name| (fold(name), row(name))))
            .collect();
        for conversation in self.conversations() {
            if conversation.kind == Kind::Server {
                continue;
            }
            let row = rows
                .entry(fold(&conversation.name))
                .or_insert_with(|| row(&conversation.name));
            row.joined = conversation.joined || conversation.kind == Kind::Query;
            row.unread = conversation.unread;
            if conversation.joined {
                row.users = conversation.members.len();
            }
        }
        let mut rows: Vec<ChannelRow> = rows.into_values().collect();
        rows.sort_by_cached_key(|row| (row.section, fold(&row.title)));
        rows
    }

    /// The row of the conversation shown, listed or not: what an app draws the open
    /// conversation by, its title and section, the Server channel's too. `None` while none is
    /// open ([`Chat::nothing_joined`], [`Chat::pick_a_conversation`]).
    #[must_use]
    pub fn shown_row(&self) -> Option<ChannelRow> {
        if self.none_open() {
            return None;
        }
        self.row_of(&self.selected().name)
    }

    /// The row of the conversation called `name`, listed or not, as [`Chat::channel_rows`]
    /// would list it: for one no list holds, such as the Server channel Jump to finds.
    #[must_use]
    pub fn row_of(&self, name: &str) -> Option<ChannelRow> {
        let conversation = self.conversation(name)?;
        let name = conversation.name.as_str();
        let users = if conversation.joined {
            conversation.members.len()
        } else {
            self.listed()
                .iter()
                .find(|(listed, _)| fold(listed) == fold(name))
                .map_or(0, |(_, users)| *users)
        };
        Some(ChannelRow {
            name: name.to_owned(),
            title: self.title(name),
            section: SidebarSection::of(name),
            joined: conversation.joined || conversation.kind != Kind::Channel,
            unread: conversation.unread,
            users,
        })
    }
}
