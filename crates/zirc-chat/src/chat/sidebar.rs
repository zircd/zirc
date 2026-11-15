//! The sidebar as one flat list of rows, in the order every app draws them
//! ([`Chat::sidebar_list`]): each part's heading, its conversations, a channel's rooms and
//! guests' rooms under it, the folds (a channel's `12 guests`),
//! Browse channels and All messages are all rows. An app draws them as they come and works
//! out nothing of its own: what's folded away isn't in the list, so moving to the next or
//! previous row walks the list as it is.
//!
//! The parts, in order: Starred (anything starred, there alone); Channels (each repository's
//! channel with its rooms and its guests' rooms under it, then Browse channels); Direct
//! messages (people, groups, agent sessions and our offline machines' threads, newest first;
//! an agent session that ended and went quiet isn't listed, till it's opened); Your machines
//! (our machines online, by name, each with its agent sessions under it, listed only while one
//! is or its own thread is unread or shown). A guest is talked to in their room, under its
//! channel, so there's no part for guests. A part with nothing in it is left out.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::conversation::{ChannelTitle, Kind, agent, fold};
use super::face::Face;
use super::list::{ChannelGroup, DirectKind, DirectRow, Shelf, SidebarSection};
use super::{
    Attendance, Avatar, BROWSE_CHANNELS, Chat, NEW_CHANNEL, Presence, ago_short, away_label,
    badge_label, is_group, room_label, runs_on,
};

/// What the row after the first few direct messages says, where an app lists only those
/// ([`ListOptions::direct_limit`]): it shows them all.
pub const ALL_MESSAGES: &str = "All messages";

/// What a channel's or a direct message's row says under its name, faint, while nothing's
/// been said in it ([`ListRow::placeholder`]).
pub const NO_MESSAGES: &str = "No messages yet";

/// What a row of the sidebar is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ListKind {
    /// A part's heading, `Channels`: pressing it folds the part away or opens it
    /// ([`Chat::set_collapsed`]).
    Heading,
    /// A conversation: a repository's channel, a starred room, a direct message or a
    /// conversation with a guest.
    Conversation,
    /// A room under its repository's channel.
    Room,
    /// A guest's room under its channel, by the guest's name: opening it joins it.
    GuestRoom,
    /// Browse channels, after the channels: opens the list of channels to join.
    Browse,
    /// New channel, ending the channels, there with none joined or listed too: asks a name
    /// and makes it ([`Chat::join_offer`]).
    NewChannel,
    /// All messages, after the first few direct messages where an app lists only those.
    AllDirect,
}

/// What an app asks of the sidebar's list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ListOptions {
    /// How many direct messages to list before an All messages row, as the iPhone's Home
    /// lists 3; `None` lists them all.
    pub direct_limit: Option<u32>,
    /// The section to list, as the sidebar's switch shows one at a time ([`Chat::tab`]);
    /// `None` lists both, one after the other.
    pub tab: Option<SidebarSection>,
}

/// A row of the sidebar, as every app draws it ([`Chat::sidebar_list`]).
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a fact about it, independent of the others, as an app draws it"
)]
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ListRow {
    /// What it is.
    pub kind: ListKind,
    /// The part it's in.
    pub shelf: Shelf,
    /// The conversation it opens ([`Chat::open`]); for a channel's guests' fold, the
    /// channel, and for a machine's `N earlier`, the machine; empty for a heading, the other
    /// folds, Browse channels, New channel and All messages.
    pub name: String,
    /// What it says: a heading's part, a room by its own name under its channel, a direct
    /// message as [`DirectRow::label`] says it, a guest by their name, a fold's words,
    /// `Browse channels`, `New channel`, `All messages`.
    pub title: String,
    /// How far it's indented: 1 for a room, a guest's room or the guests' fold under their
    /// channel, and for an agent session or its `N earlier` under its machine, else 0.
    pub depth: u8,
    /// Whether it's open: a heading's part (not folded away), a fold, a channel's guests'
    /// rooms under its row (pressed, [`Chat::set_guest_rooms_open`]); `None` for the rest.
    pub open: Option<bool>,
    /// A line under it: what an agent session at work said it's taking on.
    pub note: Option<String>,
    /// Whether there's a conversation to open: a repository's channel whose rooms alone
    /// we're in has none.
    pub opens: bool,
    /// Whether it's a direct message, with a person, a group, an agent session, a machine
    /// or a guest.
    pub direct: bool,
    /// Whether we're in it: a channel we're in (or were in, while the connection's down), a
    /// direct message always.
    pub joined: bool,
    /// Whether it's starred.
    pub starred: bool,
    /// Whether it's muted: in the muted colour, never bold.
    pub muted: bool,
    /// Whether anything in it is unread, which an app shows bold: for a channel, in it or in
    /// its guests' rooms folded out of sight ([`Chat::is_unread`]). Its rooms, and any guest's
    /// room still listed, are rows of their own, bold for themselves.
    pub unread: bool,
    /// The count it shows ([`Chat::badge`]), a channel's by the same rule as `unread`, a
    /// fold's what's under it.
    pub badge: u32,
    /// That count as its badge says it, `3`, `99+` ([`badge_label`]); `None` for none.
    pub badge_label: Option<String>,
    /// For a direct message, what it's with.
    pub direct_kind: Option<DirectKind>,
    /// For a direct message or a guest, its avatar: the agent's, the person's, a group's up
    /// to three.
    pub faces: Vec<Face>,
    /// A person's or a guest's dot, a guest's room's (here while they're connected).
    pub presence: Option<Presence>,
    /// That a person in a direct message is away, after their name ([`away_label`]).
    pub away_label: Option<String>,
    /// The newest thing said in it, or under it for a channel, as every list previews it;
    /// [`NO_MESSAGES`] for a conversation whose history came back with nothing said
    /// ([`ListRow::placeholder`]).
    pub preview: Option<String>,
    /// Whether [`ListRow::preview`] is [`NO_MESSAGES`], drawn faint: nothing's been said in
    /// it. Never while its history is on its way, so a row doesn't say so and then change.
    pub placeholder: bool,
    /// When that was, as the list says it ([`ago_short`]): `5m`, `3h`.
    pub when: Option<String>,
    /// When something was last said in it, in milliseconds since 1970.
    #[ts(type = "number | null")]
    pub latest_ms: Option<i64>,
    /// How many rooms a repository's channel has under it.
    pub rooms: u32,
    /// Whether it's a guest's: a conversation with a guest, or a guest's room.
    pub guest: bool,
    /// Who a conversation with a guest is assigned to, `assigned to dana`
    /// ([`super::assigned_label`]).
    pub assigned_label: Option<String>,
    /// For a guest's, that it's assigned to us (a dot); for a direct message, that it's
    /// with one of our own agents or machines.
    pub mine: bool,
    /// For a guest's, that the guest spoke last and nobody on staff has answered.
    pub unanswered: bool,
    /// For a guest's, that it's closed: under the Closed fold.
    pub closed: bool,
    /// For a room under its channel, that it's quiet ([`super::Chat::is_quiet_thread`]):
    /// nothing unread nor said in it for [`super::THREAD_ACTIVE_FOR`], and not shown. A
    /// phone leaves it off its list, in its channel's sheet alone, as Discord lists only
    /// active threads (Android, from 14 Nov 2026, the user's choice).
    pub quiet: bool,
    /// Whether it opens a channel, a room under one included ([`super::is_channel`]): what
    /// every app asks before offering a channel's menu, never from its name's first character.
    pub channel: bool,
    /// Its title as a channel's sigil and name, when it's drawn so, its `#` apart: a channel
    /// listed by its name. `None` for a room under its channel, a direct message and the rest,
    /// which say their title whole.
    pub channel_title: Option<ChannelTitle>,
    /// For a direct message with one person, their face: what a conversation's header shows
    /// beside their name, with their dot. `None` for a group, an agent, a machine, a guest and
    /// a channel.
    pub person: Option<Face>,
}

/// How many as a row counts it.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl ListRow {
    /// A row of `kind` in `shelf` that says `title`, nothing else said of it yet.
    fn new(kind: ListKind, shelf: Shelf, title: String) -> Self {
        Self {
            kind,
            shelf,
            name: String::new(),
            title,
            depth: 0,
            open: None,
            note: None,
            opens: false,
            direct: false,
            joined: false,
            starred: false,
            muted: false,
            unread: false,
            badge: 0,
            badge_label: None,
            direct_kind: None,
            faces: Vec::new(),
            presence: None,
            away_label: None,
            preview: None,
            placeholder: false,
            when: None,
            latest_ms: None,
            rooms: 0,
            guest: false,
            assigned_label: None,
            mine: false,
            unanswered: false,
            closed: false,
            quiet: false,
            channel: false,
            channel_title: None,
            person: None,
        }
    }

    /// Says what its name is: a channel, drawn with its `#` apart, or a direct message with
    /// one person ([`ListRow::channel`], [`ListRow::channel_title`], [`ListRow::person`]).
    fn settle_names(&mut self) {
        // A group's name is a channel's too, `#dm-abc`: a direct message is never one.
        self.channel = !self.direct && super::is_channel(&self.name);
        self.channel_title = (self.channel && self.depth == 0)
            .then(|| super::channel_title(&self.title))
            .flatten();
        self.person = (self.direct_kind == Some(DirectKind::Person) && !self.guest)
            .then(|| self.faces.first().cloned())
            .flatten()
            .filter(|face| matches!(face, Face::Person { .. }));
    }

    /// It, counting `badge`.
    fn counting(mut self, badge: usize) -> Self {
        self.badge = count(badge);
        self.badge_label = (badge > 0).then(|| badge_label(badge));
        self
    }

    /// Whether it's a conversation of some kind, which a folded part keeps when it's new or
    /// shown.
    fn is_conversation(&self) -> bool {
        matches!(
            self.kind,
            ListKind::Conversation | ListKind::Room | ListKind::GuestRoom
        )
    }

    /// A fold's row over `under`: what's unread and counted under it.
    fn fold_over(kind: ListKind, shelf: Shelf, title: String, open: bool, under: &[Self]) -> Self {
        let badge: usize = under
            .iter()
            .map(|row| usize::try_from(row.badge).unwrap_or(usize::MAX))
            .sum();
        Self {
            open: Some(open),
            unread: under.iter().any(|row| row.unread),
            ..Self::new(kind, shelf, title)
        }
        .counting(badge)
    }
}

impl Chat {
    /// The sidebar, as every app draws it: one flat list of rows in order, headings, folds,
    /// Browse channels and All messages among them (see the module's doc). What's starred
    /// is under Starred alone, a starred repository channel with its rooms. A folded part
    /// keeps only its heading and the rows unread, counted or shown; a shut fold, nothing
    /// under it; a fold holding the conversation shown is open.
    #[must_use]
    pub fn sidebar_list(&self, options: &ListOptions) -> Vec<ListRow> {
        let shown = self.selected().name.clone();
        let working = self.working_tasks();
        let groups = self.listed_channel_groups();
        // Machines' threads and agent sessions are under Machines, by their machine, and not
        // in the direct messages too: a conversation shows once.
        let online = self.our_machines();
        let (beside_machines, directs): (Vec<DirectRow>, Vec<DirectRow>) = self
            .listed_direct_rows(&working, &online)
            .into_iter()
            .partition(|row| matches!(row.kind, DirectKind::Agent | DirectKind::Machine));
        let mut list = Vec::new();

        let mut starred = Vec::new();
        for name in &self.stars {
            if self.position(name).is_none() {
                continue;
            }
            if let Some(group) = groups.iter().find(|g| g.name.eq_ignore_ascii_case(name)) {
                starred.extend(self.channel_list_rows(group, Shelf::Starred));
            } else if SidebarSection::of(name) == SidebarSection::Direct {
                let row = directs
                    .iter()
                    .find(|row| row.name.eq_ignore_ascii_case(name))
                    .cloned()
                    .unwrap_or_else(|| self.direct_row(name.clone()));
                starred.push(self.direct_list_row(&row, Shelf::Starred, &working));
            } else {
                starred.push(self.room_list_row(name, Shelf::Starred, 0));
            }
        }
        // What's starred shows in its own section's Starred, with a tab chosen.
        if let Some(tab) = options.tab {
            starred.retain(|row| {
                let name = if row.name.is_empty() {
                    &row.title
                } else {
                    &row.name
                };
                self.tab_of(name) == tab
            });
        }
        list.extend(self.sidebar_part(Shelf::Starred, starred, &shown));

        let mut channels: Vec<ListRow> = groups
            .iter()
            .filter(|group| !self.is_starred(&group.name))
            .flat_map(|group| self.channel_list_rows(group, Shelf::Channels))
            .collect();
        let any_channel = !channels.is_empty()
            || !self.browse().is_empty()
            || self.conversations.iter().any(|c| {
                c.kind == Kind::Channel && !is_group(&c.name) && c.standing != Attendance::Left
            });
        if any_channel {
            channels.push(ListRow::new(
                ListKind::Browse,
                Shelf::Channels,
                BROWSE_CHANNELS.to_owned(),
            ));
        }
        // Always, so a server with no channel yet shows how to make its first.
        channels.push(ListRow::new(
            ListKind::NewChannel,
            Shelf::Channels,
            NEW_CHANNEL.to_owned(),
        ));
        list.extend(self.sidebar_part(Shelf::Channels, channels, &shown));

        let direct = self.direct_part(&directs, &working, *options);
        list.extend(self.sidebar_part(Shelf::Direct, direct, &shown));

        let machines = self.machines_part(&online, &beside_machines, &working, &shown);
        list.extend(self.sidebar_part(Shelf::Machines, machines, &shown));

        if let Some(tab) = options.tab {
            list.retain(|row| tab.has(row.shelf));
        }
        for row in &mut list {
            row.settle_names();
            if self.says_no_messages(row) {
                row.preview = Some(NO_MESSAGES.to_owned());
                row.placeholder = true;
            }
        }
        list
    }

    /// Whether `row` says [`NO_MESSAGES`] under its name: a channel's or a direct message's
    /// own row, with nothing to preview or note, once what the server has of it is here:
    /// none of its history on its way, and connected, or its history known to go back no
    /// further. A room under its channel says nothing.
    fn says_no_messages(&self, row: &ListRow) -> bool {
        let Some(conversation) = self.conversation(&row.name) else {
            return false;
        };
        row.kind == ListKind::Conversation
            && (row.depth == 0 || row.direct)
            && row.preview.is_none()
            && row.note.is_none()
            && self.last_said(&row.name).is_none()
            && !self.history_coming(&row.name)
            && (self.link.is_some() || conversation.beginning)
    }

    /// Your machines' rows: each of our machines online that needs us, with its agent
    /// sessions under it.
    fn machines_part(
        &self,
        online: &[String],
        beside_machines: &[DirectRow],
        working: &HashMap<String, (String, String)>,
        shown: &str,
    ) -> Vec<ListRow> {
        // Our machines online, then any other with something listed: one of ours offline, or
        // a teammate's, by its sessions or its thread.
        let mut all: Vec<String> = online.to_vec();
        for row in beside_machines {
            let nick = agent(&row.name).map_or(row.name.as_str(), |(nick, _)| nick);
            if !all.iter().any(|known| known.eq_ignore_ascii_case(nick)) {
                all.push(nick.to_owned());
            }
        }
        let online = &all;
        // A machine that runs on another of ours online (a CI job's host on its warm agent's)
        // is drawn under it, with its own sessions one step in; else it's a machine of its own.
        let parent_of = |nick: &str| {
            runs_on(self.machine_owner(nick), self.realname(nick))
                .and_then(|host| online.iter().find(|m| m.eq_ignore_ascii_case(&host)))
                .filter(|host| !self.is_starred(host))
        };
        // A machine of ours that's connected is always listed. Any other only while it needs
        // us: an agent session listed under it, its thread unread or shown, or a machine
        // under it that is.
        let connected = self.our_machines();
        let mut machines = Vec::new();
        let needs_us = |nick: &str, rows: &[ListRow]| {
            connected.iter().any(|m| m.eq_ignore_ascii_case(nick))
                || rows.iter().any(|row| !row.name.eq_ignore_ascii_case(nick))
                || self.is_unread(nick)
                || self.badge(nick) > 0
                || shown.eq_ignore_ascii_case(nick)
        };
        for nick in online.iter().filter(|nick| parent_of(nick).is_none()) {
            let mut rows = self.machine_rows(nick, 0, beside_machines, working);
            if !self.is_starred(nick) {
                for child in online.iter().filter(|child| {
                    parent_of(child).is_some_and(|host| host.eq_ignore_ascii_case(nick))
                }) {
                    let under = self.machine_rows(child, 1, beside_machines, working);
                    if needs_us(child, &under) {
                        rows.extend(under);
                    }
                }
            }
            if needs_us(nick, &rows) {
                machines.extend(rows);
            }
        }
        machines
    }

    /// The agent sessions at work, ours and our teammates', by folded conversation name:
    /// the conversation's name and what it's taking on ([`Chat::active_now`]).
    fn working_tasks(&self) -> HashMap<String, (String, String)> {
        self.active_now()
            .into_iter()
            .map(|card| (fold(&card.agent.name), (card.agent.name, card.task)))
            .collect()
    }

    /// The direct messages the sidebar lists, newest first: every one of [`Chat::direct_rows`]
    /// but our machine's thread with nothing said in it yet. An agent session is listed while
    /// it's live or was active within [`super::ROOM_QUIET_AFTER`], whether or not anything
    /// was said with it; one quiet longer ([`DirectRow::folded`]) isn't listed at all: Jump
    /// to… and search still find it, and it's back once it's opened or it's active again.
    fn listed_direct_rows(
        &self,
        working: &HashMap<String, (String, String)>,
        online: &[String],
    ) -> Vec<DirectRow> {
        let shown = &self.selected().name;
        let said = |name: &str| {
            self.conversation(name)
                .is_some_and(|c| c.entries.iter().any(super::Entry::is_said))
        };
        let mut rows: Vec<DirectRow> = self
            .direct_rows()
            .into_iter()
            .filter(|row| {
                let ours = match row.kind {
                    // Live or active within the hour; else gone, though it still claims a
                    // task: a claim never dropped mustn't list it until its machine says it
                    // ended, and then not.
                    DirectKind::Agent => return !row.folded,
                    DirectKind::Machine if row.mine => false,
                    _ => return true,
                };
                ours || said(&row.name) || shown.eq_ignore_ascii_case(&row.name)
            })
            .filter(|row| !self.left_out(row, online))
            .collect();
        // Our live sessions, from their first watch, and those at work within the hour,
        // though nothing's been said with them yet.
        let live = self.live_sessions(|nick| self.ours(nick));
        for name in working.values().map(|(name, _)| name).chain(&live) {
            if !rows.iter().any(|row| row.name.eq_ignore_ascii_case(name)) {
                let row = self.direct_row(name.clone());
                if !row.folded {
                    rows.push(row);
                }
            }
        }
        rows.sort_by_cached_key(|row| (std::cmp::Reverse(row.latest), fold(&row.name)));
        rows
    }

    /// A part's rows under its heading: none for a part with nothing in it; while it's
    /// folded away, only its conversations that are unread, counted or shown.
    fn sidebar_part(&self, part: Shelf, mut rows: Vec<ListRow>, shown: &str) -> Vec<ListRow> {
        if rows.is_empty() {
            return rows;
        }
        let collapsed = self.collapsed.contains(&part);
        let heading = ListRow {
            open: Some(!collapsed),
            ..ListRow::new(ListKind::Heading, part, part.label().to_owned())
        };
        if collapsed {
            let mut keep: Vec<bool> = rows
                .iter()
                .map(|row| {
                    row.is_conversation()
                        && (row.unread || row.badge > 0 || row.name.eq_ignore_ascii_case(shown))
                })
                .collect();
            // A row kept under another keeps the one it's under: a room, its channel.
            let mut parent = None;
            for (at, row) in rows.iter().enumerate() {
                if row.depth == 0 {
                    parent = Some(at);
                } else if keep[at]
                    && let Some(parent) = parent
                {
                    keep[parent] = true;
                }
            }
            let mut keep = keep.into_iter();
            rows.retain(|_| keep.next().unwrap_or(false));
        }
        std::iter::once(heading).chain(rows).collect()
    }

    /// A repository channel's rows: its own, with what's new in its guests' rooms folded out
    /// of sight rolled up, then its rooms that aren't starred, then its guests' fold and the guests'
    /// rooms it shows.
    fn channel_list_rows(&self, group: &ChannelGroup, part: Shelf) -> Vec<ListRow> {
        let rooms: Vec<&String> = group
            .rooms
            .iter()
            .filter(|room| !self.is_starred(room))
            .collect();
        let guests = &group.guests;
        let name = &group.name;
        // Its own row is bold for what's new in it, and in its guests' rooms folded out of
        // sight under it. A room with a row of its own, its rooms and any guest's room still
        // listed, folded or not, is bold for itself, and its channel isn't for it.
        let folded_guests = guests
            .rooms
            .iter()
            .filter(|room| !room.shown && !self.is_starred(&room.name))
            .map(|room| room.name.as_str());
        let rolled: Vec<&str> = std::iter::once(name.as_str())
            .chain(folded_guests)
            .collect();
        let head = ListRow {
            name: name.clone(),
            open: (!guests.rooms.is_empty()).then_some(!guests.folded),
            opens: self.conversation(name).is_some(),
            joined: self.is_in(name),
            starred: self.is_starred(name),
            muted: self.is_muted(name),
            unread: rolled.iter().any(|name| self.is_unread(name)),
            preview: group.preview.clone(),
            when: group.when.clone(),
            latest_ms: group.latest.map(|at| at.timestamp_millis()),
            rooms: count(group.rooms.len()),
            ..ListRow::new(ListKind::Conversation, part, self.title(name))
        }
        .counting(rolled.iter().map(|name| self.badge(name)).sum());
        let mut rows = vec![head];
        rows.extend(rooms.iter().map(|room| self.room_list_row(room, part, 1)));
        // What's starred is under Starred alone.
        rows.extend(
            guests
                .shown()
                .filter(|room| !self.is_starred(&room.name))
                .map(|room| {
                    ListRow {
                        name: room.name.clone(),
                        depth: 1,
                        opens: true,
                        joined: room.joined,
                        muted: self.is_muted(&room.name),
                        // Unanswered, it's bold as unread is; assigned to us, it has a dot.
                        unread: room.unread || room.unanswered,
                        unanswered: room.unanswered,
                        mine: room.mine,
                        assigned_label: self
                            .guest_header(&room.name)
                            .and_then(|header| header.assignee)
                            .map(|to| super::assigned_label(&to)),
                        closed: room.closed,
                        guest: true,
                        faces: vec![self.face(&Avatar::Person {
                            nick: room.guest.clone(),
                        })],
                        presence: Some(if room.connected {
                            Presence::Here
                        } else {
                            Presence::Gone
                        }),
                        ..ListRow::new(ListKind::GuestRoom, part, room.label.clone())
                    }
                    .counting(room.badge)
                }),
        );
        rows
    }

    /// A channel's row on its own, `depth` deep: a room under its channel by its own name,
    /// or anything starred that isn't a repository's channel, a direct message or a guest's.
    fn room_list_row(&self, name: &str, part: Shelf, depth: u8) -> ListRow {
        let preview = self.preview(name);
        let kind = if depth > 0 {
            ListKind::Room
        } else {
            ListKind::Conversation
        };
        // A guest's room by the guest's name, as under its channel.
        let title = if let Some((_, guest)) = zirc_proto::guest_room_of(name) {
            self.guest_room_label(guest)
        } else if depth > 0 {
            room_label(name).unwrap_or_else(|| self.title(name))
        } else {
            self.title(name)
        };
        ListRow {
            name: name.to_owned(),
            depth,
            opens: true,
            direct: SidebarSection::of(name) == SidebarSection::Direct,
            joined: self.is_in(name),
            starred: self.is_starred(name),
            muted: self.is_muted(name),
            unread: self.is_unread(name),
            when: preview
                .as_ref()
                .map(|preview| ago_short(preview.at, (self.clock)())),
            preview: preview.map(|preview| preview.text),
            latest_ms: self.last_said(name).map(|at| at.timestamp_millis()),
            quiet: depth > 0 && self.is_quiet_thread(name),
            ..ListRow::new(kind, part, title)
        }
        .counting(self.badge(name))
    }

    /// A direct message's row: an agent session at work says what it's taking on, and never
    /// folds.
    fn direct_list_row(
        &self,
        row: &DirectRow,
        part: Shelf,
        working: &HashMap<String, (String, String)>,
    ) -> ListRow {
        let task = working.get(&fold(&row.name)).map(|(_, task)| task.clone());
        ListRow {
            name: row.name.clone(),
            note: task.clone(),
            opens: true,
            direct: true,
            joined: row.kind != DirectKind::Group || self.is_in(&row.name),
            starred: self.is_starred(&row.name),
            muted: self.is_muted(&row.name),
            unread: self.is_unread(&row.name),
            direct_kind: Some(row.kind),
            faces: row.avatars.iter().map(|avatar| self.face(avatar)).collect(),
            presence: row.presence,
            away_label: (row.kind == DirectKind::Person)
                .then(|| self.away(&row.name).map(away_label))
                .flatten(),
            preview: row.preview.clone(),
            when: row.when.clone(),
            latest_ms: row.latest.map(|at| at.timestamp_millis()),
            mine: row.mine,
            ..ListRow::new(ListKind::Conversation, part, row.label())
        }
        .counting(row.badge)
    }

    /// A machine's rows in Your machines, `depth` deep: its own row, unless it's starred and
    /// so under Starred, then its agent sessions listed, newest first, one step in, by what
    /// each goes by.
    fn machine_rows(
        &self,
        nick: &str,
        depth: u8,
        beside_machines: &[DirectRow],
        working: &HashMap<String, (String, String)>,
    ) -> Vec<ListRow> {
        let starred = self.is_starred(nick);
        let mut rows = Vec::new();
        if !starred {
            rows.push(ListRow {
                depth,
                ..self.machine_list_row(nick.to_owned())
            });
        }
        let depth = depth + u8::from(!starred);
        rows.extend(
            beside_machines
                .iter()
                .filter(|row| {
                    row.kind == DirectKind::Agent
                        && !self.is_starred(&row.name)
                        && agent(&row.name).is_some_and(|(of, _)| of.eq_ignore_ascii_case(nick))
                })
                .map(|row| ListRow {
                    depth,
                    title: row.title.clone(),
                    ..self.direct_list_row(row, Shelf::Machines, working)
                }),
        );
        rows
    }

    /// One of our online machines' row in Your machines, by the machine's own name: it opens
    /// the thread with it, made when it's first opened.
    fn machine_list_row(&self, nick: String) -> ListRow {
        // A teammate's by whose it is too, as its row always said: `bob · laptop`.
        let (owner, machine) = self
            .whose(&nick)
            .unwrap_or_else(|| (self.me.clone(), nick.clone()));
        let mine = owner.eq_ignore_ascii_case(&self.me);
        let title = if mine {
            machine
        } else {
            format!("{} · {machine}", self.owner_name(&owner))
        };
        let online = self
            .our_machines()
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&nick))
            || self.standing(&nick).presence() == Presence::Here;
        let preview = self.preview(&nick);
        let badge = self.badge(&nick);
        ListRow {
            opens: true,
            direct: true,
            joined: true,
            muted: self.is_muted(&nick),
            unread: self.is_unread(&nick),
            direct_kind: Some(DirectKind::Machine),
            faces: vec![self.face(&Self::agents_avatar())],
            presence: Some(if online {
                Presence::Here
            } else {
                Presence::Gone
            }),
            when: preview
                .as_ref()
                .map(|preview| ago_short(preview.at, (self.clock)())),
            preview: preview.map(|preview| preview.text),
            latest_ms: self.last_said(&nick).map(|at| at.timestamp_millis()),
            mine,
            name: nick,
            ..ListRow::new(ListKind::Conversation, Shelf::Machines, title)
        }
        .counting(badge)
    }

    /// The direct messages' rows, what's starred left out: all of them, or, with a limit,
    /// the first that many, then All messages when there are more.
    fn direct_part(
        &self,
        directs: &[DirectRow],
        working: &HashMap<String, (String, String)>,
        options: ListOptions,
    ) -> Vec<ListRow> {
        let mut rows: Vec<ListRow> = directs
            .iter()
            .filter(|row| !self.is_starred(&row.name))
            .map(|row| self.direct_list_row(row, Shelf::Direct, working))
            .collect();
        if let Some(limit) = options.direct_limit {
            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
            let hidden = rows.split_off(limit.min(rows.len()));
            if !hidden.is_empty() {
                rows.push(ListRow {
                    open: None,
                    ..ListRow::fold_over(
                        ListKind::AllDirect,
                        Shelf::Direct,
                        ALL_MESSAGES.to_owned(),
                        false,
                        &hidden,
                    )
                });
            }
        }
        rows
    }

    /// Whether we're in the conversation called `name`: a channel we joined, or were in
    /// while the connection's down (joined again on connecting); a direct message always.
    fn is_in(&self, name: &str) -> bool {
        self.conversation(name).is_some_and(|c| match c.kind {
            Kind::Channel => c.joined || (!self.is_connected() && c.standing != Attendance::Left),
            Kind::Query => true,
            Kind::Server => false,
        })
    }
}
