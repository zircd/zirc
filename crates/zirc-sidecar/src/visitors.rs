//! Telling a tenant's owners, on their phones, that a visitor is here (`Public::alert`, set
//! by the controller for zirc's own server).
//!
//! The gateway (`crate::gateway`) is the only one that sees guests, so it says here when one
//! comes into a public channel. This module decides what is worth a push, and the gateway
//! sends it as a private message to each owner from the operator, which Ergo's web push turns
//! into a notification like any other DM. Nothing leaves the tenant but through that path: the
//! text is never logged, and the team's staff, who aren't guests, never count.
//!
//! The owners' own apps are woken at most so often: each visitor once per [`PER_VISITOR`], and
//! a push no sooner than [`ARRIVAL_GAP`] after the last, what came meanwhile joining the next
//! one. The words are one plain line.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long after a visitor's arrival was told the same visitor's isn't, again.
pub const PER_VISITOR: Duration = Duration::from_mins(10);

/// The least between two pushes about arrivals; those between are told in the next.
pub const ARRIVAL_GAP: Duration = Duration::from_mins(1);

/// The most names a push waiting holds on to; more are only counted.
const HELD: usize = 20;

/// The nick the push is sent from: one of zirc's own (`zirc_proto::names`), which no team's
/// hook may post as, so nobody can pass for it.
pub const FROM: &str = zirc_proto::names::SYSTEM;

/// What waits for its turn to be told.
#[derive(Debug)]
struct Lane<T> {
    gap: Duration,
    last: Option<Instant>,
    held: Vec<T>,
    count: usize,
}

impl<T> Lane<T> {
    const fn new(gap: Duration) -> Self {
        Self {
            gap,
            last: None,
            held: Vec::new(),
            count: 0,
        }
    }

    /// Takes everything waiting, if the gap since the last push is over.
    fn take(&mut self, now: Instant) -> Option<(Vec<T>, usize)> {
        if self.count == 0 || self.last.is_some_and(|l| now.duration_since(l) < self.gap) {
            return None;
        }
        self.last = Some(now);
        Some((
            std::mem::take(&mut self.held),
            std::mem::take(&mut self.count),
        ))
    }

    fn push(&mut self, item: T, now: Instant) -> Option<(Vec<T>, usize)> {
        self.count += 1;
        if self.held.len() < HELD {
            self.held.push(item);
        }
        self.take(now)
    }
}

/// What the owners are to be told, and what is waiting.
#[derive(Debug)]
pub struct Visitors {
    arrivals: Lane<String>,
    /// When each visitor's arrival was last told.
    told: HashMap<String, Instant>,
}

impl Default for Visitors {
    fn default() -> Self {
        Self::new()
    }
}

/// `text` as one short line: control characters become spaces and what is over `max`
/// characters is cut, with an ellipsis.
#[must_use]
pub fn plain(text: &str, max: usize) -> String {
    let line = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() <= max {
        line
    } else {
        let mut cut: String = line.chars().take(max).collect();
        cut.push('…');
        cut
    }
}

impl Visitors {
    #[must_use]
    /// No visitors waiting yet.
    pub fn new() -> Self {
        Self {
            arrivals: Lane::new(ARRIVAL_GAP),
            told: HashMap::new(),
        }
    }

    /// `visitor` (a guest's nick, with their name if they gave one) came into `place`, a
    /// public channel, in `room`, theirs beside it (or `place`); the push to send now, if any,
    /// which names the room when it's about them alone, so a tap on it opens it.
    pub fn arrived(
        &mut self,
        visitor: &str,
        place: &str,
        room: &str,
        now: Instant,
    ) -> Option<String> {
        self.told
            .retain(|_, at| now.duration_since(*at) < PER_VISITOR);
        if self.told.contains_key(visitor) {
            return None;
        }
        self.told.insert(visitor.to_owned(), now);
        let (names, count) = self.arrivals.push(plain(visitor, 40), now)?;
        Some(arrivals_text(
            &names,
            count,
            if count == 1 { room } else { place },
        ))
    }

    /// What has waited long enough since, for a periodic look.
    pub fn due(&mut self, place: &str, now: Instant) -> Vec<String> {
        let mut texts = Vec::new();
        if let Some((names, count)) = self.arrivals.take(now) {
            texts.push(arrivals_text(&names, count, place));
        }
        texts
    }
}

fn arrivals_text(names: &[String], count: usize, place: &str) -> String {
    match names {
        [one] if count == 1 => format!("{one} came into {place}"),
        _ => format!(
            "{count} visitors came into {place}: {}{}",
            names.join(", "),
            if count > names.len() { ", …" } else { "" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACE: &str = "#support";

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn a_visitor_s_arrival_is_told_at_once_and_only_once_in_ten_minutes() {
        let (mut v, t) = (Visitors::new(), Instant::now());
        assert_eq!(
            v.arrived("guest-ab12", PLACE, PLACE, t).as_deref(),
            Some("guest-ab12 came into #support")
        );
        // A reload, a reconnect: not again.
        assert_eq!(v.arrived("guest-ab12", PLACE, PLACE, at(t, 120)), None);
        assert_eq!(v.arrived("guest-ab12", PLACE, PLACE, at(t, 599)), None);
        assert!(v.arrived("guest-ab12", PLACE, PLACE, at(t, 601)).is_some());
    }

    #[test]
    fn arrivals_within_the_gap_join_the_next_push() {
        let (mut v, t) = (Visitors::new(), Instant::now());
        assert!(v.arrived("guest-a", PLACE, PLACE, t).is_some());
        assert_eq!(v.arrived("guest-b", PLACE, PLACE, at(t, 5)), None);
        assert_eq!(v.arrived("guest-c", PLACE, PLACE, at(t, 10)), None);
        assert!(v.due(PLACE, at(t, 30)).is_empty());
        assert_eq!(
            v.due(PLACE, at(t, 61)),
            ["2 visitors came into #support: guest-b, guest-c"]
        );
        assert!(v.due(PLACE, at(t, 4000)).is_empty());
    }

    #[test]
    fn a_flood_of_visitors_is_counted_not_listed() {
        let (mut v, t) = (Visitors::new(), Instant::now());
        assert!(v.arrived("guest-0", PLACE, PLACE, t).is_some());
        for i in 1..=100 {
            assert_eq!(
                v.arrived(&format!("guest-{i}"), PLACE, PLACE, at(t, 1)),
                None
            );
        }
        let texts = v.due(PLACE, at(t, 61));
        assert_eq!(texts.len(), 1);
        assert!(texts[0].starts_with("100 visitors came into #support: guest-1, guest-2"));
        assert!(texts[0].ends_with(", …"));
        assert!(texts[0].len() < 400);
    }

    #[test]
    fn a_name_is_one_short_plain_line() {
        let long = format!("a\r\nb\u{1}\u{7} {}", "x".repeat(200));
        let text = plain(&long, 40);
        assert!(!text.contains(['\r', '\n', '\u{1}', '\u{7}']));
        assert!(text.ends_with('…'));
        assert_eq!(text.chars().count(), 41);
        assert_eq!(plain("  a \n\t b  ", 10), "a b");
    }
}
