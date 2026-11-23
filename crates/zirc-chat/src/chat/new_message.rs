//! New message: a To: field that takes people as chips from what's typed, then opens a
//! direct message with one of them or makes a group of several, decided once for every app
//! (the user's choice, 17 Nov 2026: the GPUI desktop's picker everywhere).

use zirc_line::Line;

use super::conversation::Kind;
use super::{Avatar, Chat, Face, in_label, is_group, switch_rank};

/// How many people New message offers at once for what's typed.
pub const PEOPLE_OFFERED: usize = 8;

/// The To: field's label.
pub const TO: &str = "To";

/// What the To: field says while it's empty.
pub const TYPE_A_NAME: &str = "Type a name";

/// Someone New message can take.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Pickable {
    /// Their nick, what a chip and the conversation go by.
    pub nick: String,
    /// What shows: their name.
    pub name: String,
    /// Beside it, quieter, for someone we've no direct message with yet: where they are,
    /// `in #api`.
    pub note: Option<String>,
    /// Their face.
    pub face: Face,
}

/// What starting a new message came to ([`Chat::start_message`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The direct message to open, with one person; none for a group, which opens once the
    /// server says it's made.
    pub open: Option<String>,
    /// What to send: a group's making.
    pub lines: Vec<Line>,
}

/// Of `people`, who matches `query` and isn't in `chosen` yet, best first as the quick
/// switcher ranks ([`switch_rank`]), at most [`PEOPLE_OFFERED`]; nobody until something's
/// typed.
#[must_use]
pub fn pick_people(people: &[Pickable], query: &str, chosen: &[String]) -> Vec<Pickable> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut ranked: Vec<(u8, usize)> = people
        .iter()
        .enumerate()
        .filter(|(_, person)| {
            !chosen
                .iter()
                .any(|nick| nick.eq_ignore_ascii_case(&person.nick))
        })
        .filter_map(|(at, person)| {
            let rank = switch_rank(query, &person.name, Some(&person.nick))?;
            Some((rank, at))
        })
        .collect();
    ranked.sort_by_key(|&(rank, at)| (rank, at));
    ranked
        .into_iter()
        .take(PEOPLE_OFFERED)
        .map(|(_, at)| people[at].clone())
        .collect()
}

impl Chat {
    /// Who New message can take, in the order it offers them: the people we have direct
    /// messages with, as the list orders them, then everyone else in the channels we're in,
    /// each saying where. Never us, a machine or an app.
    #[must_use]
    pub fn pickable_people(&self) -> Vec<Pickable> {
        let mut people: Vec<Pickable> = Vec::new();
        let mut add = |chat: &Self, nick: &str, note: Option<String>| {
            if nick.eq_ignore_ascii_case(chat.me())
                || chat.is_machine(nick)
                || people.iter().any(|p| p.nick.eq_ignore_ascii_case(nick))
            {
                return;
            }
            people.push(Pickable {
                nick: nick.to_owned(),
                name: chat.title(nick),
                note,
                face: chat.face(&Avatar::Person {
                    nick: nick.to_owned(),
                }),
            });
        };
        for row in self.direct_rows() {
            if row.kind == super::DirectKind::Person {
                add(self, &row.name, None);
            }
        }
        for channel in self
            .conversations
            .iter()
            .filter(|c| c.kind == Kind::Channel && c.joined && !is_group(&c.name))
        {
            for row in self.people_rows(&channel.name) {
                let person = &row.person;
                if row.here && person.app.is_none() {
                    add(self, &person.nick, Some(in_label(&channel.name)));
                }
            }
        }
        people
    }

    /// Who New message offers for `query`, those in `chosen` aside ([`pick_people`]).
    #[must_use]
    pub fn pick_people(&self, query: &str, chosen: &[String]) -> Vec<Pickable> {
        pick_people(&self.pickable_people(), query, chosen)
    }

    /// Starts a new message with `chosen`, by nick: the direct message with one person, or
    /// a new group of several ([`Chat::create_group`]). Nothing with nobody chosen.
    pub fn start_message(&mut self, chosen: &[String]) -> Started {
        match chosen {
            [] => Started {
                open: None,
                lines: Vec::new(),
            },
            [one] => Started {
                open: Some(one.clone()),
                lines: Vec::new(),
            },
            several => Started {
                open: None,
                lines: self.create_group(several),
            },
        }
    }
}
