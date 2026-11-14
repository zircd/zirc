//! How things stand ([`zirc_proto::state`]), read from the states said in `#ops`: the
//! newest of each repository's name, and who said it when.

use std::collections::BTreeMap;

use zirc_proto::state::{Level, State};

use crate::ipc::Delivery;

/// Where states go and are read: the organisation's `#ops`, the incident room, where a broken
/// master is news to whoever watches it.
pub const CHANNEL: &str = zirc_proto::state::CHANNEL;

/// How many of `#ops`'s newest messages are read for states unless said: zircd keeps
/// these, to answer such reads at once.
pub const LOOK_BACK: usize = 1000;

/// A state and the message that said it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// How it stands.
    pub state: State,
    /// Who said it: the session, person or script.
    pub by: String,
    /// When, as the server says.
    pub time: Option<String>,
}

/// The newest state of each repository's name among `deliveries`, oldest first as history
/// comes: one per (repository, name), sorted by them. Those `ok` are kept, so a reader can
/// tell a state that's fine again from one never said.
#[must_use]
pub fn current<'a>(deliveries: impl IntoIterator<Item = &'a Delivery>) -> Vec<Said> {
    let mut newest: BTreeMap<(String, String), Said> = BTreeMap::new();
    for delivery in deliveries {
        let Some(state) = State::of(&delivery.message.event) else {
            continue;
        };
        let said = Said {
            by: if delivery.message.session.is_empty() {
                delivery.from.clone()
            } else {
                delivery.message.session.clone()
            },
            time: delivery.time.clone(),
            state,
        };
        newest.insert((said.state.repo.clone(), said.state.name.clone()), said);
    }
    newest.into_values().collect()
}

/// Those of `states` that aren't fine.
#[must_use]
pub fn troubled(states: &[Said]) -> Vec<&Said> {
    states
        .iter()
        .filter(|said| said.state.level != Level::Ok)
        .collect()
}

/// What a reader of a state that isn't fine does to wait for it, on one line under it: run
/// `zirc wait state`, which ends once it's fine, rather than poll `zirc state --check` in a
/// loop.
#[must_use]
pub fn wait_hint(name: &str) -> String {
    format!("to be woken when it's fine: zirc wait state {name} ok, in the background")
}

#[cfg(test)]
mod tests {
    use zirc_proto::{Event, Message};

    use super::*;

    fn delivery(session: &str, time: &str, state: Option<State>) -> Delivery {
        let event = state.map_or_else(
            || Event::Text {
                body: "unrelated".into(),
            },
            |state| state.event(),
        );
        Delivery {
            time: Some(time.into()),
            ..Delivery::new(CHANNEL, "mattias-mbp", Message::new(session, event))
        }
    }

    fn state(repo: &str, name: &str, level: Level) -> State {
        State {
            repo: repo.into(),
            name: name.into(),
            level,
            why: None,
            owner: None,
            link: None,
        }
    }

    #[test]
    fn the_newest_of_each_name_stands() {
        let history = [
            delivery("ci", "t1", Some(state("#api", "master", Level::Broken))),
            delivery("cc-1", "t2", None),
            delivery("ci", "t3", Some(state("#web", "master", Level::Warning))),
            delivery("cc-2", "t4", Some(state("#api", "master", Level::Ok))),
            delivery("ci", "t5", Some(state("#api", "staging", Level::Broken))),
        ];
        let now = current(&history);
        let seen: Vec<_> = now
            .iter()
            .map(|said| {
                (
                    said.state.repo.as_str(),
                    said.state.name.as_str(),
                    said.state.level,
                    said.by.as_str(),
                    said.time.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("#api", "master", Level::Ok, "cc-2", Some("t4")),
                ("#api", "staging", Level::Broken, "ci", Some("t5")),
                ("#web", "master", Level::Warning, "ci", Some("t3")),
            ]
        );
        let names: Vec<_> = troubled(&now)
            .iter()
            .map(|said| said.state.name.as_str())
            .collect();
        assert_eq!(names, ["staging", "master"]);
    }

    #[test]
    fn waiting_on_a_state_is_a_command_to_run() {
        assert_eq!(
            wait_hint("master"),
            "to be woken when it's fine: zirc wait state master ok, in the background"
        );
    }
}
