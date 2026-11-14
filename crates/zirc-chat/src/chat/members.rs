//! The people list as every app draws it: a person's row with their machines, and the agent
//! sessions on those, ready to draw and send as they are. The phones read these records
//! through `UniFFI` and web as JSON; [`Chat::people_rows`] says
//! who's there and in what order.

use serde::Serialize;
use ts_rs::TS;

use super::list::{Agent, MachineRow, PersonRow};
use super::{Avatar, BothChangingLine, Chat, Face, Presence, YOU};

/// A person in a conversation, with their machines there under them.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a fact about them, independent of the others, as an app draws it"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MemberRow {
    /// Their nick, to open their card or a conversation with, and which mentions use.
    pub nick: String,
    /// The row's line: `alice`, `alice op · 2 agents` ([`PersonRow::label`]).
    pub label: String,
    /// What follows their name on their row, muted after it in every app: `op · 2 agents`,
    /// `· 1 agent`; none when the line is their name alone.
    pub detail: Option<String>,
    /// What their row says at its end: `you` for us, else that they're away or offline
    /// ([`MemberRow::away_label`]).
    pub aside: Option<String>,
    /// Whether it's us.
    pub me: bool,
    /// Whether they're here themselves, not only their machines.
    pub here: bool,
    /// Why they're away, when they are; may be empty.
    pub away: Option<String>,
    /// That away, said after their name: `away`, `away: lunch` ([`super::away_label`]);
    /// none when they aren't.
    pub away_label: Option<String>,
    /// Whether they're a guest, let into a public channel without an account.
    pub guest: bool,
    /// For one of zirc's integrations, its short name, `ci`, shown in capitals.
    pub app: Option<String>,
    /// Their machines here, by name, each with its agent sessions.
    pub machines: Vec<MemberMachine>,
    /// Their dot: gone when only their machines are here ([`super::Standing`]).
    pub presence: Presence,
    /// The pill after their name: `APP` for one of zirc's apps, `GUEST` for a guest
    /// ([`PersonRow::pill`]).
    pub pill: Option<String>,
    /// Their name as shown: `mattias`.
    pub name: String,
    /// Whether they're a channel operator.
    pub op: bool,
    /// Their face: their photo, or their initials on their colour. Always there; optional
    /// only so the phones' fakes may leave it out.
    pub face: Option<Face>,
}

/// One of a person's machines under their [`MemberRow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MemberMachine {
    /// Its nick, `alice-mbp`.
    pub nick: String,
    /// Its own name, `mbp` ([`MachineRow::name`]).
    pub name: String,
    /// Why it's away, when it is; may be empty.
    pub away: Option<String>,
    /// Its agent sessions at work.
    pub sessions: Vec<MemberSession>,
    /// Its dot.
    pub presence: Presence,
    /// That away, said after its name ([`super::away_label`]); none when it isn't.
    pub away_label: Option<String>,
}

/// An agent session under its machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MemberSession {
    /// Its conversation, `alice-mbp/cc-1531f8`, to open.
    pub conversation: String,
    /// Its short name, `cc-1531f8`.
    pub short: String,
    /// What it said it's taking on, if it did.
    pub task: Option<String>,
    /// What it goes by, `zirc-chat`, or else its id, first on its row.
    pub label: String,
    /// Its avatar: its tool on its colour, its owner's photo small. Always there; optional
    /// only so the phones' fakes may leave it out.
    pub face: Option<Face>,
    /// The machine it's on, by nick, whose card on it a click opens.
    pub machine: String,
    /// What it's changing that others are too, its row's caption, in warning
    /// ([`Chat::session_both`]): `! also changing client.ts with checkout`.
    pub both: Option<BothChangingLine>,
}

impl Chat {
    /// Who is in the conversation called `name`, one row per person, in the people list's
    /// order ([`Chat::people_rows`]).
    #[must_use]
    pub fn member_rows(&self, name: &str) -> Vec<MemberRow> {
        self.people_rows(name)
            .into_iter()
            .map(|row| self.member_row(row))
            .collect()
    }

    /// `nick`'s machines and the agent sessions on them, as their card lists them under
    /// Machines in every app: as the people list has them in the first channel we share.
    #[must_use]
    pub fn card_machines(&self, nick: &str) -> Vec<MemberMachine> {
        self.profile(nick)
            .channels
            .iter()
            .find_map(|channel| {
                self.member_rows(channel)
                    .into_iter()
                    .find(|member| member.nick.eq_ignore_ascii_case(nick))
            })
            .map(|member| member.machines)
            .unwrap_or_default()
    }

    /// `row` as the people list draws it, with its machines and their sessions.
    #[must_use]
    pub fn member_row(&self, row: PersonRow) -> MemberRow {
        let label = row.label();
        let presence = row.standing().presence();
        let away_label = row.away_label();
        let pill = row.pill();
        let person = row.person;
        let detail = label
            .strip_prefix(person.name.as_str())
            .map(str::trim)
            .filter(|rest| !rest.is_empty())
            .map(str::to_owned);
        let aside = if person.me {
            Some(YOU.to_owned())
        } else {
            away_label.clone()
        };
        MemberRow {
            detail,
            aside,
            face: Some(self.face(&Avatar::Person {
                nick: person.nick.clone(),
            })),
            machines: row
                .machines
                .into_iter()
                .map(|machine| self.member_machine(machine))
                .collect(),
            label,
            presence,
            away_label,
            pill,
            here: row.here,
            name: person.name,
            nick: person.nick,
            me: person.me,
            op: person.op,
            away: person.away,
            guest: person.guest,
            app: person.app,
        }
    }

    fn member_machine(&self, machine: MachineRow) -> MemberMachine {
        MemberMachine {
            name: machine.name(),
            presence: machine.presence(),
            away_label: machine.away_label(),
            sessions: machine
                .sessions
                .into_iter()
                .map(|agent| self.member_session(agent))
                .collect(),
            nick: machine.person.nick,
            away: machine.person.away,
        }
    }

    fn member_session(&self, agent: Agent) -> MemberSession {
        MemberSession {
            both: self.session_both(&agent).map(BothChangingLine::from),
            face: Some(self.face(&agent.avatar)),
            conversation: agent.name,
            short: agent.short,
            task: agent.task,
            label: agent.label,
            machine: agent.machine,
        }
    }
}
