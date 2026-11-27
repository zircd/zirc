//! One row in Messages for a role and the session holding it (the user's choices, 26 Nov
//! 2026, after an iPhone showed two "oncall" rows, the role's and its holder's, the
//! holder's saying "No messages yet" over lines). The holder's own conversation folds into
//! the role's row: writing there reaches whoever holds it, and the holder's wake lines and
//! what it and we said show there. A session holding no role keeps its own row.
//!
//! A row previews the last real message, never a wake's line; one with only wake lines
//! says how many ([`zirc_proto::woke::wakes`]), never "No messages yet".

use chrono::{DateTime, Utc};

use super::Chat;
use super::conversation::{Entry, EntryKind, Kind, agent, agent_channel, fold};
use super::woke::{outcome_of, woke_of};

impl Chat {
    /// The role whose row the conversation called `name` folds into: the role the agent
    /// session it's with holds. `None` for any other conversation.
    #[must_use]
    pub fn folds_into_role(&self, name: &str) -> Option<String> {
        let (nick, session) = agent(name)?;
        self.role_held(nick, session)
    }

    /// The conversation we have with the session that holds the role `role` now, by name,
    /// `henrik-mbp/cc-2`, whether or not anything's been said in it.
    #[must_use]
    pub fn holder_conversation(&self, role: &str) -> Option<String> {
        let holder = self.role_seat(role)?.holder?;
        Some(agent_channel(&holder.machine, &holder.session.session))
    }

    /// The wake lines the role `role`'s conversation shows, from every room, in their
    /// rooms' order: those it was woken for, and its holder's own since it took the role,
    /// and what the role's nick said of what it woke for ([`zirc_proto::incident::Outcome`]);
    /// never one a room's own message caused, which is that room's alone
    /// ([`zirc_proto::woke::Woke::in_its_chat`]).
    pub(super) fn role_wakes(&self, role: &str) -> Vec<&Entry> {
        let role = fold(role);
        let holder = self.role_seat(&role).and_then(|seat| seat.holder);
        let holders = |entry: &Entry| {
            let EntryKind::Zirc(message) = &entry.kind else {
                return false;
            };
            holder.as_ref().is_some_and(|holder| {
                entry.from.eq_ignore_ascii_case(&holder.machine)
                    && message.session == holder.session.session
                    && u64::try_from(entry.time.timestamp()).unwrap_or(0) >= holder.since
            })
        };
        self.conversations
            .iter()
            .filter(|room| room.kind == Kind::Channel)
            .flat_map(|room| room.entries.iter().map(move |entry| (room, entry)))
            .filter(|(room, entry)| {
                woke_of(entry).is_some_and(|woke| {
                    !woke.in_its_chat(&room.name)
                        && (woke.role.as_deref().is_some_and(|of| fold(of) == role)
                            || holders(entry))
                }) || outcome_of(entry).is_some_and(|(outcome, _)| {
                    fold(&outcome.role) == role && fold(&entry.from) == role
                })
            })
            .map(|(_, entry)| entry)
            .collect()
    }

    /// How many wake lines the conversation called `name` shows and when the newest was:
    /// a role's ([`Chat::role_wakes`]) or an agent session's own. Nothing for any other.
    pub(super) fn wakes_shown(&self, name: &str) -> (usize, Option<DateTime<Utc>>) {
        let wakes: Vec<&Entry> = if self.is_role(name) {
            self.role_wakes(name)
                .into_iter()
                .filter(|entry| woke_of(entry).is_some())
                .collect()
        } else if let Some((nick, session)) = agent(name) {
            self.conversations
                .iter()
                .filter(|room| room.kind == Kind::Channel && room.joined)
                .flat_map(|room| &room.entries)
                .filter(|entry| {
                    woke_of(entry).is_some()
                        && entry.from.eq_ignore_ascii_case(nick)
                        && matches!(&entry.kind, EntryKind::Zirc(message) if message.session == session)
                })
                .collect()
        } else {
            Vec::new()
        };
        let newest = wakes.iter().map(|entry| entry.time).max();
        (wakes.len(), newest)
    }

    /// The names whose words the row of the conversation `name` previews and counts: a
    /// role's and its holder's conversation, else its own alone.
    pub(super) fn row_names(&self, name: &str) -> Vec<String> {
        let mut names = vec![name.to_owned()];
        if self.is_role(name)
            && let Some(holder) = self.holder_conversation(name)
        {
            names.push(holder);
        }
        names
    }
}
