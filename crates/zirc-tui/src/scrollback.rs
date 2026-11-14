//! The scrollback's window on the conversation shown. Only what it shows is laid out, with
//! what's needed around it to scroll and to know how near the top it is, and each item's
//! rows are kept, by what it is and the width, until it changes, so a frame wraps nothing
//! it wrapped before. Where it's read from, while it isn't at the end, is held to an item
//! and a row of it, not a count of rows from the end: messages arriving below and older
//! history coming in above leave what's read where it was.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher as _};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use zirc_core::chat::{Item, SCROLL_RULES};

use crate::rows::{self, Look, Row};

/// How many items' rows are kept before they're all let go, well above the most a
/// conversation holds.
const KEPT: usize = 16_384;

/// Where the scrollback of a conversation is read from while it isn't at the end: its top
/// row is `row` rows on from the first of the item `key` names, before it when negative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    key: Key,
    row: isize,
}

/// An item, as found again however many come in before or after it: by its message's id,
/// or else by its time and how many before it have the same.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    id: Option<String>,
    time: Option<DateTime<Utc>>,
    nth: usize,
}

impl Key {
    /// The item `at` of `items`.
    fn of(items: &[Item<'_>], at: usize) -> Self {
        let item = &items[at];
        let time = item.time();
        Self {
            id: rows::entry_id(item).map(str::to_owned),
            time,
            nth: items[..at]
                .iter()
                .rev()
                .take_while(|before| before.time() == time)
                .count(),
        }
    }

    /// Where the item is among `items`, or, gone, the first after when it was.
    fn find(&self, items: &[Item<'_>]) -> Option<usize> {
        if let Some(id) = &self.id
            && let Some(at) = items
                .iter()
                .position(|item| rows::entry_id(item) == Some(id))
        {
            return Some(at);
        }
        let time = self.time?;
        let first = items
            .iter()
            .position(|item| item.time().is_some_and(|at| at >= time))?;
        let same = items[first..]
            .iter()
            .take_while(|item| item.time() == Some(time))
            .count();
        Some(first + self.nth.min(same.saturating_sub(1)))
    }
}

/// A row of the conversation: the item, and the row of it.
pub type Place = (usize, usize);

/// The rows items were laid out in, by what each was and everything they were laid out by,
/// as [`rows::fingerprint`] says, for as long as what [`Cache::stamp`] covers stays the same.
#[derive(Debug, Default)]
pub struct Cache {
    stamp: u64,
    rows: HashMap<u64, Arc<[Row]>>,
    /// How many items were laid out afresh, rather than found here: for tests.
    pub laid: usize,
}

impl Cache {
    /// Starts over unless `stamp`, everything every item is laid out by that its
    /// fingerprint doesn't cover, is what it was.
    pub fn stamp(&mut self, stamp: impl Hash) {
        let mut hasher = DefaultHasher::new();
        stamp.hash(&mut hasher);
        let stamp = hasher.finish();
        if stamp != self.stamp || self.rows.len() > KEPT {
            self.stamp = stamp;
            self.rows.clear();
        }
    }
}

/// A conversation's items, laid out as `look` says when they're looked at, with `cache`.
pub struct Laid<'a, 'c> {
    look: Look<'a>,
    items: Vec<Item<'a>>,
    cache: &'c RefCell<Cache>,
}

/// What the scrollback shows: its rows from the top, `None` for the blank ones above the
/// oldest; where its top row is; and whether it shows the end.
pub struct View {
    /// The rows, as many as it's high.
    pub rows: Vec<Option<Row>>,
    /// Where the top row shown is, unless nothing is.
    pub top: Option<Place>,
    /// Whether the newest row is the bottom one.
    pub at_end: bool,
}

impl<'a, 'c> Laid<'a, 'c> {
    /// `items` laid out as `look` says, kept in `cache`.
    #[must_use]
    pub fn new(look: Look<'a>, items: Vec<Item<'a>>, cache: &'c RefCell<Cache>) -> Self {
        Self { look, items, cache }
    }

    /// The items.
    #[must_use]
    pub fn items(&self) -> &[Item<'a>] {
        &self.items
    }

    /// The rows of the item `at`.
    #[must_use]
    pub fn rows(&self, at: usize) -> Arc<[Row]> {
        let item = &self.items[at];
        let before = at.checked_sub(1).map(|before| &self.items[before]);
        let fingerprint = rows::fingerprint(&self.look, item, before);
        if let Some(rows) = self.cache.borrow().rows.get(&fingerprint) {
            return Arc::clone(rows);
        }
        let rows: Arc<[Row]> = rows::item_rows(&self.look, item, before).into();
        let mut cache = self.cache.borrow_mut();
        cache.laid += 1;
        cache.rows.insert(fingerprint, Arc::clone(&rows));
        rows
    }

    /// Every row, oldest first.
    #[must_use]
    pub fn all(&self) -> Vec<Row> {
        (0..self.items.len())
            .flat_map(|at| self.rows(at).to_vec())
            .collect()
    }

    /// Where `anchor` is now, if what it names, or something after it, is still here.
    #[must_use]
    pub fn place(&self, anchor: &Anchor) -> Option<Place> {
        let at = anchor.key.find(&self.items)?;
        Some(self.step((at, 0), anchor.row))
    }

    /// An anchor for `place`, on the first item at or after it with a time, so it's found
    /// again as items come and go around it: a day's divider has none.
    #[must_use]
    pub fn anchor(&self, (at, row): Place) -> Anchor {
        let signed = |rows: usize| isize::try_from(rows).unwrap_or(isize::MAX);
        let timed = (at..self.items.len()).find(|&i| self.items[i].time().is_some());
        match timed {
            Some(item) if item > at => {
                let between: usize = (at..item).map(|i| self.rows(i).len()).sum();
                Anchor {
                    key: Key::of(&self.items, item),
                    row: -signed(between.saturating_sub(row)),
                }
            }
            Some(item) => Anchor {
                key: Key::of(&self.items, item),
                row: signed(row),
            },
            None => Anchor {
                key: Key {
                    id: None,
                    time: None,
                    nth: 0,
                },
                row: 0,
            },
        }
    }

    /// `by` rows on from `place`, towards the newest, or the oldest when negative: no
    /// further than the first row, or than one past the last, `(items, 0)`.
    #[must_use]
    pub fn step(&self, (mut at, mut row): Place, by: isize) -> Place {
        let count = self.items.len();
        if at < count {
            row = row.min(self.rows(at).len().saturating_sub(1));
        }
        let mut left = by.unsigned_abs();
        if by >= 0 {
            while at < count {
                let len = self.rows(at).len();
                if row + left < len {
                    return (at, row + left);
                }
                left -= len.saturating_sub(row);
                at += 1;
                row = 0;
            }
            return (count, 0);
        }
        loop {
            if left <= row && at < count {
                return (at, row - left);
            }
            left -= (row + 1).min(left);
            let Some(before) = (0..at).rev().find(|&i| !self.rows(i).is_empty()) else {
                return self.first_from(0);
            };
            at = before;
            row = self.rows(at).len() - 1;
            if left == 0 {
                return (at, row);
            }
        }
    }

    /// The first row from the item `at` on that there is.
    fn first_from(&self, at: usize) -> Place {
        (at..self.items.len())
            .find(|&i| !self.rows(i).is_empty())
            .map_or((self.items.len(), 0), |i| (i, 0))
    }

    /// What `height` rows show from `top`, or, with none or from too near the end to fill
    /// them, the newest `height` rows.
    #[must_use]
    pub fn view(&self, top: Option<Place>, height: usize) -> View {
        if let Some(top) = top {
            let rows = self.down_from(top, height);
            if rows.len() == height {
                return View {
                    rows: rows.into_iter().map(Some).collect(),
                    top: Some(top),
                    at_end: false,
                };
            }
        }
        // The newest rows, read back from the end.
        let mut rows: Vec<Row> = Vec::with_capacity(height);
        let mut top = None;
        'items: for at in (0..self.items.len()).rev() {
            let laid = self.rows(at);
            for (row, laid) in laid.iter().enumerate().rev() {
                if rows.len() == height {
                    break 'items;
                }
                rows.push(laid.clone());
                top = Some((at, row));
            }
        }
        let mut shown = vec![None; height - rows.len()];
        shown.extend(rows.into_iter().rev().map(Some));
        View {
            rows: shown,
            top,
            at_end: true,
        }
    }

    /// Up to `count` rows from `place` on.
    fn down_from(&self, (at, row): Place, count: usize) -> Vec<Row> {
        let mut rows = Vec::with_capacity(count);
        for item in at..self.items.len() {
            let laid = self.rows(item);
            let skip = if item == at { row } else { 0 };
            for laid in laid.iter().skip(skip) {
                if rows.len() == count {
                    return rows;
                }
                rows.push(laid.clone());
            }
        }
        rows
    }

    /// How many rows there are from `place` to the end, counting them all: for tests.
    #[must_use]
    pub fn to_end(&self, (at, row): Place) -> usize {
        (at..self.items.len())
            .map(|item| self.rows(item).len())
            .sum::<usize>()
            .saturating_sub(row)
    }

    /// Whether, `top` being the top row of a scrollback `height` rows high, the page of
    /// history before should be asked for, as zirc-chat's [`SCROLL_RULES`] say: within two
    /// screenfuls of the top, a row counting as a point. Lays out no more above it than
    /// that takes.
    #[must_use]
    pub fn wants_older(&self, (at, row): Place, height: usize) -> bool {
        let rows = |count: usize| f32::from(u16::try_from(count).unwrap_or(u16::MAX));
        let mut above = row;
        for item in (0..at).rev() {
            if !SCROLL_RULES.wants_older(rows(above), rows(height)) {
                return false;
            }
            above += self.rows(item).len();
        }
        SCROLL_RULES.wants_older(rows(above), rows(height))
    }
}
