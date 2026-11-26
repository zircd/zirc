//! One line for one wake of a session that holds a role (the user's choice, 26 Nov 2026,
//! after one landing showed `oncall woke: CI landed its branch … · build 10714` and then
//! `oncall woke as the oncall: CI landed its branch … · build 10714 · the item is closed`).
//! CI's own line about a build wakes the holder first, and its role's line about the same
//! build comes within the wake's window ([`super::wakes::TOGETHER`]), saying more: so the
//! first waits that long, and the role's line, when it comes, is said in its place, with its
//! wake's id. One the role doesn't follow is said once the window ends.

use std::time::{Duration, Instant};

use zirc_proto::woke::{What, Woke};

use super::Daemon;

/// How long a holder's line about a build waits for its role's: a wake's window.
const WAITS: Duration = Duration::from_secs(super::wakes::TOGETHER);

/// A wake's line waiting, for its session, to be said in its scope.
#[derive(Debug)]
struct Waiting {
    session: String,
    scope: String,
    woke: Woke,
    until: Instant,
}

/// The wake lines of role holders waiting for their role's.
#[derive(Debug, Default)]
pub(super) struct Held(Vec<Waiting>);

impl Held {
    /// Whether `woke`, for a session that holds a role, waits for its role's line: CI's
    /// about a build, which a role's line about the same build says again.
    fn waits(woke: &Woke) -> bool {
        woke.role.is_none() && woke.what == What::Build && woke.build.is_some()
    }

    /// Keeps `woke` for `session` in `scope`, to say at `now` plus [`WAITS`].
    fn hold(&mut self, session: &str, scope: &str, woke: Woke, now: Instant) {
        self.0.push(Waiting {
            session: session.to_owned(),
            scope: scope.to_owned(),
            woke,
            until: now + WAITS,
        });
    }

    /// The role's line `woke` for `session`, in place of the line waiting for it, if one
    /// is: with that line's wake id when it has none of its own.
    fn take(&mut self, session: &str, mut woke: Woke) -> Woke {
        if woke.role.is_none() || woke.build.is_none() {
            return woke;
        }
        if let Some(at) = self
            .0
            .iter()
            .position(|held| held.session == session && held.woke.build == woke.build)
        {
            let held = self.0.remove(at);
            woke.wake = woke.wake.or(held.woke.wake);
        }
        woke
    }

    /// The lines whose wait ended by `now`, to be said as they are: session, scope, line.
    fn due(&mut self, now: Instant) -> Vec<(String, String, Woke)> {
        let (due, waiting) = std::mem::take(&mut self.0)
            .into_iter()
            .partition(|held| held.until <= now);
        self.0 = waiting;
        due.into_iter()
            .map(|held: Waiting| (held.session, held.scope, held.woke))
            .collect()
    }
}

impl Daemon {
    /// `woke`, said for `session` in `scope`, as it's to be said now: `None` while it waits
    /// for its role's line, which is said in its place ([`Held`]).
    pub(super) fn hold_or_take_woke(
        &mut self,
        session: &str,
        scope: &str,
        woke: Woke,
    ) -> Option<Woke> {
        if Held::waits(&woke) && self.role_of(session).is_some() {
            self.woke_held.hold(session, scope, woke, self.clock.now());
            return None;
        }
        Some(self.woke_held.take(session, woke))
    }

    /// Once a second: says the role holders' lines whose role said nothing in time.
    pub(super) fn say_held_wokes(&mut self) {
        for (session, scope, woke) in self.woke_held.due(self.clock.now()) {
            self.publish_woke(&session, &scope, &woke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landed(role: Option<&str>, build: &str, wake: Option<&str>) -> Woke {
        let mut line = match role {
            Some(role) => Woke::closed_by_landing(role, Some(build.into()), Some("fix".into()), 1),
            None => Woke {
                build: Some(build.into()),
                landing: true,
                ..Woke::new(What::Build, zirc_proto::woke::By::Ci)
            },
        };
        line.wake = wake.map(str::to_owned);
        line
    }

    #[test]
    fn one_landing_of_a_role_holder_says_one_line_its_roles() {
        let (mut held, now) = (Held::default(), Instant::now());
        let ci = landed(None, "10714", Some("w1"));
        assert!(Held::waits(&ci));
        held.hold("cc-1", "monorepo", ci, now);
        // The role's line about the same build, part of the same wake: said with its id,
        // and CI's never.
        let role = held.take("cc-1", landed(Some("oncall"), "10714", None));
        assert_eq!(
            (role.role.as_deref(), role.wake.as_deref()),
            (Some("oncall"), Some("w1"))
        );
        assert!(held.due(now + WAITS).is_empty());
    }

    #[test]
    fn a_line_the_role_doesnt_follow_is_said_when_its_wait_ends() {
        let (mut held, now) = (Held::default(), Instant::now());
        held.hold("cc-1", "monorepo", landed(None, "10714", Some("w1")), now);
        // Another build's, or another session's, takes nothing.
        let other = held.take("cc-1", landed(Some("oncall"), "10715", None));
        assert_eq!(other.wake, None);
        assert_eq!(
            held.take("cc-2", landed(Some("oncall"), "10714", None))
                .wake,
            None
        );
        assert!(held.due(now + WAITS / 2).is_empty());
        let due = held.due(now + WAITS);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].0.as_str(), due[0].1.as_str()), ("cc-1", "monorepo"));
        assert_eq!(due[0].2.wake.as_deref(), Some("w1"));
        // A role's own line, or one about no build, never waits.
        assert!(!Held::waits(&landed(Some("oncall"), "10714", None)));
        assert!(!Held::waits(&Woke::new(
            What::Mentioned,
            zirc_proto::woke::By::Agent
        )));
    }
}
