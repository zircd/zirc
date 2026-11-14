//! Who is in a conversation, as every app names them: a person by the name they show as,
//! `Mattias Granlund` ([`super::Chat::shown_name`]), and a machine as its owner and its own
//! name, `mattias · mbp`, operators tagged [`OP_LABEL`]. A person's nick is said only where
//! there's room for both, as under the name on their card.

use super::conversation::{Conversation, Member, fold};
use super::list::{MachineRow, PersonRow};

/// What tags a channel operator, after their name, in the muted tag style where an app has
/// one: `ada op`. Never `@`, which reads as a mention.
pub const OP_LABEL: &str = "op";
/// The statuses that count as operator or higher.
const OPERATORS: &str = "~&@";

pub use zirc_proto::{GUEST_PREFIX, is_guest};

/// Someone in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// Their nick.
    pub nick: String,
    /// The name they show as, [`super::Chat::shown_name`]: their nick when they have no other.
    pub name: String,
    /// For a machine, whose it is and its own name: `("mattias", "mbp")` for `mattias-mbp`.
    pub machine: Option<(String, String)>,
    /// Whether they're a channel operator.
    pub op: bool,
    /// Whether it's us.
    pub me: bool,
    /// Why they're away, when they are: their away message, which may be empty. A person
    /// none of whose apps is connected is away.
    pub away: Option<String>,
    /// Whether they're a guest: see [`is_guest`].
    pub guest: bool,
    /// For one of zirc's integrations, its short name, shown in place of a generic app
    /// badge: `alerts` for `zirc-alerts`. See [`super::app_label`].
    pub app: Option<String>,
}

impl Person {
    /// How they stand, being here in the conversation: away or here ([`super::Standing`]).
    #[must_use]
    pub fn standing(&self) -> super::Standing {
        super::Standing::of(true, self.away.as_deref())
    }

    /// Their name as shown, an operator's tagged after it: `Mattias Granlund`, `mattias · mbp`,
    /// `Mattias Granlund op`.
    #[must_use]
    pub fn label(&self) -> String {
        let name = match &self.machine {
            Some((owner, machine)) => machine_label(owner, machine),
            None => self.name.clone(),
        };
        if self.op {
            format!("{name} {OP_LABEL}")
        } else {
            name
        }
    }
}

/// `people`, as [`people`] lists them, one row per person with their machines under them.
/// An owner who isn't here gets a row of their own, away as `away` says.
pub(super) fn rows(
    people: Vec<Person>,
    me: &str,
    away: impl Fn(&str) -> Option<String>,
    shown: impl Fn(&str) -> String,
) -> Vec<PersonRow> {
    let mut rows: Vec<PersonRow> = Vec::new();
    for person in people {
        let owner = person
            .machine
            .as_ref()
            .map_or(person.nick.clone(), |(owner, _)| owner.clone());
        let at = rows
            .iter()
            .position(|row| row.person.nick.eq_ignore_ascii_case(&owner));
        match (person.machine.is_some(), at) {
            (false, Some(at)) => {
                rows[at].person = person;
                rows[at].here = true;
            }
            (false, None) => rows.push(PersonRow {
                person,
                here: true,
                machines: Vec::new(),
            }),
            (true, at) => {
                let at = at.unwrap_or_else(|| {
                    rows.push(PersonRow {
                        person: Person {
                            me: owner.eq_ignore_ascii_case(me),
                            away: away(&owner),
                            name: shown(&owner),
                            nick: owner,
                            machine: None,
                            op: false,
                            guest: false,
                            app: None,
                        },
                        here: false,
                        machines: Vec::new(),
                    });
                    rows.len() - 1
                });
                rows[at].machines.push(MachineRow {
                    person,
                    sessions: Vec::new(),
                });
            }
        }
    }
    rows
}

/// A machine as people read it, by its owner and its own name: `mattias · mbp`. Every
/// label of a machine is this one.
#[must_use]
pub fn machine_label(owner: &str, machine: &str) -> String {
    format!("{owner} · {machine}")
}

/// What separates a machine's owner from the machine it runs on in its real name:
/// `mattias on ci-1`, a CI job's host on the warm agent's.
const RUNS_ON: &str = " on ";

/// The machine one of `owner`'s machines runs on, when its real name says
/// (`mattias on ci-1`): that machine's name, `ci-1`. A CI job's host runs on its warm agent's
/// host, and the sidebar draws it under it. For grouping rows only: anyone may say anything
/// in a real name, so it counts only for the owner its host says (`owner`).
#[must_use]
pub fn runs_on(owner: Option<&str>, realname: Option<&str>) -> Option<String> {
    let (said, host) = realname?.trim().split_once(RUNS_ON)?;
    let host = host.trim();
    (owner?.eq_ignore_ascii_case(said.trim())
        && !host.is_empty()
        && !host.contains(char::is_whitespace))
    .then(|| host.to_owned())
}

/// The people in `conversation`: operators first, then by name, each machine after its
/// owner. A machine is one whose host says whose it is (`owner`, `machine/<owner>`, which only
/// the server sets), and goes by its nick; a guest is no one's machine. Who's away is from
/// `away`.
pub(super) fn people(
    conversation: &Conversation,
    me: &str,
    owner: impl Fn(&str) -> Option<String>,
    away: impl Fn(&str) -> Option<String>,
    guest: impl Fn(&str) -> bool,
    shown: impl Fn(&str) -> String,
    app: impl Fn(&str) -> Option<String>,
) -> Vec<Person> {
    let mut people: Vec<Person> = conversation
        .members
        .values()
        .map(|Member { nick, prefix }| {
            let guest = guest(nick);
            let machine = owner(nick)
                .filter(|_| !guest)
                .map(|owner| (owner, nick.clone()));
            Person {
                nick: nick.clone(),
                name: shown(nick),
                machine,
                op: prefix.is_some_and(|p| OPERATORS.contains(p)),
                me: nick.eq_ignore_ascii_case(me),
                away: away(nick),
                guest,
                app: app(nick),
            }
        })
        .collect();
    people.sort_by_key(|person| {
        let (owner, name) = person
            .machine
            .clone()
            .unwrap_or_else(|| (person.nick.clone(), String::new()));
        (!person.op, fold(&owner), fold(&name))
    });
    people
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Kind;

    #[test]
    fn a_real_name_may_say_which_machine_a_machine_runs_on() {
        let on = Some("mattias on ci-1");
        assert_eq!(runs_on(Some("mattias"), on), Some("ci-1".into()));
        assert_eq!(runs_on(Some("mattias"), Some("mattias")), None);
        // Only for the owner its host says, whatever its real name claims.
        assert_eq!(runs_on(Some("bob"), on), None);
        assert_eq!(runs_on(None, on), None);
        assert_eq!(runs_on(Some("mattias"), Some("mattias on ")), None);
    }

    #[test]
    fn names_people_and_machines_the_same_way_everywhere() {
        let mut general = Conversation::new("#general", Kind::Channel);
        for nick in ["m3-mac", "@mattias", "bob", "laptop", "alice-mbp"] {
            general.add_member(nick);
        }
        // Whose machines are, as their hosts say: never by their names.
        let owner = |nick: &str| match nick {
            "m3-mac" => Some("mattias".to_owned()),
            "laptop" => Some("carol".to_owned()),
            _ => None,
        };
        // Someone who shows as another name is listed by it; a machine by its owner's nick.
        let name = |nick: &str| match nick {
            "bob" => "Bob Dylan".to_owned(),
            _ => nick.to_owned(),
        };
        let shown: Vec<String> = people(
            &general,
            "mattias",
            owner,
            |_| None,
            |_| false,
            name,
            |nick| crate::chat::app_label(nick, None),
        )
        .iter()
        .map(Person::label)
        .collect();
        // alice-mbp's host says no owner: just a nick, whatever its name looks like.
        assert_eq!(
            shown,
            [
                "mattias op",
                "alice-mbp",
                "Bob Dylan",
                "carol · laptop",
                "mattias · m3-mac"
            ]
        );
    }
}
