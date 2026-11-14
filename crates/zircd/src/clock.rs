//! The clock zircd counts its windows by, such as the wake breaker's ten minutes and how long
//! a session's watch may be gone before its leases are given back: every one the daemon
//! keeps to itself is counted by [`Clock::now`]. It's the real clock, unless a test has
//! moved it ahead, which tests do rather than wait those minutes out ([`crate::Hooks`]).
//!
//! The time of day is another matter ([`unix`]): other machines and the disk read it too,
//! so it's always the machine's own.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The real clock, and how far a test has moved it ahead: shared by every clone, so a test
/// keeps one and moves the daemon's.
#[derive(Debug, Clone, Default)]
pub struct Clock {
    ahead_ms: Arc<AtomicU64>,
}

/// Seconds since the Unix epoch, by the machine's real clock, which no test moves: what's
/// said to other machines and saved to disk is timed by it, as they read it by theirs.
#[must_use]
pub fn unix() -> u64 {
    unix_ms() / 1000
}

/// [`unix`], in milliseconds.
#[must_use]
pub fn unix_ms() -> u64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

impl Clock {
    /// Now, by this clock.
    #[must_use]
    pub fn now(&self) -> Instant {
        Instant::now() + Duration::from_millis(self.ahead_ms.load(Ordering::SeqCst))
    }

    /// Moves it `by` ahead, as if that much time had passed.
    pub fn advance(&self, by: Duration) {
        let by = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
        self.ahead_ms.fetch_add(by, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_moved_ahead_is_ahead_in_every_clone() {
        let clock = Clock::default();
        let other = clock.clone();
        let before = Instant::now();
        clock.advance(Duration::from_mins(10));
        assert!(other.now().duration_since(before) >= Duration::from_mins(10));
    }
}
