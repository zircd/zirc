//! Each worktree's working set said alone as it changes ([`zirc_proto::ALONE`]), between the
//! machine's whole sets, which go every [`super::Daemon::announce_every`] at most. A whole set
//! is a line for every worktree with changes, ten on a busy machine, so it waits minutes; a
//! worktree's own is one line, so it goes [`UPDATE_EVERY`] after the channel last heard this
//! machine's sets, whole or alone, and readers' overlaps are that fresh.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use zirc_proto::Event;

use super::{Daemon, WATCHER_SESSION, worktree_name};

/// How often at most a channel hears a worktree's set alone.
const UPDATE_EVERY: Duration = Duration::from_secs(15);

/// Where a set is on this machine: its worktree's name and branch.
type Place = (Option<String>, Option<String>);

/// What each place's set is: whose it is and its paths.
type Sets = BTreeMap<Place, (String, BTreeSet<String>)>;

/// What each channel was last told of this machine's sets, and which channels are due.
#[derive(Debug, Default)]
pub(super) struct WorktreeUpdates {
    /// By channel: what it was last told, in a whole set or alone, and when.
    told: BTreeMap<String, (Sets, Instant)>,
    /// The channels whose sets have changed since they were last told.
    due: BTreeSet<String>,
}

impl WorktreeUpdates {
    /// Notes that this machine's sets in `channel` may have changed.
    pub(super) fn changed(&mut self, channel: &str) {
        self.due.insert(channel.to_owned());
    }

    /// Notes that `channel` was told `sets` whole at `at`.
    fn told_whole(&mut self, channel: &str, sets: Sets, at: Instant) {
        self.told.insert(channel.to_owned(), (sets, at));
    }
}

/// The places whose sets differ between `told` and `now`, with what each is now: no paths
/// for one that's gone.
fn changes(told: &Sets, now: &Sets) -> Vec<(Place, String, BTreeSet<String>)> {
    let mut changes: Vec<_> = now
        .iter()
        .filter(|(place, set)| told.get(*place) != Some(*set))
        .map(|(place, (session, paths))| (place.clone(), session.clone(), paths.clone()))
        .collect();
    changes.extend(
        told.iter()
            .filter(|(place, _)| !now.contains_key(*place))
            .map(|(place, (session, _))| (place.clone(), session.clone(), BTreeSet::new())),
    );
    changes
}

impl Daemon {
    /// This machine's sets in `channel` as they are now, by place.
    pub(super) fn sets_now(&self, channel: &str) -> Sets {
        self.local(channel)
            .into_iter()
            .filter(|(_, _, paths)| !paths.is_empty())
            .map(|(worktree, session, paths)| {
                let place = (worktree_name(&worktree.path), worktree.branch);
                let session = session.unwrap_or_else(|| WATCHER_SESSION.to_owned());
                (place, (session, paths))
            })
            .collect()
    }

    /// Notes that `channel` was just told this machine's sets whole.
    pub(super) fn told_whole(&mut self, channel: &str) {
        let sets = self.sets_now(channel);
        self.worktree_updates
            .told_whole(channel, sets, self.clock.now());
    }

    /// Tells the channels due each worktree's changed set alone, where it's been
    /// [`UPDATE_EVERY`] since they last heard one. A channel never told the whole sets, or
    /// one whose change doesn't fit a line, waits for the whole sets instead.
    pub(super) fn update_due(&mut self, now: Instant) {
        let due: Vec<String> = self.worktree_updates.due.iter().cloned().collect();
        for channel in due {
            let Some((told, at)) = self.worktree_updates.told.get(&channel) else {
                self.worktree_updates.due.remove(&channel);
                continue;
            };
            if now.duration_since(*at) < UPDATE_EVERY {
                continue;
            }
            self.worktree_updates.due.remove(&channel);
            let sets = self.sets_now(&channel);
            let events: Vec<(String, Event)> = changes(told, &sets)
                .into_iter()
                .map(|((worktree, branch), session, paths)| {
                    let event = Event::Working {
                        paths: paths.into_iter().collect(),
                        continued: false,
                        worktree,
                        branch,
                        part: Some(zirc_proto::ALONE),
                    };
                    (session, event)
                })
                .collect();
            if events.is_empty() {
                continue;
            }
            let fits = events.iter().all(|(session, event)| {
                zirc_core::send::fitting_as(&self.signed(session, event.clone()), event.clone())
                    .len()
                    == 1
            });
            if !fits {
                continue;
            }
            for (session, event) in events {
                self.publish(&channel, &session, event);
            }
            self.worktree_updates
                .told
                .insert(channel.clone(), (sets, now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets(entries: &[(&str, &str, &[&str])]) -> Sets {
        entries
            .iter()
            .map(|(worktree, session, paths)| {
                let place = (Some((*worktree).to_owned()), Some(format!("{worktree}-b")));
                let paths = paths.iter().map(|&path| path.to_owned()).collect();
                (place, ((*session).to_owned(), paths))
            })
            .collect()
    }

    #[test]
    fn says_only_the_places_that_changed_and_those_gone() {
        let told = sets(&[("one", "cc-1", &["a.rs"]), ("two", "cc-2", &["b.rs"])]);
        let now = sets(&[
            ("one", "cc-1", &["a.rs"]),
            ("three", "cc-3", &["c.rs"]),
            ("two", "cc-2", &["b.rs", "d.rs"]),
        ]);
        let place = |name: &str| (Some(name.to_owned()), Some(format!("{name}-b")));
        let paths = |paths: &[&str]| paths.iter().map(|&p| p.to_owned()).collect();
        assert_eq!(
            changes(&told, &now),
            vec![
                (place("three"), "cc-3".to_owned(), paths(&["c.rs"])),
                (place("two"), "cc-2".to_owned(), paths(&["b.rs", "d.rs"])),
            ]
        );
        let gone = sets(&[("one", "cc-1", &["a.rs"])]);
        assert_eq!(
            changes(&told, &gone),
            vec![(place("two"), "cc-2".to_owned(), BTreeSet::new())]
        );
        assert!(changes(&told, &told).is_empty());
    }
}
