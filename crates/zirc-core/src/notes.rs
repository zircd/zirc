//! Notes: a few words from one agent session for another, wherever it works
//! ([`zirc_proto::Event::Note`]). `zirc note <to> <text>` sends one, and so does a `zirc post`
//! addressed to one agent session (`coordinator: landed it`), which nobody else need read.
//! zircd finds the machine the session works on from what it's heard of it ([`Known`]), and
//! sends the note to that machine's nick, whose watch tells that session alone.

use std::fmt;

use crate::team;

/// A session zircd has heard of: one working on this machine, or one another machine's
/// working sets and claims name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    /// The nick of the machine it works on.
    pub machine: String,
    /// Its id, such as `cc-272fe8`.
    pub session: String,
    /// What it's called, such as `coordinator`, if it says.
    pub name: Option<String>,
}

/// Where a note goes: one session on one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// The machine's nick.
    pub machine: String,
    /// The session, as the note names it: its id, or the name it goes by.
    pub session: String,
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.machine, self.session)
    }
}

/// Why a note can't go where it was told to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// It names nothing that could be a session.
    #[error("{0:?} names no session: name one as <session>, its name, or <machine>/<session>")]
    Unnamed(String),
    /// No session of that id or name has been heard of.
    #[error(
        "no session called {0} is known here: name it as <machine>/<session>, as zirc \
         sessions shows it"
    )]
    Unknown(String),
    /// Several sessions go by it.
    #[error("{0} could be any of {1}: name one as <machine>/<session>")]
    Ambiguous(String, String),
}

/// The sessions among `known` that `to` names, once each: by id or name, or as
/// `<machine>/<id or name>`.
fn matching<'a>(to: &str, known: &'a [Known]) -> Vec<&'a Known> {
    let (machine, session) = match to.split_once('/') {
        Some((machine, session)) => (Some(machine), session),
        None => (None, to),
    };
    let mut found: Vec<&Known> = Vec::new();
    for it in known.iter().filter(|it| {
        machine.is_none_or(|machine| it.machine.eq_ignore_ascii_case(machine))
            && (it.session.eq_ignore_ascii_case(session)
                || it
                    .name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(session)))
    }) {
        if !found.iter().any(|seen| {
            seen.machine.eq_ignore_ascii_case(&it.machine) && seen.session == it.session
        }) {
            found.push(it);
        }
    }
    found
}

/// Where a note `to` names goes, given the sessions `found` it matches: the one, or the
/// machine they're all on, by the name they share, which that machine tells apart itself, as
/// one of them may be a session that's ended.
fn one(to: &str, found: &[&Known]) -> Option<Address> {
    let first = found.first()?;
    if let [only] = found {
        return Some(Address {
            machine: only.machine.clone(),
            session: only.session.clone(),
        });
    }
    found
        .iter()
        .all(|it| it.machine.eq_ignore_ascii_case(&first.machine))
        .then(|| Address {
            machine: first.machine.clone(),
            session: to.rsplit('/').next().unwrap_or(to).to_owned(),
        })
}

/// The one session `to` names among `known`: `<session>`, the name it goes by, or
/// `<machine>/<session>`. One named with its machine goes there even when it's not been
/// heard of, as the machine knows its own sessions.
///
/// # Errors
///
/// When `to` names no session, none known, or several.
pub fn resolve(to: &str, known: &[Known]) -> Result<Address, Error> {
    let to = to.trim().trim_start_matches('@');
    let unnamed = || Error::Unnamed(to.to_owned());
    if to.is_empty() || to.chars().any(char::is_whitespace) {
        return Err(unnamed());
    }
    if let Some((machine, session)) = to.split_once('/')
        && (machine.is_empty() || session.is_empty() || session.contains('/'))
    {
        return Err(unnamed());
    }
    let found = matching(to, known);
    if let Some(address) = one(to, &found) {
        return Ok(address);
    }
    match found.as_slice() {
        [] => match to.split_once('/') {
            Some((machine, session)) => Ok(Address {
                machine: machine.to_owned(),
                session: session.to_owned(),
            }),
            None => Err(Error::Unknown(to.to_owned())),
        },
        several => Err(Error::Ambiguous(
            to.to_owned(),
            several
                .iter()
                .map(|it| format!("{}/{}", it.machine, it.session))
                .collect::<Vec<_>>()
                .join(", "),
        )),
    }
}

/// Whether a post saying `text` is addressed to one agent session among `known`, as
/// `coordinator: landed it`, and so is a note for it: where to, and what it says without
/// the address. Never a status line ([`team::STATUS`]), which is for every session, nor one
/// to `@everyone` or one of `people`, nor one whose address could be several sessions.
#[must_use]
pub fn addressed<'a>(
    text: &'a str,
    people: &[&str],
    known: &[Known],
) -> Option<(Address, &'a str)> {
    let text = text.trim();
    if team::is_status(text) {
        return None;
    }
    let (to, rest) = text.split_once(": ")?;
    let to = to.trim_start_matches('@');
    let rest = rest.trim();
    if rest.is_empty()
        || to.is_empty()
        || to.chars().any(char::is_whitespace)
        || to.eq_ignore_ascii_case("everyone")
        || people.iter().any(|person| person.eq_ignore_ascii_case(to))
    {
        return None;
    }
    one(to, &matching(to, known)).map(|address| (address, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<Known> {
        let known = |machine: &str, session: &str, name: Option<&str>| Known {
            machine: machine.into(),
            session: session.into(),
            name: name.map(Into::into),
        };
        vec![
            known("mattias-mini", "cc-272fe8", Some("coordinator")),
            known("mattias-mini", "cc-272fe8", None),
            known("mattias-mbp", "cc-1a2b3c", Some("web")),
            known("ada-mbp", "cc-9f9f9f", Some("web")),
            known("ada-mbp", "cc-4d4d4d", None),
            known("bob-mini", "cc-5e5e5e", Some("reviewer")),
            known("bob-mini", "cc-6f6f6f", Some("reviewer")),
        ]
    }

    #[test]
    fn a_note_goes_to_the_one_session_it_names() {
        let known = known();
        let to = |name: &str| resolve(name, &known).map(|address| address.to_string());
        assert_eq!(to("cc-272fe8").unwrap(), "mattias-mini/cc-272fe8");
        assert_eq!(to("coordinator").unwrap(), "mattias-mini/cc-272fe8");
        assert_eq!(to("@Coordinator").unwrap(), "mattias-mini/cc-272fe8");
        assert_eq!(to("ada-mbp/web").unwrap(), "ada-mbp/cc-9f9f9f");
        // One named with its machine goes there, heard of or not.
        assert_eq!(to("carol-mini/cc-0").unwrap(), "carol-mini/cc-0");
        // Two of one machine's sessions by one name: that machine tells which is about.
        assert_eq!(to("reviewer").unwrap(), "bob-mini/reviewer");
        assert_eq!(
            to("web").unwrap_err().to_string(),
            "web could be any of mattias-mbp/cc-1a2b3c, ada-mbp/cc-9f9f9f: name one as \
             <machine>/<session>"
        );
        assert_eq!(resolve("cc-0", &known), Err(Error::Unknown("cc-0".into())));
        for unnamed in ["", "/cc-1", "ada-mbp/", "a b", "a/b/c"] {
            assert_eq!(
                resolve(unnamed, &known),
                Err(Error::Unnamed(unnamed.into())),
                "{unnamed:?}"
            );
        }
    }

    #[test]
    fn a_post_to_one_agent_session_is_a_note_for_it() {
        let known = known();
        let people = ["mattias", "ada"];
        let note = |text: &str| {
            addressed(text, &people, &known).map(|(to, rest)| (to.to_string(), rest.to_owned()))
        };
        assert_eq!(
            note("coordinator: landed the login page as 1a2b3c"),
            Some((
                "mattias-mini/cc-272fe8".into(),
                "landed the login page as 1a2b3c".into()
            ))
        );
        assert_eq!(
            note("reviewer: have a look at 1a2b3c"),
            Some(("bob-mini/reviewer".into(), "have a look at 1a2b3c".into()))
        );
        assert_eq!(
            note("@cc-4d4d4d: rebase onto master"),
            Some(("ada-mbp/cc-4d4d4d".into(), "rebase onto master".into()))
        );
        for text in [
            "done: coordinator: landed it",
            "taking: the login page",
            "@everyone: master is frozen",
            "mattias: shall I publish?",
            "web: two sessions go by it",
            "bob: nobody's heard of bob",
            "heads-up: master moved",
            "coordinator:",
            "coordinator: ",
            "the coordinator: said so",
            "is anyone in daemon.rs?",
        ] {
            assert_eq!(note(text), None, "{text}");
        }
    }
}
