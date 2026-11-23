//! The sidebar as one flat list of rows, in the order every app draws them
//! ([`Chat::sidebar_list`]): each part's heading, its conversations and a channel's rooms
//! under it are all rows. An app draws them as they come and works
//! out nothing of its own: what's folded away isn't in the list, so moving to the next or
//! previous row walks the list as it is.
//!
//! The parts, in order: Starred (anything starred, there alone); Channels (each channel with
//! its rooms under it); Direct
//! messages (people, groups, agent sessions and machines' own threads, newest first; an
//! agent session that ended and went quiet isn't listed, till it's opened). A part with nothing in it is left out. The Server channel, Browse channels and New channel have no rows: Jump to
//! finds the first two, and every app's + makes a channel (the user's choices, 19 Nov 2026).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::conversation::{Kind, fold};
use super::face::Face;
use super::list::{ChannelGroup, DirectKind, DirectRow, Shelf, SidebarSection};
use super::{Attendance, Chat, Presence, ago_short, badge_label, room_label};

/// What a channel's, a thread's or a direct message's row says under its name, faint, while
/// nothing's been said in it ([`UnderKind::Nothing`]).
pub const NO_MESSAGES: &str = "No messages yet";

/// The line under a conversation's name in the sidebar, as every app draws it
/// ([`ListRow::under`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RowUnder {
    /// What it says.
    pub text: String,
    /// What it is, which says how it's drawn.
    pub kind: UnderKind,
}

impl RowUnder {
    /// The line under a row's name: its `note` while it has one, else its `preview`, which
    /// is [`NO_MESSAGES`] when `placeholder` (the user's choice, 22 Nov 2026).
    #[must_use]
    pub fn of(note: Option<String>, preview: Option<String>, placeholder: bool) -> Option<Self> {
        let (text, kind) = match (note, preview) {
            (Some(note), _) => (note, UnderKind::Note),
            (None, Some(said)) if placeholder => (said, UnderKind::Nothing),
            (None, Some(said)) => (said, UnderKind::Said),
            (None, None) => return None,
        };
        Some(Self { text, kind })
    }
}

/// What the line under a row's name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum UnderKind {
    /// An agent session away with messages waiting: `Not listening for 17 min · 1 message
    /// waiting` ([`Chat::not_listening`]), in the warning colour, in place of anything else.
    NotListening,
    /// What an agent session at work said it's taking on ([`ListRow::note`]), muted.
    Note,
    /// The newest thing said in it, muted, in the text's colour while it's unread.
    Said,
    /// [`NO_MESSAGES`], faint.
    Nothing,
}

/// What a row of the sidebar is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ListKind {
    /// A part's heading, `Channels`: pressing it folds the part away or opens it
    /// ([`Chat::set_collapsed`]).
    Heading,
    /// A conversation: a repository's channel, a starred room, or a direct message.
    Conversation,
    /// A room under its repository's channel.
    Room,
}

/// What an app asks of the sidebar's list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ListOptions {
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
    /// The conversation it opens ([`Chat::open`]); for a machine's `N earlier`, the
    /// machine; empty for a heading and the other folds.
    pub name: String,
    /// What it says: a heading's part, a room by its own name under its channel, a direct
    /// message as [`DirectRow::label`] says it, a fold's words.
    pub title: String,
    /// How far it's indented: 1 for a room under its channel, and for an agent session or its `N earlier` under its machine, else 0.
    pub depth: u8,
    /// Whether it's open: a heading's part (not folded away) or a fold; `None` for the rest.
    pub open: Option<bool>,
    /// A line under it: what an agent session at work said it's taking on.
    pub note: Option<String>,
    /// Whether there's a conversation to open: a repository's channel whose rooms alone
    /// we're in has none. Every row that opens one has its menu ([`Chat::row_menu`]), in
    /// every app.
    pub opens: bool,
    /// Whether it's a direct message, with a person, a group, an agent session or a
    /// machine.
    pub direct: bool,
    /// Whether we're in it: a channel we're in (or were in, while the connection's down), a
    /// direct message always.
    pub joined: bool,
    /// Whether it's starred.
    pub starred: bool,
    /// Whether it's muted: in the muted colour, never bold.
    pub muted: bool,
    /// Whether anything in it is unread, which an app shows bold: ([`Chat::is_unread`]). A
    /// channel's rooms are rows of their own, bold for themselves.
    pub unread: bool,
    /// Whether it shows the unread pip beside its name: a channel's or a room's while unread and not muted, never a direct message's, which shows unread by its
    /// bold name and badge alone (the user's choice, 20 Nov 2026).
    pub pip: bool,
    /// Whether its name is bold, in the text's colour: while it's unread and not muted. Every
    /// other row's name is muted, a thread's and a channel we're not in alike (the user's
    /// choices, 22 Nov 2026); an app draws the conversation shown in the text's colour too.
    pub bold: bool,
    /// The count it shows ([`Chat::badge`]), a channel's by the same rule as `unread`, a
    /// fold's what's under it.
    pub badge: u32,
    /// That count as its badge says it, `3`, `99+` ([`badge_label`]); `None` for none.
    pub badge_label: Option<String>,
    /// For a direct message, what it's with.
    pub direct_kind: Option<DirectKind>,
    /// For a direct message, its avatar: the agent's, the person's, a group's
    /// [`super::GROUP_FACES`], the first drawn in front ([`super::Chat::faces_of`]).
    pub faces: Vec<Face>,
    /// A person's dot.
    pub presence: Option<Presence>,
    /// The one line under a conversation's name: its note while it has one, else the newest
    /// thing said in it, or under it for a channel, as every list previews it (the user's
    /// choice, 22 Nov 2026); [`NO_MESSAGES`] for one whose history came back with nothing
    /// said, never while its history is on its way, so a row doesn't say so and then change.
    /// `None` for a heading, a fold and a row with nothing to say. Every app draws this.
    pub under: Option<RowUnder>,
    /// When that was, as the list says it ([`ago_short`]): `5m`, `3h`.
    pub when: Option<String>,
    /// When something was last said in it, in milliseconds since 1970.
    #[ts(type = "number | null")]
    pub latest_ms: Option<i64>,
    /// How many rooms a repository's channel has under it.
    pub rooms: u32,
    /// Whether it opens a channel, a room under one included ([`super::is_channel`]): what
    /// every app asks before offering a channel's menu, never from its name's first character.
    pub channel: bool,
    /// For a direct message with one person, their face: what a conversation's header shows
    /// beside their name, with their dot. `None` for a group, an agent, a machine and a
    /// channel.
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
            pip: false,
            bold: false,
            badge: 0,
            badge_label: None,
            direct_kind: None,
            faces: Vec::new(),
            presence: None,
            under: None,
            when: None,
            latest_ms: None,
            rooms: 0,
            channel: false,
            person: None,
        }
    }

    /// Says what its name is: a channel, or a direct message with one person
    /// ([`ListRow::channel`], [`ListRow::person`]).
    fn settle_names(&mut self) {
        // A group's name is a channel's too, `#dm-abc`: a direct message is never one.
        self.channel = !self.direct && super::is_channel(&self.name);
        self.person = (self.direct_kind == Some(DirectKind::Person))
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
        matches!(self.kind, ListKind::Conversation | ListKind::Room)
    }
}

impl Chat {
    /// The sidebar, as every app draws it: one flat list of rows in order, headings and folds
    /// among them (see the module's doc). What's starred
    /// is under Starred alone, a starred repository channel with its rooms. A folded part
    /// keeps only its heading and the rows unread, counted or shown; a shut fold, nothing
    /// under it; a fold holding the conversation shown is open.
    #[must_use]
    pub fn sidebar_list(&self, options: &ListOptions) -> Vec<ListRow> {
        let shown = self.selected().name.clone();
        let working = self.working_tasks();
        let groups = self.listed_channel_groups();
        // Agent sessions and machines' threads are direct messages among people's, newest
        // first (the user's choice, 15 Nov 2026).
        let directs = self.listed_direct_rows();
        // No Server row: the server's channel is found through Jump to (the user's choice,
        // 19 Nov 2026).
        let mut list: Vec<ListRow> = Vec::new();

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
                SidebarSection::of(name) == tab
            });
        }
        list.extend(self.sidebar_part(Shelf::Starred, starred, &shown));

        // Every channel, the feeds systems post to among them (no part of their own, the
        // user's choice, 19 Nov 2026): a feed's traffic never shows bold, though what
        // mentions us still counts.
        let channels: Vec<ListRow> = groups
            .iter()
            .filter(|group| !self.is_starred(&group.name))
            .flat_map(|group| {
                let quiet = self.is_feed(&group.name);
                self.channel_list_rows(group, Shelf::Channels)
                    .into_iter()
                    .map(move |row| ListRow {
                        unread: row.unread && !quiet,
                        ..row
                    })
            })
            .collect();
        list.extend(self.sidebar_part(Shelf::Channels, channels, &shown));

        let direct = self.direct_part(&directs, &working);
        list.extend(self.sidebar_part(Shelf::Direct, direct, &shown));

        if let Some(tab) = options.tab {
            list.retain(|row| tab.has(row.shelf));
        }
        for row in &mut list {
            row.settle_names();
            row.bold = row.unread && !row.muted;
            row.pip = row.bold
                && !row.direct
                && matches!(row.kind, ListKind::Conversation | ListKind::Room);
            if self.says_no_messages(row) {
                row.under = RowUnder::of(None, Some(NO_MESSAGES.to_owned()), true);
            }
        }
        list
    }

    /// Whether `row` says [`NO_MESSAGES`] under its name: a channel's, a room's under its
    /// channel or a direct message's, with nothing to preview or note
    /// ([`Chat::nothing_said`]).
    fn says_no_messages(&self, row: &ListRow) -> bool {
        matches!(row.kind, ListKind::Conversation | ListKind::Room)
            && row.under.is_none()
            && self.nothing_said(&row.name)
    }

    /// Whether nothing's been said in the conversation called `name`, as far as the server
    /// says: none of its history on its way, and connected, or its history known to go back
    /// no further. So a row never says [`NO_MESSAGES`] and then changes as history comes.
    pub(super) fn nothing_said(&self, name: &str) -> bool {
        self.conversation(name).is_some_and(|conversation| {
            self.last_said(name).is_none()
                && !self.history_coming(name)
                && (self.link.is_some() || conversation.beginning)
        })
    }

    /// The agent sessions at work, ours and our teammates', by folded conversation name:
    /// the conversation's name and what it's taking on ([`Chat::active_now`]).
    fn working_tasks(&self) -> HashMap<String, (String, String)> {
        self.active_now()
            .into_iter()
            .map(|card| (fold(&card.agent.name), (card.agent.name, card.task)))
            .collect()
    }

    /// Whether the sidebar lists no direct message at all, folded away or not
    /// ([`Self::listed_direct_rows`]).
    pub(super) fn has_no_listed_directs(&self) -> bool {
        self.listed_direct_rows().is_empty()
    }

    /// The direct messages the sidebar lists, newest first: every one of [`Chat::direct_rows`]
    /// a person's or a group's; an agent session's or a machine's own thread only
    /// once something's been said in it, there and not in a channel (the user's choice, 15 Nov 2026), as an agent that exists but never wrote to us is nothing to read. An agent
    /// session whose task has landed or that has been away 12 hours ([`DirectRow::folded`])
    /// isn't listed either: Jump to… and search still find it, and it's back once it's opened
    /// or something's said with it again. The one shown is always listed.
    fn listed_direct_rows(&self) -> Vec<DirectRow> {
        let shown = &self.selected().name;
        let said = |name: &str| {
            self.conversation(name)
                .is_some_and(|c| c.entries.iter().any(super::Entry::is_said))
        };
        let mut rows: Vec<DirectRow> = self
            .direct_rows()
            .into_iter()
            .filter(|row| {
                shown.eq_ignore_ascii_case(&row.name)
                    || match row.kind {
                        DirectKind::Agent => said(&row.name) && !row.folded,
                        // Whatever counts has a row: a machine's own thread too.
                        DirectKind::Machine => said(&row.name),
                        DirectKind::Person | DirectKind::Group => true,
                    }
            })
            .filter(|row| !self.left_out(row))
            .collect();
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

    /// A repository channel's rows: its own, then its rooms that aren't starred.
    fn channel_list_rows(&self, group: &ChannelGroup, part: Shelf) -> Vec<ListRow> {
        let name = &group.name;
        let head = ListRow {
            name: name.clone(),
            opens: self.conversation(name).is_some(),
            joined: self.is_in(name),
            starred: self.is_starred(name),
            muted: self.is_muted(name),
            unread: self.is_unread(name),
            under: RowUnder::of(None, group.preview.clone(), false),
            when: group.when.clone(),
            latest_ms: group.latest.map(|at| at.timestamp_millis()),
            rooms: count(group.rooms.len()),
            ..ListRow::new(ListKind::Conversation, part, self.title(name))
        }
        .counting(self.badge(name));
        let mut rows = vec![head];
        // A room under it is listed while it's an active thread, as Discord lists them: a
        // quiet one is in its channel's threads alone ([`Chat::threads`], the user's choice,
        // 14 Nov 2026). What's starred is under Starred alone.
        rows.extend(
            group
                .rooms
                .iter()
                .filter(|room| !self.is_starred(room) && !self.is_quiet_thread(room))
                .map(|room| self.room_list_row(room, part, 1)),
        );
        rows
    }

    /// A channel's row on its own, `depth` deep: a room under its channel by its own name,
    /// or anything starred that isn't a repository's channel, or a direct message.
    fn room_list_row(&self, name: &str, part: Shelf, depth: u8) -> ListRow {
        let preview = self.preview(name);
        let kind = if depth > 0 {
            ListKind::Room
        } else {
            ListKind::Conversation
        };
        let title = if depth > 0 {
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
            under: RowUnder::of(None, preview.map(|preview| preview.text), false),
            latest_ms: self.last_said(name).map(|at| at.timestamp_millis()),
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
            under: row
                .not_listening
                .clone()
                .map(|text| RowUnder {
                    text,
                    kind: UnderKind::NotListening,
                })
                .or_else(|| RowUnder::of(task, row.preview.clone(), false)),
            when: row.when.clone(),
            latest_ms: row.latest.map(|at| at.timestamp_millis()),
            ..ListRow::new(ListKind::Conversation, part, row.label())
        }
        .counting(row.badge)
    }

    /// The direct messages' rows, what's starred left out.
    fn direct_part(
        &self,
        directs: &[DirectRow],
        working: &HashMap<String, (String, String)>,
    ) -> Vec<ListRow> {
        directs
            .iter()
            .filter(|row| !self.is_starred(&row.name))
            .map(|row| self.direct_list_row(row, Shelf::Direct, working))
            .collect()
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
