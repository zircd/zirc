//! Group direct messages. IRC has none, so a group is a channel nobody else can find or
//! join: secret and invite-only (`+s +i`), named `#dm-<random>` so it can't clash with a
//! team's channels, and registered with `ChanServ` by whoever makes it, who gives each
//! member a persistent voice (`AMODE +v`). Ergo lets an account with a persistent mode into
//! an invite-only channel, so a member's every app can join it, while it's never listed to
//! anyone else. (Ergo 2.19 has no account invite exceptions, `+I $a:<account>`: it takes
//! them as nick masks.) Members are invited, and the server keeps an invitation in the
//! private history of an always-on account, so their apps join when they next connect.

use std::hash::{BuildHasher as _, Hasher as _};

use zirc_line::Line;

use super::conversation::{fold, is_channel};

/// What every group's name starts with.
pub const GROUP_PREFIX: &str = "#dm-";

/// How many faces a group's avatar shows, the latest to speak first and drawn in front, over
/// the other's corner, in every app (the user's choice, 17 Nov 2026).
pub const GROUP_FACES: usize = 2;

/// How many random letters and digits follow [`GROUP_PREFIX`].
const RANDOM_LEN: usize = 10;

impl super::Chat {
    /// Who the private conversation `name` is with, by nick, for its avatar: the other
    /// person, an agent session's machine, or a group's other members. None for a channel.
    #[must_use]
    pub fn others(&self, name: &str) -> Vec<String> {
        if let Some((nick, _)) = super::conversation::agent(name) {
            return vec![nick.to_owned()];
        }
        if !is_channel(name) {
            return vec![name.to_owned()];
        }
        if !is_group(name) {
            return Vec::new();
        }
        let me = self.me();
        self.conversation(name).map_or_else(Vec::new, |group| {
            group
                .members
                .values()
                .filter(|member| !member.nick.eq_ignore_ascii_case(me))
                .map(|member| member.nick.clone())
                .collect()
        })
    }

    /// Whose faces the private conversation `name`'s avatar shows, by nick: the other
    /// person, an agent session's machine, or [`GROUP_FACES`] of a group's others, whoever
    /// spoke in it last first (drawn in front), then the rest by nick.
    #[must_use]
    pub fn faces_of(&self, name: &str) -> Vec<String> {
        let mut others = self.others(name);
        if let Some(group) = self.conversation(name).filter(|_| is_group(name)) {
            let mut spoke: Vec<&str> = Vec::new();
            for entry in group.entries.iter().rev().filter(|entry| entry.is_said()) {
                if !spoke
                    .iter()
                    .any(|nick| nick.eq_ignore_ascii_case(&entry.from))
                {
                    spoke.push(&entry.from);
                }
            }
            let rank = |nick: &str| {
                spoke
                    .iter()
                    .position(|said| said.eq_ignore_ascii_case(nick))
                    .unwrap_or(usize::MAX)
            };
            others.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| fold(a).cmp(&fold(b))));
        }
        others.truncate(GROUP_FACES);
        others
    }
}

/// Whether the channel called `name` is a group direct message.
#[must_use]
pub fn is_group(name: &str) -> bool {
    is_channel(name)
        && name
            .get(..GROUP_PREFIX.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(GROUP_PREFIX))
}

/// A new group's name, `#dm-` and random lowercase letters and digits, from `seed` and the
/// process's random hasher keys.
pub(super) fn new_name(seed: u64) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(seed);
    let mut bits = hasher.finish();
    let mut name = GROUP_PREFIX.to_owned();
    for _ in 0..RANDOM_LEN {
        let len = ALPHABET.len() as u64;
        name.push(char::from(
            ALPHABET[usize::try_from(bits % len).unwrap_or(0)],
        ));
        bits /= len;
        if bits == 0 {
            hasher.write_u64(seed);
            bits = hasher.finish();
        }
    }
    name
}

/// What the person who made the group `name` sends once they're in it: the modes that
/// hide it and keep others out, its registration, and each member let in and invited.
pub(super) fn set_up(name: &str, members: &[String]) -> Vec<Line> {
    let mut lines = vec![
        Line::new("MODE", [name, "+si"]),
        Line::new("PRIVMSG", ["ChanServ", &format!("REGISTER {name}")]),
    ];
    lines.extend(add(name, members));
    lines
}

/// What lets `members` into the group `name`, and invites them.
pub(super) fn add(name: &str, members: &[String]) -> Vec<Line> {
    members
        .iter()
        .flat_map(|member| {
            [
                Line::new(
                    "PRIVMSG",
                    ["ChanServ", &format!("AMODE {name} +v {member}")],
                ),
                Line::new("INVITE", [member.as_str(), name]),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_groups_apart_from_channels() {
        let name = new_name(1);
        assert!(is_group(&name), "{name}");
        assert_eq!(name.len(), GROUP_PREFIX.len() + RANDOM_LEN);
        assert_ne!(new_name(1), new_name(2));
        assert!(is_group("#DM-abc"));
        assert!(!is_group("#dmx"));
        assert!(!is_group("dm-bob"));
        assert!(!is_group("#api"));
    }
}
