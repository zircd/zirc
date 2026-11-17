//! What a conversation's menu offers, decided once for every app: a row's long press or
//! right-click, and the conversation's own menu.

use super::Chat;
use super::conversation::{Kind, is_channel};
use super::guest_rooms::is_guest_room;
use super::notify::{Level, menu as words};

/// How long a [`RowMenu`] offers to mute for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MuteChoice {
    /// What it says: `For 1 hour`.
    pub label: String,
    /// For how many hours, or none for until it's turned back on.
    pub hours: Option<u32>,
}

/// A level a [`RowMenu`] offers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LevelChoice {
    /// Which.
    pub level: Level,
    /// What it says: `All messages`.
    pub label: String,
    /// Whether the conversation notifies at it now.
    pub chosen: bool,
}

/// What a conversation's menu offers, in every app's words, in order: Star, Mark as read
/// while something's unread, Mute (or Unmute), how much it notifies, then Leave for a channel
/// the person is in, Close for a direct message, or Hide for a guest's room the person isn't
/// in (the user's choice, 17 Nov 2026: every app draws all of it).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RowMenu {
    /// `Star`, or `Unstar` when it's starred.
    pub star: String,
    /// Whether it's starred: what the star item sets to its opposite.
    pub starred: bool,
    /// `Mark as read`, while something in it is unread.
    pub mark_read: Option<String>,
    /// Whether it's muted: then [`RowMenu::unmute`] shows, else the mutes.
    pub muted: bool,
    /// `Unmute`.
    pub unmute: String,
    /// `Mute`, the heading over [`RowMenu::mutes`].
    pub mute: String,
    /// How long to mute for.
    pub mutes: Vec<MuteChoice>,
    /// `Notify me about`, the heading over [`RowMenu::levels`].
    pub levels_title: String,
    /// How much it can notify, the one it does chosen.
    pub levels: Vec<LevelChoice>,
    /// `Leave`, for a channel the person is in: it asks first.
    pub leave: Option<String>,
    /// `Close`, for a direct message, not a guest's; `Hide`, for a suggestion listed though
    /// the person isn't in it. Either closes it until something new is said in it.
    pub close: Option<String>,
}

impl Chat {
    /// The menu for the conversation `name`.
    #[must_use]
    pub fn row_menu(&self, name: &str) -> RowMenu {
        let conversation = self.conversation(name);
        let starred = self
            .stars()
            .iter()
            .any(|star| star.eq_ignore_ascii_case(name));
        let unread = conversation.is_some_and(|c| c.unread > 0) || self.badge(name) > 0;
        let joined = conversation.is_some_and(|c| c.joined);
        let direct = conversation.is_some_and(|c| c.kind == Kind::Query);
        let level = self.level(name);
        RowMenu {
            star: if starred { words::UNSTAR } else { words::STAR }.to_owned(),
            starred,
            mark_read: unread.then(|| words::MARK_READ.to_owned()),
            muted: self.is_muted(name),
            unmute: words::UNMUTE.to_owned(),
            mute: words::MUTE.to_owned(),
            mutes: words::MUTE_FOR
                .iter()
                .map(|(label, hours)| MuteChoice {
                    label: (*label).to_owned(),
                    hours: hours.and_then(|hours| u32::try_from(hours).ok()),
                })
                .collect(),
            levels_title: words::NOTIFY_ME_ABOUT.to_owned(),
            levels: Level::ALL
                .into_iter()
                .map(|it| LevelChoice {
                    level: it,
                    label: it.label().to_owned(),
                    chosen: it == level,
                })
                .collect(),
            leave: (!direct && joined && is_channel(name)).then(|| words::LEAVE.to_owned()),
            close: if is_guest_room(name) && !joined {
                Some(words::HIDE.to_owned())
            } else {
                (direct && !self.is_guest_conversation(name) && !is_channel(name))
                    .then(|| words::CLOSE.to_owned())
            },
        }
    }
}
