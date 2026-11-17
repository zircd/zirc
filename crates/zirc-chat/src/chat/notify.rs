//! How much each conversation notifies, the same in every app and in push: all new
//! messages, mentions only, or nothing, as Slack and Discord ask it, and muted for a while
//! or until turned back on. Direct messages are meant for the person, so at Mentions they
//! notify as a mention does. The apps keep the settings per organisation
//! ([`Chat::notify_settings`]) and hand them back ([`Chat::set_notify_settings`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::conversation::{Conversation, Entry, EntryKind, fold, is_channel};
use super::{Chat, is_group};

/// How much a conversation notifies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "NotifyLevel", rename_all = "lowercase")]
pub enum Level {
    /// Every new message: what Watch was.
    All,
    /// Messages that mention the person, and in a direct message every message. The default.
    #[default]
    Mentions,
    /// Nothing but what mentions the person in a channel, which counts in the badge too.
    Nothing,
}

impl Level {
    /// Every level, in the order a menu offers them.
    pub const ALL: [Self; 3] = [Self::All, Self::Mentions, Self::Nothing];

    /// What a conversation's menu calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All messages",
            Self::Mentions => "Mentions",
            Self::Nothing => "Nothing",
        }
    }
}

/// What a conversation's menu says, as every app says it (the user's choice of words on
/// 10 Nov 2026: iOS's, short, under the Mute heading).
pub mod menu {
    /// Stars the conversation.
    pub const STAR: &str = "Star";
    /// Unstars it.
    pub const UNSTAR: &str = "Unstar";
    /// Marks it read: offered only while something in it is unread.
    pub const MARK_READ: &str = "Mark as read";
    /// Marks every conversation read, atop the list on the phones and web: offered only
    /// while something is unread. The desktops' menu bars say it in title case
    /// ([`crate::chat::menu_bar::MARK_ALL_READ`]), as macOS's menus do.
    pub const MARK_ALL_READ: &str = "Mark all read";
    /// The heading over how long to mute it.
    pub const MUTE: &str = "Mute";
    /// Turns its notifications back on.
    pub const UNMUTE: &str = "Unmute";
    /// How long to mute it: hours, or none for until it's turned back on.
    pub const MUTE_FOR: [(&str, Option<i64>); 3] = [
        ("For 1 hour", Some(1)),
        ("For 8 hours", Some(8)),
        ("Until I unmute", None),
    ];
    /// The heading over how much it notifies ([`super::Level::label`]).
    pub const NOTIFY_ME_ABOUT: &str = "Notify me about";
    /// Leaves a channel the person is in, asking first.
    pub const LEAVE: &str = "Leave";
    /// Closes a direct message until something new is said in it.
    pub const CLOSE: &str = "Close";
    /// Hides a suggestion, a row listed though the person isn't in it, until something new
    /// is said in it ([`super::Chat::is_suggestion`]).
    pub const HIDE: &str = "Hide";
}

/// How long a conversation is muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "until")]
pub enum Muted {
    /// Until this time, such as an hour or eight from when it was muted.
    Time {
        /// When it notifies again, in seconds since the Unix epoch.
        at: i64,
    },
    /// Until the person turns it back on.
    Off,
}

/// Every conversation's notification settings, as an app keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifySettings {
    /// Folded conversation name → its level, where it isn't the default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub levels: BTreeMap<String, Level>,
    /// Folded conversation name → how long it's muted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub muted: BTreeMap<String, Muted>,
}

impl NotifySettings {
    /// How much the conversation called `name` notifies.
    #[must_use]
    pub fn level(&self, name: &str) -> Level {
        self.levels.get(&fold(name)).copied().unwrap_or_default()
    }

    /// Whether the conversation called `name` is muted at `now`, in seconds since the Unix
    /// epoch: until a time still to come, or until turned back on.
    #[must_use]
    pub fn is_muted(&self, name: &str, now: i64) -> bool {
        match self.muted.get(&fold(name)) {
            Some(Muted::Off) => true,
            Some(Muted::Time { at }) => *at > now,
            None => false,
        }
    }

    /// Whether something someone else said in the conversation called `name` notifies at
    /// `now`, by its level and whether it's muted: `direct` for a direct message or a group,
    /// `aimed` when it names the person (or, for the pusher, answers them). A channel message
    /// aimed at them always does, even at [`Level::Nothing`] or muted, as one that names them
    /// is meant for them; a direct message keeps to its own level and mute.
    #[must_use]
    pub fn notifies(&self, name: &str, direct: bool, aimed: bool, now: i64) -> bool {
        let named = aimed && !direct;
        if self.is_muted(name, now) && !named {
            return false;
        }
        match self.level(name) {
            Level::All => true,
            Level::Mentions => direct || aimed,
            Level::Nothing => named,
        }
    }
}

impl Chat {
    /// How much the conversation called `name` notifies.
    #[must_use]
    pub fn level(&self, name: &str) -> Level {
        self.notify.level(name)
    }

    /// Sets how much the conversation called `name` notifies.
    pub fn set_level(&mut self, name: &str, level: Level) {
        if level == Level::default() {
            self.notify.levels.remove(&fold(name));
        } else {
            self.notify.levels.insert(fold(name), level);
        }
    }

    /// Mutes the conversation called `name` as `muted` says, or with `None` turns it back on:
    /// [`Chat::mute_for`] mutes it for a while.
    pub fn mute(&mut self, name: &str, muted: Option<Muted>) {
        match muted {
            Some(muted) => self.notify.muted.insert(fold(name), muted),
            None => self.notify.muted.remove(&fold(name)),
        };
    }

    /// Mutes the conversation called `name` for `hours` from now: 1 or 8, as the apps offer.
    pub fn mute_for(&mut self, name: &str, hours: i64) {
        let at = (self.clock)().timestamp() + hours * 3600;
        self.mute(name, Some(Muted::Time { at }));
    }

    /// Whether the conversation called `name` is muted now: until a time still to come, or
    /// until turned back on.
    #[must_use]
    pub fn is_muted(&self, name: &str) -> bool {
        self.notify.is_muted(name, (self.clock)().timestamp())
    }

    /// Whether `entry`, newly arrived in the conversation called `name`, deserves a
    /// notification: [`Chat::would_notify`], and never what's from history.
    #[must_use]
    pub fn notifies(&self, name: &str, entry: &Entry) -> bool {
        !entry.history && self.would_notify(name, entry)
    }

    /// Whether `entry` in the conversation called `name` is what a notification is for,
    /// whether it arrived just now or from history, by its level and whether it's muted:
    /// never what's from the person themselves or not said by anyone. A channel message that
    /// mentions the person always is, even at [`Level::Nothing`] or muted, as one that names
    /// them is meant for them; a direct message keeps to its own level and mute. Activity
    /// lists these ([`Chat::activity`]), so it holds what the phones push.
    #[must_use]
    pub fn would_notify(&self, name: &str, entry: &Entry) -> bool {
        self.would_notify_in(name, self.conversation(name), entry)
    }

    /// [`Chat::would_notify`], with the conversation called `name` already found.
    pub(super) fn would_notify_in(
        &self,
        name: &str,
        conversation: Option<&Conversation>,
        entry: &Entry,
    ) -> bool {
        let said = matches!(
            entry.kind,
            EntryKind::Message(_)
                | EntryKind::Action(_)
                | EntryKind::Notice(_)
                | EntryKind::Zirc(_)
        );
        let shown = conversation.is_none_or(|conversation| conversation.shows(entry));
        if !said || !shown || !entry.counts(&self.me) {
            return false;
        }
        let direct = !is_channel(name) || is_group(name);
        let named = entry.mention && !direct;
        if self.is_muted(name) && !named {
            return false;
        }
        if self.is_guest(&entry.from) {
            // Agents hear nothing from guests.
            if !self.is_staff() {
                return false;
            }
            if self.is_guest_conversation(name) || zirc_proto::guest_room_of(name).is_some() {
                return self.guest_notifies(name, entry);
            }
        }
        self.notify
            .notifies(name, direct, entry.mention, (self.clock)().timestamp())
    }

    /// The settings, for the app to keep.
    #[must_use]
    pub fn notify_settings(&self) -> &NotifySettings {
        &self.notify
    }

    /// The settings an app kept, in place of what's set now.
    pub fn set_notify_settings(&mut self, settings: NotifySettings) {
        self.notify = settings;
    }
}
