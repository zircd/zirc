//! How every app's message list scrolls, decided once: how near its end counts as reading
//! at the end, so what arrives follows and the list stays at the end as it changes size, and
//! how near its top the page of history before is asked for, so it's in before it's reached.
//! In the units each app lays out in: points on Apple's, dp on Android, CSS pixels on the
//! web; the terminal client's rows count `pin_within` as none, as a row is a line.

/// The rules every message list follows; [`SCROLL_RULES`] is them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollRules {
    /// Within this many points of the end, the person reads at the end.
    pub pin_within: f32,
    /// Within this many screenfuls of the top, the page before is asked for.
    pub older_within_screens: f32,
}

/// Forty points is a short line and a half: a list nudged up by a resize, or a scroll that
/// overshot, is still at the end; two screenfuls ask early enough that a quick scroll up
/// rarely reaches the top before the page is in.
pub const SCROLL_RULES: ScrollRules = ScrollRules {
    pin_within: 40.0,
    older_within_screens: 2.0,
};

impl ScrollRules {
    /// Whether a list `from_end` points from its end is at the end.
    #[must_use]
    pub fn at_end(&self, from_end: f32) -> bool {
        from_end <= self.pin_within
    }

    /// Whether a list `from_top` from its top, `screen` tall, should ask for the page before.
    #[must_use]
    pub fn wants_older(&self, from_top: f32, screen: f32) -> bool {
        from_top <= screen * self.older_within_screens
    }
}

/// One change to a list's rows, as a virtualised list is told it: the `removed` rows from
/// `at` gave way to `added` new ones. Rows outside it keep what the list knows of them,
/// their heights above all, and the place read stays on the row it was on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Splice {
    /// The first row changed, counted after the splices before it.
    pub at: usize,
    /// How many rows from `at` went.
    pub removed: usize,
    /// How many rows came in their place.
    pub added: usize,
}

/// How a list's rows went from `old` to `new`, as at most two [`Splice`]s, applied in order:
/// rows gone from the front (history dropped to keep a conversation's size), then the one run
/// between what's the same at the start and at the end. So a page of older history coming in
/// above is rows added at the top, and messages arriving are rows added at the end, and
/// neither disturbs the rows read. Rows are the same when `key` (a message's id, say) and
/// `same` agree: `same` compares what's drawn, leaving out what moves with the rows, such as
/// their places among a conversation's entries.
pub fn splices<T, K: Eq + std::hash::Hash>(
    old: &[T],
    new: &[T],
    key: impl Fn(&T) -> K,
    same: impl Fn(&T, &T) -> bool,
) -> Vec<Splice> {
    let mut out = Vec::new();
    // Rows at the front that are no longer anywhere: dropped.
    let gone = if old
        .first()
        .is_some_and(|row| new.first().is_none_or(|first| key(row) != key(first)))
    {
        let kept: std::collections::HashSet<K> = new.iter().map(&key).collect();
        old.iter()
            .take_while(|row| !kept.contains(&key(row)))
            .count()
    } else {
        0
    };
    if gone > 0 {
        out.push(Splice {
            at: 0,
            removed: gone,
            added: 0,
        });
    }
    let old = &old[gone..];
    let alike = |a: &T, b: &T| key(a) == key(b) && same(a, b);
    let end = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| alike(a, b))
        .count();
    let start = old[..old.len() - end]
        .iter()
        .zip(&new[..new.len() - end])
        .take_while(|(a, b)| alike(a, b))
        .count();
    let removed = old.len() - end - start;
    let added = new.len() - end - start;
    if removed > 0 || added > 0 {
        out.push(Splice {
            at: start,
            removed,
            added,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{SCROLL_RULES, Splice, splices};

    /// Rows as (key, what's drawn).
    fn changes(old: &[(&str, u8)], new: &[(&str, u8)]) -> Vec<(usize, usize, usize)> {
        splices(old, new, |row| row.0, |a, b| a.1 == b.1)
            .into_iter()
            .map(|Splice { at, removed, added }| (at, removed, added))
            .collect()
    }

    #[test]
    fn older_history_is_rows_added_at_the_top() {
        let old = [("d1", 0), ("a", 0), ("b", 0)];
        let new = [("d0", 0), ("x", 0), ("y", 0), ("d1", 0), ("a", 0), ("b", 0)];
        assert_eq!(changes(&old, &new), [(0, 0, 3)]);
        // The same day: its divider stays first, the page goes in under it.
        let new = [("d1", 0), ("x", 0), ("y", 0), ("a", 0), ("b", 0)];
        assert_eq!(changes(&old, &new), [(1, 0, 2)]);
    }

    #[test]
    fn arrivals_and_edits_touch_only_their_rows() {
        let old = [("a", 0), ("b", 0), ("c", 0)];
        assert_eq!(
            changes(&old, &[("a", 0), ("b", 0), ("c", 0), ("d", 0)]),
            [(3, 0, 1)]
        );
        assert_eq!(changes(&old, &[("a", 0), ("b", 1), ("c", 0)]), [(1, 1, 1)]);
        assert_eq!(changes(&old, &old), []);
    }

    #[test]
    fn dropped_from_the_front_then_arrived() {
        let old = [("a", 0), ("b", 0), ("c", 0), ("d", 0)];
        let new = [("c", 0), ("d", 0), ("e", 0)];
        assert_eq!(changes(&old, &new), [(0, 2, 0), (2, 0, 1)]);
    }

    #[test]
    fn near_the_end_is_at_the_end() {
        assert!(SCROLL_RULES.at_end(0.0));
        assert!(SCROLL_RULES.at_end(40.0));
        assert!(!SCROLL_RULES.at_end(41.0));
    }

    #[test]
    fn older_is_asked_for_two_screens_from_the_top() {
        assert!(SCROLL_RULES.wants_older(1599.0, 800.0));
        assert!(!SCROLL_RULES.wants_older(1601.0, 800.0));
    }
}
