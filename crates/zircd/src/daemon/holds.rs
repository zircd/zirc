//! States that end by themselves, such as a hold on landing: `zirc state set landings broken
//! --until 15:00` ([`zirc_proto::state::State::until`]). The machine that said one says it's
//! fine again once its time has passed, in `#ops` as any state is, so every reader, app,
//! role and waiting session hears it end; unless a newer state of that name was said
//! first. A machine that's off by then, or restarted since, says nothing: readers take a
//! state whose time has passed as fine all the same ([`zirc_core::states::in_force`]).

use std::collections::BTreeMap;

use zirc_proto::state::{Level, State};

use super::Daemon;

/// The states this machine said that end by themselves, by repository channel and name: when,
/// in seconds since the Unix epoch, who said it, and the state.
#[derive(Debug, Default)]
pub(super) struct Holds(BTreeMap<(String, String), (u64, String, State)>);

impl Holds {
    /// Notes `state`, which `session` said here, if it ends by itself.
    pub(super) fn said(&mut self, session: &str, state: &State) {
        let key = (state.repo.clone(), state.name.clone());
        match state
            .until
            .as_deref()
            .and_then(zirc_core::time::seconds_at)
            .filter(|_| state.level != Level::Ok)
        {
            Some(until) => {
                self.0
                    .insert(key, (until, session.to_owned(), state.clone()));
            }
            None => {
                self.0.remove(&key);
            }
        }
    }

    /// Notes `state`, heard in `#ops`: any but the one kept, such as its own echo, ends it.
    pub(super) fn heard(&mut self, state: &State) {
        let key = (state.repo.clone(), state.name.clone());
        if self.0.get(&key).is_some_and(|(_, _, kept)| kept != state) {
            self.0.remove(&key);
        }
    }

    /// Those whose time has passed by `now`, each with who said it, as the state that ends it.
    pub(super) fn due(&mut self, now: u64) -> Vec<(String, State)> {
        let due: Vec<_> = self
            .0
            .iter()
            .filter(|(_, (until, ..))| *until <= now)
            .map(|(key, _)| key.clone())
            .collect();
        due.into_iter()
            .filter_map(|key| self.0.remove(&key))
            .map(|(_, session, held)| (session, held.over()))
            .collect()
    }
}

impl Daemon {
    /// Says each state this machine said whose time has passed is fine again.
    pub(super) fn holds_tick(&mut self) {
        for (session, ended) in self.holds.due(super::now()) {
            let ops = zirc_core::states::CHANNEL;
            if self.joined.contains(ops) {
                self.publish(ops, &session, ended.event());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(until: Option<&str>) -> State {
        State {
            repo: "#monorepo".into(),
            name: "landings".into(),
            level: Level::Broken,
            why: Some("the database moves".into()),
            owner: None,
            link: None,
            until: until.map(str::to_owned),
        }
    }

    #[test]
    fn a_hold_ends_once_its_time_has_passed_and_says_so_once() {
        let at = zirc_core::time::seconds_at("2026-11-14T15:00:00Z").unwrap();
        let mut holds = Holds::default();
        let hold = held(Some("2026-11-14T15:00:00.000Z"));
        holds.said("cc-1", &hold);
        // Its own echo doesn't end it.
        holds.heard(&hold);
        assert!(holds.due(at - 1).is_empty());
        let due = holds.due(at);
        assert_eq!(due.len(), 1);
        let (session, ended) = &due[0];
        assert_eq!(session, "cc-1");
        assert_eq!(
            ended.text(),
            "#monorepo landings is ok: it was broken until 2026-11-14T15:00:00.000Z"
        );
        assert!(holds.due(at + 60).is_empty());
    }

    #[test]
    fn a_newer_state_or_one_without_a_time_ends_nothing_by_itself() {
        let at = zirc_core::time::seconds_at("2026-11-14T15:00:00Z").unwrap();
        let mut holds = Holds::default();
        holds.said("cc-1", &held(Some("2026-11-14T15:00:00.000Z")));
        holds.heard(&State {
            level: Level::Ok,
            ..held(None)
        });
        assert!(holds.due(at).is_empty());
        holds.said("cc-1", &held(None));
        assert!(holds.due(at).is_empty());
    }
}
