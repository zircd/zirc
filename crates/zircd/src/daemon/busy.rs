//! How long the daemon's one task was busy with one thing lately: an input handled, or a
//! tick. Everything else waits meanwhile, a `zirc` command's request included, so the
//! longest stretch in the last minute is how long a command can have waited, and
//! `zirc status` says it ([`zirc_core::ipc::Status::busy_ms`]). Git was once read on this
//! task, and nothing said so but commands that took seconds.

use std::time::{Duration, Instant};

/// How long each of the two spans kept is: the longest stretch said is of the last one to
/// two of them.
const SPAN: Duration = Duration::from_secs(60);

/// The longest stretch in the span begun at `since`, and in the one before it.
#[derive(Debug)]
pub(super) struct Busy {
    since: Instant,
    longest: Duration,
    before: Duration,
}

impl Busy {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            since: now,
            longest: Duration::ZERO,
            before: Duration::ZERO,
        }
    }

    /// Moves on to the span `now` is in: the one before it is kept, older ones aren't.
    fn turn(&mut self, now: Instant) {
        let spans = now.duration_since(self.since).as_secs() / SPAN.as_secs();
        if spans == 0 {
            return;
        }
        self.before = if spans == 1 {
            self.longest
        } else {
            Duration::ZERO
        };
        self.longest = Duration::ZERO;
        self.since = now;
    }

    /// The task was busy with one thing from `start` to `now`.
    pub(super) fn took(&mut self, start: Instant, now: Instant) {
        self.turn(now);
        self.longest = self.longest.max(now.duration_since(start));
    }

    /// The longest it was busy with one thing lately, as of `now`, in milliseconds.
    pub(super) fn longest_ms(&mut self, now: Instant) -> u64 {
        self.turn(now);
        let longest = self.longest.max(self.before);
        u64::try_from(longest.as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_the_longest_stretch_of_the_last_minute_or_two_and_forgets_older_ones() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut busy = Busy::new(start);
        assert_eq!(busy.longest_ms(start), 0);
        busy.took(at(1), at(4));
        busy.took(at(5), at(6));
        assert_eq!(busy.longest_ms(at(10)), 3000);
        // A minute on it's the span before's, still said; a shorter one doesn't hide it.
        busy.took(at(70), at(71));
        assert_eq!(busy.longest_ms(at(75)), 3000);
        // Two minutes on from then, nothing that long has happened lately.
        busy.took(at(140), at(140) + Duration::from_millis(20));
        assert_eq!(busy.longest_ms(at(141)), 1000);
        assert_eq!(busy.longest_ms(at(400)), 0);
    }
}
