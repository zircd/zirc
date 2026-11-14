//! Waiting for something to become true, the one way tests do it: poll, and on failure say
//! what was waited for, for how long and what the program was showing. A test that sleeps a
//! fixed time is slow when things are fast and flaky when they aren't.
//!
//! ```ignore
//! let row = wait("the #api row").showing(|| zirc.summary()).until(|| zirc.row("#api"));
//! wait("a connection").until(|| zirc.is_connected()); // a bool works too
//! ```

use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// How long [`wait`] waits unless told otherwise.
pub const DEFAULT_LIMIT: Duration = Duration::from_secs(20);

/// How often it looks.
const INTERVAL: Duration = Duration::from_millis(20);

/// What a probe says: done, with a value, or not yet. `bool` and `Option<T>` both work.
pub trait Probe {
    /// What waiting produces.
    type Found;
    /// The value when it's done, `None` while it isn't.
    fn found(self) -> Option<Self::Found>;
}

impl Probe for bool {
    type Found = ();
    fn found(self) -> Option<()> {
        self.then_some(())
    }
}

impl<T> Probe for Option<T> {
    type Found = T;
    fn found(self) -> Option<T> {
        self
    }
}

/// A wait in the making: [`wait`] names it, the rest is optional, [`Wait::until`] runs it.
pub struct Wait<'a> {
    what: &'a str,
    limit: Duration,
    show: Option<Box<dyn Fn() -> String + 'a>>,
}

/// Starts waiting for `what`, as in "no {what} in 20s". Finish with [`Wait::until`].
#[must_use]
pub fn wait(what: &str) -> Wait<'_> {
    Wait {
        what,
        limit: DEFAULT_LIMIT,
        show: None,
    }
}

impl<'a> Wait<'a> {
    /// Gives up after `limit` instead of [`DEFAULT_LIMIT`].
    #[must_use]
    pub fn within(mut self, limit: Duration) -> Self {
        self.limit = limit;
        self
    }

    /// Adds what the program under test shows to the failure, such as its screen.
    #[must_use]
    pub fn showing(mut self, show: impl Fn() -> String + 'a) -> Self {
        self.show = Some(Box::new(show));
        self
    }

    /// Calls `probe` until it's done, and returns what it found.
    ///
    /// # Panics
    ///
    /// When the limit passes first, naming what was waited for.
    pub fn until<P: Probe>(self, mut probe: impl FnMut() -> P) -> P::Found {
        let started = Instant::now();
        loop {
            if let Some(found) = probe().found() {
                return found;
            }
            if started.elapsed() >= self.limit {
                let mut message = format!("no {} in {:?}", self.what, self.limit);
                if let Some(show) = &self.show {
                    let _ = write!(message, "; it shows:\n{}", show());
                }
                panic!("{message}");
            }
            std::thread::sleep(INTERVAL);
        }
    }

    /// Like [`Wait::until`], and says how long it took.
    ///
    /// # Panics
    ///
    /// When the limit passes first.
    pub fn until_timed(self, probe: impl FnMut() -> bool) -> Duration {
        let started = Instant::now();
        self.until(probe);
        started.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_what_the_probe_found() {
        let mut calls = 0;
        let found = wait("a third call").until(|| {
            calls += 1;
            (calls == 3).then_some(calls)
        });
        assert_eq!(found, 3);
    }

    #[test]
    #[should_panic(expected = "no a miracle in 50ms; it shows:\nthe screen")]
    fn names_what_it_waited_for_and_what_the_program_showed() {
        wait("a miracle")
            .within(Duration::from_millis(50))
            .showing(|| "the screen".into())
            .until(|| false);
    }
}
