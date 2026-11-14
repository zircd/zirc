//! Keeping a connection up, without any I/O: how long to wait between attempts and when to
//! ping a quiet server, the same way in every app. `zirc-core`'s supervisor keeps a TCP
//! connection up by its [`Policy`]; the web client keeps a WebSocket up with [`Keepalive`]
//! and [`Backoff`], passing in the time, as a browser's clock is the page's.
//!
//! Times here are [`Duration`]s since whatever origin the caller counts from, such as the
//! page loading, and only ever compared with each other.

use std::time::Duration;

use chrono::{DateTime, Utc};

use zirc_line::Line;

/// How the connection is kept up. [`Policy::default`] suits every app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The wait before the first attempt after the connection is lost, doubled after each
    /// failure up to [`Policy::max_retry`]. Each wait is cut by up to half at random, so
    /// clients that lost the server together don't come back together.
    pub first_retry: Duration,
    /// The longest wait between attempts.
    pub max_retry: Duration,
    /// How long an attempt may take, connecting and registering, before it's given up: a
    /// network that drops what it's sent leaves a connection waiting far longer.
    pub connect_within: Duration,
    /// How long the server may be silent before it's pinged.
    pub keepalive: Duration,
    /// How long it then has to answer before the connection counts as dead.
    pub answer_within: Duration,
    /// How long it has to answer when the machine woke or the network changed.
    pub probe_within: Duration,
    /// How long a connection must last for the next loss to start the backoff over.
    pub stable: Duration,
    /// How often to check whether the machine slept.
    pub wake_check: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            first_retry: Duration::from_secs(1),
            max_retry: Duration::from_secs(30),
            connect_within: Duration::from_secs(20),
            keepalive: Duration::from_secs(30),
            answer_within: Duration::from_secs(15),
            probe_within: Duration::from_secs(10),
            stable: Duration::from_secs(60),
            wake_check: Duration::from_secs(5),
        }
    }
}

impl Policy {
    /// The wait before the next attempt after `failures` failed ones:
    /// [`Policy::first_retry`] doubled for each, up to [`Policy::max_retry`], less up to
    /// half of it as `fraction`, a random number from 0 up to 1, says.
    #[must_use]
    pub fn backoff(&self, failures: u32, fraction: f64) -> Duration {
        let doubled = self
            .first_retry
            .saturating_mul(1 << failures.min(16))
            .min(self.max_retry);
        doubled.mul_f64(1.0 - fraction.clamp(0.0, 1.0) / 2.0)
    }
}

/// Failed attempts since the last stable connection, for [`Policy::backoff`].
#[derive(Debug, Clone, Default)]
pub struct Backoff {
    failures: u32,
    /// When the connection came up, while it's up.
    since: Option<Duration>,
}

impl Backoff {
    /// The connection came up at `now`.
    pub fn connected(&mut self, now: Duration) {
        self.since = Some(now);
    }

    /// The connection, or an attempt, ended at `now`: how long to wait before the next
    /// attempt, with `fraction` as [`Policy::backoff`] takes it.
    pub fn lost(&mut self, policy: &Policy, now: Duration, fraction: f64) -> Duration {
        if self
            .since
            .take()
            .is_some_and(|since| now.saturating_sub(since) >= policy.stable)
        {
            self.failures = 0;
        }
        let wait = policy.backoff(self.failures, fraction);
        self.failures = self.failures.saturating_add(1);
        wait
    }
}

/// What a probe's `PING` says, so its answer is told apart from others.
pub const PROBE: &str = "zirc-probe";

/// Noticing a connection that died without closing: a server silent for
/// [`Policy::keepalive`] is pinged and must answer within [`Policy::answer_within`]. Hand it
/// every line heard, and [`Keepalive::poll`] it by [`Keepalive::due`].
#[derive(Debug, Clone)]
pub struct Keepalive {
    policy: Policy,
    /// When a PING is due or, once one is sent, when the server must have answered by.
    deadline: Duration,
    /// How long the server was given to answer the PING sent, if one was.
    pinged: Option<Duration>,
    /// Whether only the answer to a probe's PING will do: lines sent before it may still be
    /// arriving.
    probing: bool,
}

impl Keepalive {
    /// A connection registered at `now`.
    #[must_use]
    pub fn new(policy: Policy, now: Duration) -> Self {
        Self {
            deadline: now + policy.keepalive,
            policy,
            pinged: None,
            probing: false,
        }
    }

    /// When to [`Keepalive::poll`] next.
    #[must_use]
    pub fn due(&self) -> Duration {
        self.deadline
    }

    /// `line` arrived at `now`. Answers a PING with its PONG.
    pub fn heard(&mut self, now: Duration, line: &Line) -> Option<Line> {
        let answered = line.command == "PONG" && line.params.last().is_some_and(|p| p == PROBE);
        if !self.probing || answered {
            self.probing = false;
            self.pinged = None;
            self.deadline = now + self.policy.keepalive;
        }
        (line.command == "PING").then(|| Line::new("PONG", line.params.clone()))
    }

    /// The machine woke or the network changed at `now`: the PING to send, which the
    /// server must answer within [`Policy::probe_within`].
    pub fn probe(&mut self, now: Duration) -> Line {
        let soon = now + self.policy.probe_within;
        if self.pinged.is_none() || soon < self.deadline {
            self.pinged = Some(self.policy.probe_within);
            self.deadline = soon;
        }
        self.probing = true;
        Line::new("PING", [PROBE])
    }

    /// What to do at `now`: send a PING when the server has been quiet, nothing yet, or
    /// give up on a server that didn't answer, saying why.
    ///
    /// # Errors
    ///
    /// When the connection counts as dead: close it and connect again.
    pub fn poll(&mut self, now: Duration) -> Result<Option<Line>, String> {
        if now < self.deadline {
            return Ok(None);
        }
        if let Some(within) = self.pinged {
            return Err(format!("the server didn't answer in {within:?}"));
        }
        self.pinged = Some(self.policy.answer_within);
        self.deadline = now + self.policy.answer_within;
        Ok(Some(Line::new("PING", ["zirc"])))
    }
}

/// Where the connection is, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Connecting: the first attempt, or the `attempt`th since the connection was lost.
    Connecting {
        /// Counting from 1.
        attempt: u32,
        /// Whether it was ever connected, so this is getting back on rather than on at all.
        was_up: bool,
    },
    /// Connected and registered.
    Connected {
        /// Since when.
        since: DateTime<Utc>,
    },
    /// Not connected, and trying again at `retry_at`.
    Offline {
        /// Since when the connection's been lost.
        since: DateTime<Utc>,
        /// Why the last connection or attempt ended.
        why: String,
        /// When the next attempt starts.
        retry_at: DateTime<Utc>,
        /// Whether it was ever connected: what was lost was a connection, not only attempts.
        was_up: bool,
    },
}

impl Status {
    /// Whether it's connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    /// Whether it is, or ever was, connected: what the next status's `was_up` says.
    #[must_use]
    pub fn was_up(&self) -> bool {
        match self {
            Self::Connected { .. } => true,
            Self::Connecting { was_up, .. } | Self::Offline { was_up, .. } => *was_up,
        }
    }

    /// It in a few words, at `now`: `offline for 2m (the server closed the connection),
    /// retrying in 8s`.
    #[must_use]
    pub fn describe(&self, now: DateTime<Utc>) -> String {
        match self {
            Self::Connecting { attempt: 1, .. } => "connecting…".to_owned(),
            Self::Connecting { attempt, .. } => format!("connecting… (attempt {attempt})"),
            Self::Connected { .. } => "connected".to_owned(),
            Self::Offline {
                since,
                why,
                retry_at,
                ..
            } => {
                let retry = if *retry_at > now {
                    format!("retrying in {}", span(*retry_at - now))
                } else {
                    "retrying now".to_owned()
                };
                format!("offline for {} ({why}), {retry}", span(now - *since))
            }
        }
    }

    /// It as every app's banner over the chat says it while not connected, at `now`:
    /// `Connecting…` until it's first connected, `Reconnecting…` while getting back on, both
    /// also for the first [`QUIET_OFFLINE`] offline, as most drops come back by then; after
    /// that, `Offline · retrying in 12s`, in minutes from a minute (`in 3m`). Never why,
    /// which [`Status::describe`] says for a place with room for it. None when connected.
    /// Redraw it at [`Status::banner_changes`].
    #[must_use]
    pub fn banner(&self, now: DateTime<Utc>) -> Option<String> {
        self.banner_words(now, countdown)
    }

    /// When [`Status::banner`] next says something else, after `now`: each second while
    /// it counts seconds, each minute while minutes, and when the quiet start ends. None
    /// when it won't change by itself, as while connecting.
    #[must_use]
    pub fn banner_changes(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let Self::Offline {
            since, retry_at, ..
        } = self
        else {
            return None;
        };
        if now - *since < QUIET_OFFLINE {
            return Some(*since + QUIET_OFFLINE);
        }
        (*retry_at > now).then(|| {
            let next = countdown_next(seconds_left(*retry_at - now));
            *retry_at - chrono::TimeDelta::seconds(i64::try_from(next).unwrap_or(0))
        })
    }

    /// [`Status::banner`] for a screen reader's live region: the state alone, never the
    /// countdown, so a long outage is said once rather than talked over what the person
    /// reads. `Connecting…` or `Reconnecting…` as [`Status::banner`] says them, then
    /// [`OFFLINE_SPOKEN`] from going offline on, through the attempts that follow, until
    /// connected. None when connected.
    #[must_use]
    pub fn banner_spoken(&self, now: DateTime<Utc>) -> Option<String> {
        match self {
            Self::Connecting { attempt: 2.., .. } => Some(OFFLINE_SPOKEN.to_owned()),
            Self::Offline { since, .. } if now - *since >= QUIET_OFFLINE => {
                Some(OFFLINE_SPOKEN.to_owned())
            }
            _ => self.banner(now),
        }
    }

    fn banner_words(&self, now: DateTime<Utc>, left: impl Fn(u64) -> String) -> Option<String> {
        match self {
            Self::Connected { .. } => None,
            Self::Offline {
                since, retry_at, ..
            } if now - *since >= QUIET_OFFLINE => Some(if *retry_at > now {
                format!("Offline · retrying {}", left(seconds_left(*retry_at - now)))
            } else {
                "Offline · retrying now".to_owned()
            }),
            // Getting on, and for the quiet start offline.
            _ if self.was_up() => Some(RECONNECTING.to_owned()),
            _ => Some(CONNECTING.to_owned()),
        }
    }

    /// It in a word, for a status line that a [`Status::banner`] says the rest above:
    /// `connecting` or `offline`. None when connected.
    #[must_use]
    pub fn short(&self) -> Option<&'static str> {
        match self {
            Self::Connected { .. } => None,
            Self::Connecting { .. } => Some("connecting"),
            Self::Offline { .. } => Some("offline"),
        }
    }
}

/// What a [`Status::banner`] says while getting on, before it's ever been connected.
const CONNECTING: &str = "Connecting…";

/// What a screen reader hears once offline ([`Status::banner_spoken`]), until it's connected.
pub const OFFLINE_SPOKEN: &str = "Offline. Retrying automatically.";

/// What a [`Status::banner`] says while getting back on.
const RECONNECTING: &str = "Reconnecting…";

/// What every app's banner, and its connection's words, say while the device has no network
/// at all: nothing to retry until it has one. Only a phone knows it has none.
pub const NO_NETWORK: &str = "Offline · waiting for a network";

/// What every app says of the connection when nobody's signed in.
pub const SIGNED_OUT: &str = "Signed out";

/// How long a [`Status::banner`] says only [`RECONNECTING`] after the connection drops.
pub const QUIET_OFFLINE: chrono::TimeDelta = chrono::TimeDelta::seconds(3);

/// Whole seconds left of `delta`, rounding up, as a countdown counts them.
fn seconds_left(delta: chrono::TimeDelta) -> u64 {
    u64::try_from((delta.num_milliseconds().max(0) + 999) / 1000).unwrap_or_default()
}

/// `seconds` left, as a countdown says them: `in 12s` under a minute, then rounding up to
/// whole minutes, `in 3m`, and hours from an hour, `in 1h`; `now` at 0.
fn countdown(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    match seconds {
        0 => "now".to_owned(),
        1..60 => format!("in {seconds}s"),
        _ if minutes < 60 => format!("in {minutes}m"),
        _ => format!("in {}h", seconds.div_ceil(3600)),
    }
}

/// The seconds left at which [`countdown`] next says something else, counting down from
/// `seconds`: the second before under a minute, then each minute.
fn countdown_next(seconds: u64) -> u64 {
    let minutes = seconds.div_ceil(60);
    match seconds {
        0..=60 => seconds.saturating_sub(1),
        _ if minutes < 60 => (minutes - 1) * 60,
        _ => ((seconds.div_ceil(3600) - 1) * 3600).max(59 * 60),
    }
}

/// A span of time as [`crate::chat::span`] says it, rounding seconds up: `8s`, `2m`, `3h`.
fn span(delta: chrono::TimeDelta) -> String {
    let seconds = (delta.num_milliseconds().max(0) + 999) / 1000;
    crate::chat::span(u64::try_from(seconds).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn line(text: &str) -> Line {
        text.parse().unwrap()
    }

    #[test]
    fn backs_off_doubling_up_to_the_most_less_up_to_half() {
        let policy = Policy::default();
        let waits: Vec<_> = (0..7).map(|n| policy.backoff(n, 0.0)).collect();
        assert_eq!(
            waits,
            [
                secs(1),
                secs(2),
                secs(4),
                secs(8),
                secs(16),
                secs(30),
                secs(30)
            ]
        );
        assert_eq!(policy.backoff(3, 1.0), secs(4));
        assert_eq!(policy.backoff(3, 0.5), secs(6));
    }

    #[test]
    fn a_stable_connection_starts_the_backoff_over() {
        let policy = Policy::default();
        let mut backoff = Backoff::default();
        assert_eq!(backoff.lost(&policy, secs(0), 0.0), secs(1));
        assert_eq!(backoff.lost(&policy, secs(1), 0.0), secs(2));
        // Up briefly: still backing off.
        backoff.connected(secs(10));
        assert_eq!(backoff.lost(&policy, secs(20), 0.0), secs(4));
        // Up for longer than `stable`: from the start.
        backoff.connected(secs(30));
        assert_eq!(backoff.lost(&policy, secs(100), 0.0), secs(1));
    }

    #[test]
    fn pings_a_quiet_server_and_gives_up_when_it_doesnt_answer() {
        let mut keepalive = Keepalive::new(Policy::default(), secs(0));
        assert_eq!(keepalive.poll(secs(29)), Ok(None));
        assert_eq!(keepalive.due(), secs(30));
        assert_eq!(keepalive.poll(secs(30)), Ok(Some(line("PING zirc"))));
        assert_eq!(keepalive.due(), secs(45));
        assert_eq!(keepalive.poll(secs(44)), Ok(None));
        assert_eq!(
            keepalive.poll(secs(45)),
            Err("the server didn't answer in 15s".into())
        );
    }

    #[test]
    fn anything_heard_counts_as_an_answer() {
        let mut keepalive = Keepalive::new(Policy::default(), secs(0));
        keepalive.poll(secs(30)).unwrap();
        assert_eq!(keepalive.heard(secs(40), &line(":s NOTICE me :hi")), None);
        assert_eq!(keepalive.due(), secs(70));
        assert_eq!(
            keepalive.heard(secs(41), &line("PING :s")),
            Some(line("PONG s"))
        );
    }

    #[test]
    fn only_the_answer_to_a_probe_counts() {
        let mut keepalive = Keepalive::new(Policy::default(), secs(0));
        assert_eq!(keepalive.probe(secs(5)), line("PING zirc-probe"));
        assert_eq!(keepalive.due(), secs(15));
        // Sent before the PING arrived, maybe: not an answer.
        keepalive.heard(secs(6), &line(":s NOTICE me :earlier"));
        assert_eq!(keepalive.due(), secs(15));
        keepalive.heard(secs(7), &line(":s PONG s zirc-probe"));
        assert_eq!(keepalive.due(), secs(37));
        assert_eq!(keepalive.poll(secs(15)), Ok(None));
    }

    fn offline(was_up: bool, for_s: i64, retry_ms: i64) -> (Status, DateTime<Utc>) {
        let now = DateTime::UNIX_EPOCH + chrono::TimeDelta::days(20_000);
        let status = Status::Offline {
            since: now - chrono::TimeDelta::seconds(for_s),
            why: "connection refused".into(),
            retry_at: now + chrono::TimeDelta::milliseconds(retry_ms),
            was_up,
        };
        (status, now)
    }

    #[test]
    fn counts_down_in_seconds_then_in_whole_minutes_rounding_up() {
        let said: Vec<_> = [0, 1, 59, 60, 61, 125, 3599, 3600, 7201]
            .into_iter()
            .map(countdown)
            .collect();
        assert_eq!(
            said,
            [
                "now", "in 1s", "in 59s", "in 1m", "in 2m", "in 3m", "in 1h", "in 1h", "in 3h"
            ]
        );
    }

    #[test]
    fn says_connecting_until_it_was_up_and_reconnecting_after() {
        let first = Status::Connecting {
            attempt: 1,
            was_up: false,
        };
        let again = Status::Connecting {
            attempt: 1,
            was_up: true,
        };
        let now = Utc::now();
        assert_eq!(first.banner(now).as_deref(), Some("Connecting…"));
        assert_eq!(again.banner(now).as_deref(), Some("Reconnecting…"));
        assert!(Status::Connected { since: now }.was_up());
        // A first attempt that failed is still only connecting, for the quiet start.
        let (status, now) = offline(false, 1, 5000);
        assert_eq!(status.banner(now).as_deref(), Some("Connecting…"));
        let (status, now) = offline(true, 1, 5000);
        assert_eq!(status.banner(now).as_deref(), Some("Reconnecting…"));
        // Then the countdown, either way.
        let (status, now) = offline(false, 4, 5000);
        assert_eq!(
            status.banner(now).as_deref(),
            Some("Offline · retrying in 5s")
        );
    }

    #[test]
    fn the_countdown_changes_each_second_then_each_minute_and_never_goes_stale() {
        for (left, then) in [
            (0, None),
            (1, Some(1)),
            (12, Some(1)),
            (59, Some(1)),
            (60, Some(1)),
            (61, Some(1)),
            (125, Some(5)),
            (3600, Some(60)),
        ] {
            let (status, now) = offline(true, 10, left * 1000);
            let changes = status
                .banner_changes(now)
                .map(|at| (at - now).num_seconds());
            assert_eq!(changes, then, "{left}s left");
            // Just before, it still says the same; at the change, something else.
            if let Some(at) = status.banner_changes(now) {
                let before = at - chrono::TimeDelta::milliseconds(1);
                assert_eq!(status.banner(before), status.banner(now), "{left}s left");
                assert_ne!(status.banner(at), status.banner(now), "{left}s left");
            }
        }
        // Mid-second, the next change is at the second's end.
        let (status, now) = offline(true, 10, 11_400);
        assert_eq!(
            status.banner(now).as_deref(),
            Some("Offline · retrying in 12s")
        );
        assert_eq!(
            status.banner_changes(now),
            Some(now + chrono::TimeDelta::milliseconds(400))
        );
        // The quiet start ends at QUIET_OFFLINE; connected or connecting, nothing to redraw.
        let (status, now) = offline(false, 1, 9000);
        assert_eq!(
            status.banner_changes(now),
            Some(now + chrono::TimeDelta::seconds(2))
        );
        assert_eq!(Status::Connected { since: now }.banner_changes(now), None);
        assert_eq!(
            Status::Connecting {
                attempt: 2,
                was_up: true
            }
            .banner_changes(now),
            None
        );
    }

    #[test]
    fn a_screen_reader_hears_the_state_only_never_the_countdown() {
        let spoken = |was_up, for_s, left_ms| {
            let (status, now) = offline(was_up, for_s, left_ms);
            status.banner_spoken(now).unwrap()
        };
        assert_eq!(spoken(true, 1, 5_000), "Reconnecting…");
        assert_eq!(spoken(false, 1, 5_000), "Connecting…");
        for left_ms in [125_000, 12_000, 3_000, 0] {
            assert_eq!(spoken(true, 10, left_ms), OFFLINE_SPOKEN);
        }
        // The attempts after the first say nothing new: still offline, retrying.
        let retrying = Status::Connecting {
            attempt: 3,
            was_up: true,
        };
        assert_eq!(
            retrying.banner_spoken(Utc::now()).as_deref(),
            Some(OFFLINE_SPOKEN)
        );
        let first = Status::Connecting {
            attempt: 1,
            was_up: false,
        };
        assert_eq!(
            first.banner_spoken(Utc::now()).as_deref(),
            Some("Connecting…")
        );
        assert_eq!(
            Status::Connected { since: Utc::now() }.banner_spoken(Utc::now()),
            None
        );
    }
}
