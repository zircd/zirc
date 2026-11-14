//! Breakages nobody owns: a state said `broken` with no owner, such as CI saying master is
//! broken, that stays so for [`UNOWNED_FOR`] is told to the machine's owner, once, by direct
//! message. Overnight on 11 Nov 2026 two breakages of master went 5 to 7 hours with no owner:
//! seven sessions said in #ops that it wasn't theirs and left it there.

use std::collections::BTreeMap;

use zirc_proto::state::{Level, State};

use super::Daemon;

/// How long a breakage may go with no owner before the machine's owner is told.
pub(super) const UNOWNED_FOR: u64 = 30 * 60;

/// How old a state may be and still count: one heard in history long after it was said, by a
/// zircd that just started, isn't news.
const STALE: u64 = 24 * 60 * 60;

/// One breakage with no owner: since when, and why, as its state says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Breakage {
    since: u64,
    why: Option<String>,
    told: bool,
}

/// The breakages with no owner, by repository channel and state name.
#[derive(Debug, Default)]
pub(super) struct Unowned(BTreeMap<(String, String), Breakage>);

impl Unowned {
    /// Notes `state`, said at `at`: broken with no owner, it's one, from when it was first
    /// said so; anything else ends it.
    pub(super) fn heard(&mut self, state: &State, at: u64, now: u64) {
        let key = (state.repo.clone(), state.name.clone());
        if state.level == Level::Broken && state.owner.is_none() && now.saturating_sub(at) < STALE {
            self.0.entry(key).or_insert(Breakage {
                since: at,
                why: state.why.clone(),
                told: false,
            });
        } else {
            self.0.remove(&key);
        }
    }

    /// What to tell the machine's owner now: each breakage that has had no owner for
    /// [`UNOWNED_FOR`] and wasn't told yet, which it then is.
    pub(super) fn due(&mut self, now: u64) -> Vec<String> {
        self.0
            .iter_mut()
            .filter(|(_, breakage)| {
                !breakage.told && now.saturating_sub(breakage.since) >= UNOWNED_FOR
            })
            .map(|((repo, name), breakage)| {
                breakage.told = true;
                let minutes = now.saturating_sub(breakage.since) / 60;
                let why = breakage
                    .why
                    .as_deref()
                    .map_or_else(String::new, |why| format!(": {why}"));
                format!(
                    "{repo} {name} has been broken for {minutes} min and nobody is on it{why}\n\
                     - start a session on it, or have one take it: zirc state set --session \
                     <its session> {name} broken --owner <its session>"
                )
            })
            .collect()
    }
}

impl Daemon {
    /// Notes a state heard at `at` ([`Unowned::heard`]).
    pub(super) fn unowned_heard(&mut self, state: &State, at: u64) {
        self.unowned.heard(state, at, super::now());
    }

    /// Tells the machine's owner of each breakage that has gone too long with no owner.
    pub(super) fn unowned_tick(&mut self) {
        for notice in self.unowned.due(super::now()) {
            self.tell_owner(&notice);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_800_000_000;

    fn state(level: Level, owner: Option<&str>) -> State {
        State {
            repo: "#api".into(),
            name: "master".into(),
            level,
            why: Some("//web:e2e fails since build 9".into()),
            owner: owner.map(Into::into),
            link: None,
        }
    }

    #[test]
    fn a_breakage_nobody_owns_is_told_once_after_half_an_hour() {
        let mut unowned = Unowned::default();
        unowned.heard(&state(Level::Broken, None), AT, AT);
        assert!(unowned.due(AT + UNOWNED_FOR - 1).is_empty());
        let told = unowned.due(AT + UNOWNED_FOR);
        assert_eq!(told.len(), 1);
        assert!(
            told[0].starts_with(
                "#api master has been broken for 30 min and nobody is on it: //web:e2e fails"
            ),
            "{told:?}"
        );
        assert!(unowned.due(AT + 2 * UNOWNED_FOR).is_empty(), "told once");
    }

    #[test]
    fn an_owner_or_a_fix_ends_it() {
        for next in [state(Level::Broken, Some("cc-1")), state(Level::Ok, None)] {
            let mut unowned = Unowned::default();
            unowned.heard(&state(Level::Broken, None), AT, AT);
            unowned.heard(&next, AT + 60, AT + 60);
            assert!(unowned.due(AT + UNOWNED_FOR).is_empty(), "{next:?}");
        }
    }

    #[test]
    fn a_breakage_said_again_counts_from_the_first_time_and_an_old_one_not_at_all() {
        let mut unowned = Unowned::default();
        unowned.heard(&state(Level::Broken, None), AT, AT);
        unowned.heard(&state(Level::Broken, None), AT + 600, AT + 600);
        assert_eq!(unowned.due(AT + UNOWNED_FOR).len(), 1);
        let mut old = Unowned::default();
        old.heard(&state(Level::Broken, None), AT, AT + STALE);
        assert!(old.due(AT + STALE + UNOWNED_FOR).is_empty());
    }
}
