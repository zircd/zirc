//! Guests' rooms, as every app lists them for an organisation's owners and admins: each guest
//! of a public channel has a room of their own beside it (`#support/guest-21042`,
//! [`zirc_proto::guest_room`]), which the server keeps secret, so it's in no `LIST`. zirc.dev
//! knows every guest the organisation keeps (14 days after they were last seen), connected or
//! not, and tells owners and admins at [`KEPT_GUESTS_PATH`]; the app hands that to
//! [`Chat::set_kept_guests`].
//!
//! Each channel's row of the list then has its guests' rooms under it
//! ([`super::ChannelGroup::guests`]), by the guest's name when they gave one and else their
//! nick, connected ones first. They're only listed: opening one ([`Chat::open`]) joins it, so
//! nothing is heard, nor notifies, from a room nobody opened.
//!
//! They fold under their channel's own row, which says whether they're open
//! ([`Chat::set_guest_rooms_open`]); folded, as by default, a room still shows while something
//! in it is new or unanswered, or it's the conversation shown ([`GuestRoom::new`]), as a
//! folded part of the sidebar keeps what's new. A room we leave is hidden too, as a room we
//! aren't in can be ([`Chat::close_direct`]): out of the list, open or folded, until something
//! new is said in it or it's opened, as a closed direct message is.
//!
//! Anyone on staff who joined a guest's room sees it there too, an admin or not.

use serde::Deserialize;

use super::Chat;
use super::conversation::fold;
use super::guests::{GATEWAY_REALNAME, guest_label};

/// Where zirc.dev lists an organisation's kept guests for its owners and admins: `GET`, with
/// the organisation's slug for `{org}`, answers `{"guests": [{"nick", "name", "channels",
/// "connected", …}]}`, the latest seen first. Anyone else is refused (403).
pub const KEPT_GUESTS_PATH: &str = "/api/organisations/{org}/guests";

/// How often an app asks zirc.dev for the kept guests again while it's connected as an owner
/// or admin, so a guest who came since shows: every two minutes.
pub const KEPT_GUESTS_EVERY: std::time::Duration = std::time::Duration::from_secs(120);

/// [`KEPT_GUESTS_PATH`] for the organisation `org` (its slug).
#[must_use]
pub fn kept_guests_path(org: &str) -> String {
    KEPT_GUESTS_PATH.replace("{org}", org)
}

/// A guest the organisation keeps, as zirc.dev lists them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KeptGuest {
    /// Their nick, `guest-21042`.
    pub nick: String,
    /// The name they gave, or their profile's for one signed in; zirc.dev says `Guest 2104`
    /// ([`guest_label`]) for none.
    #[serde(default)]
    pub name: Option<String>,
    /// The public channels they've been in, or their rooms beside them, as far as the server
    /// knows now.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Their rooms of their own, whether they've been in them lately or not: what keeps a
    /// guest who's away listed once the server has forgotten where they were.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Whether they're connected now.
    #[serde(default)]
    pub connected: bool,
}

/// zirc.dev's answer at [`KEPT_GUESTS_PATH`].
#[derive(Deserialize)]
struct KeptGuests {
    guests: Vec<KeptGuest>,
}

/// The kept guests in zirc.dev's answer at [`KEPT_GUESTS_PATH`].
///
/// # Errors
///
/// When it isn't one.
pub fn parse_kept_guests(json: &str) -> Result<Vec<KeptGuest>, serde_json::Error> {
    serde_json::from_str::<KeptGuests>(json).map(|answer| answer.guests)
}

/// Whether the channel called `name` is a guest's room, `#support/guest-21042`.
#[must_use]
pub fn is_guest_room(name: &str) -> bool {
    zirc_proto::guest_room_of(name).is_some()
}

impl KeptGuest {
    /// The rooms of theirs that zirc.dev names: each public channel's beside it, and each
    /// room of theirs as it is, once each, lowercase.
    #[must_use]
    pub fn rooms(&self) -> Vec<String> {
        let mut rooms: Vec<String> = Vec::new();
        for channel in self.channels.iter().chain(&self.rooms) {
            let room = match zirc_proto::guest_room_of(channel) {
                Some((_, guest)) if guest.eq_ignore_ascii_case(&self.nick) => {
                    channel.to_lowercase()
                }
                None if channel.starts_with('#') && !channel.contains('/') => {
                    zirc_proto::guest_room(channel, &self.nick)
                }
                // Another guest's room is never theirs, nor a thread of a channel.
                _ => continue,
            };
            if !rooms.contains(&room) {
                rooms.push(room);
            }
        }
        rooms
    }
}

/// A guest's room, as a row under its channel shows it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a fact about it, independent of the others, as the row shows it"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestRoom {
    /// The room's channel, `#support/guest-21042`, which opening joins.
    pub name: String,
    /// The guest's nick.
    pub guest: String,
    /// What the row says: the guest's name when they gave one, else their nick.
    pub label: String,
    /// Whether they're connected now, as zirc.dev last said or the room shows.
    pub connected: bool,
    /// Whether we're in it.
    pub joined: bool,
    /// Whether anything in it is unread ([`Chat::is_unread`]).
    pub unread: bool,
    /// What its row counts ([`Chat::badge`]).
    pub badge: usize,
    /// Whether we're in it, the guest spoke last and nobody on staff has answered: its row
    /// is bold, as unread is.
    pub unanswered: bool,
    /// Whether it's assigned to us: its row has a dot.
    pub mine: bool,
    /// Whether someone closed it and the guest hasn't spoken since.
    pub closed: bool,
    /// Whether something in it is new or unanswered, or it's the conversation shown: it
    /// shows with its channel's guests folded.
    pub new: bool,
    /// Whether its row shows: while it's [`GuestRoom::new`], or its channel's guests are
    /// open.
    pub shown: bool,
}

/// A channel's guests' rooms, as listed under it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestRooms {
    /// Every room listed: connected guests first, then as zirc.dev listed them, latest seen
    /// first; never one we left or hid while nothing new was said in it since.
    pub rooms: Vec<GuestRoom>,
    /// Whether they're folded now: by default, till the person opens the channel's row.
    pub folded: bool,
}

impl GuestRooms {
    /// The rooms whose rows show.
    pub fn shown(&self) -> impl Iterator<Item = &GuestRoom> {
        self.rooms.iter().filter(|room| room.shown)
    }
}

impl Chat {
    /// Takes in the organisation's kept guests, as zirc.dev lists them to its owners and
    /// admins ([`parse_kept_guests`]): what the channels list their rooms from.
    pub fn set_kept_guests(&mut self, guests: Vec<KeptGuest>) {
        self.kept_guests = guests;
    }

    /// The kept guests zirc.dev last listed.
    #[must_use]
    pub fn kept_guests(&self) -> &[KeptGuest] {
        &self.kept_guests
    }

    /// Whether `name`, a guest's room we aren't in, is hidden: the person hid it or left it
    /// ([`Chat::close_direct`]) and nothing newer was said in it since.
    pub(super) fn hidden(&self, name: &str) -> bool {
        let joined = self.conversation(name).is_some_and(|c| c.joined);
        !joined
            && self
                .closed
                .get(&fold(name))
                .is_some_and(|at| self.last_said(name).is_none_or(|latest| latest <= *at))
    }

    /// Whether the conversation `name` is a suggestion the sidebar lists though we aren't in
    /// it, a guest's room, whose menu hides it ([`super::conversation_menu::HIDE`]) rather
    /// than leaves it.
    #[must_use]
    pub fn is_suggestion(&self, name: &str) -> bool {
        is_guest_room(name) && self.is_staff() && !self.conversation(name).is_some_and(|c| c.joined)
    }

    /// Opens a channel's folded guests' rooms, or folds them again: a press on the channel
    /// row's fold mark ([`super::ListRow::open`]). Apps keep it as long as the session.
    pub fn set_guest_rooms_open(&mut self, channel: &str, open: bool) {
        self.guest_rooms_open.insert(fold(channel), open);
    }

    /// The guest `nick`'s name, as their room's row says it: the name zirc.dev has for them,
    /// else the one they connected with, else their nick.
    #[must_use]
    pub fn guest_room_label(&self, nick: &str) -> String {
        let placeholder = guest_label(nick);
        let given = |name: &str| {
            let name = name.trim();
            (!name.is_empty() && name != GATEWAY_REALNAME && name != placeholder)
                .then(|| name.to_owned())
        };
        self.kept_guests
            .iter()
            .find(|guest| guest.nick.eq_ignore_ascii_case(nick))
            .and_then(|guest| guest.name.as_deref().and_then(given))
            .or_else(|| self.realname(nick).and_then(given))
            .unwrap_or_else(|| nick.to_lowercase())
    }

    /// Every guest's room we know of, with its channel: those zirc.dev lists to an owner or
    /// admin, and any we joined. Each once, in the rooms' order. With `only`, those under
    /// that channel alone, without looking through every conversation.
    pub(super) fn known_guest_rooms(&self, only: Option<&str>) -> Vec<(String, GuestRoom)> {
        let selected = &self.selected().name;
        let mut known: Vec<(String, GuestRoom)> = Vec::new();
        let mut add = |chat: &Self, room: String, guest: &str, connected: bool| {
            if known
                .iter()
                .any(|(_, known)| known.name.eq_ignore_ascii_case(&room))
            {
                return;
            }
            let Some((parent, _)) = zirc_proto::guest_room_of(&room) else {
                return;
            };
            if only.is_some_and(|only| !only.eq_ignore_ascii_case(parent)) {
                return;
            }
            let conversation = chat.conversation(&room);
            let joined = conversation.is_some_and(|c| c.joined);
            let unread = chat.is_unread(&room);
            let badge = chat.badge(&room);
            let connected =
                connected || conversation.is_some_and(|c| joined && c.has_member(guest));
            // Unanswered only while we're in it: one we left is no longer ours to answer.
            let (unanswered, mine, closed) = chat.room_marks(&room);
            let unanswered = unanswered && joined;
            let new = unread || badge > 0 || unanswered || selected.eq_ignore_ascii_case(&room);
            // Left or hidden, it's out of the list till something new is said in it.
            if !new && chat.hidden(&room) {
                return;
            }
            known.push((
                parent.to_lowercase(),
                GuestRoom {
                    label: chat.guest_room_label(guest),
                    guest: guest.to_lowercase(),
                    connected,
                    joined,
                    unread,
                    badge,
                    unanswered,
                    mine,
                    closed,
                    new,
                    shown: new,
                    name: room,
                },
            ));
        };
        if self.admin && self.is_staff() {
            for guest in &self.kept_guests {
                for room in guest.rooms() {
                    add(self, room, &guest.nick, guest.connected);
                }
            }
        }
        if self.is_staff() {
            let ats: Vec<usize> = match only {
                Some(channel) => self.under(channel).collect(),
                None => (0..self.conversations.len()).collect(),
            };
            for conversation in ats.into_iter().map(|at| &self.conversations[at]) {
                if let Some((_, guest)) = zirc_proto::guest_room_of(&conversation.name)
                    && conversation.joined
                {
                    add(self, conversation.name.to_lowercase(), guest, false);
                }
            }
        }
        known
    }

    /// The guests' rooms under `channel`, as its row lists them: see [`GuestRooms`].
    pub(super) fn guest_rooms_of(
        &self,
        channel: &str,
        known: &[(String, GuestRoom)],
    ) -> GuestRooms {
        let mut rooms: Vec<GuestRoom> = known
            .iter()
            .filter(|(parent, _)| parent.eq_ignore_ascii_case(channel))
            .map(|(_, room)| room.clone())
            .collect();
        // Connected first; otherwise as listed, the latest seen first.
        rooms.sort_by_key(|room| !room.connected);
        let folded = !self
            .guest_rooms_open
            .get(&fold(channel))
            .copied()
            .unwrap_or(false);
        if !folded {
            for room in &mut rooms {
                room.shown = true;
            }
        }
        GuestRooms { rooms, folded }
    }
}
