//! Each worktree's working set said alone as it changes ([`zirc_proto::ALONE`]), between the
//! machine's whole sets, which go every [`super::Daemon::announce_every`] at most. A whole set
//! is a line for every worktree with changes, ten on a busy machine, so it waits minutes; a
//! worktree's own is one line, so it goes [`UPDATE_EVERY`] after the channel last heard this
//! machine's sets, whole or alone, and readers' overlaps are that fresh.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use zirc_core::ipc::Built;
use zirc_proto::Event;

use super::{Daemon, WATCHER_SESSION, worktree_name};

/// How often at most a channel hears a worktree's set alone: as long as another machine may
/// wait to hear of an edit, which tests that wait for one allow for.
pub const UPDATE_EVERY: Duration = Duration::from_secs(15);

/// Where a set is on this machine: its worktree's name and branch.
type Place = (Option<String>, Option<String>);

/// What a machine says of one place's set: a [`Event::Working`] but for where it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Said {
    /// Whose it is.
    session: String,
    /// The paths with uncommitted changes.
    paths: BTreeSet<String>,
    /// What its work is built on.
    base: Option<String>,
    /// The paths its own commits change, beyond `paths`.
    committed: BTreeSet<String>,
    /// The lines of `base` its work changes, file by file.
    lines: BTreeMap<String, Vec<[u32; 2]>>,
}

impl Said {
    /// The event that says it of the set at `place`, as part `part`.
    pub(super) fn event(self, (worktree, branch): Place, continued: bool, part: [u32; 2]) -> Event {
        Event::Working {
            paths: self.paths.into_iter().collect(),
            continued,
            worktree,
            branch,
            part: Some(part),
            base: self.base.as_deref().map(said_base),
            committed: self.committed.into_iter().collect(),
        }
    }

    /// What says the lines it changes ([`zirc_proto::lines`]), when it's built on a known
    /// commit: none of a set without one. A set that's gone says none, which clears what
    /// was said.
    pub(super) fn lines(&self) -> Option<Event> {
        let gone = self.paths.is_empty() && self.committed.is_empty();
        let base = match &self.base {
            Some(base) => said_base(base),
            None if gone => String::new(),
            None => return None,
        };
        Some(
            zirc_proto::lines::Lines {
                base,
                continued: false,
                lines: self.lines.clone(),
            }
            .event(),
        )
    }

    /// Whose it is.
    pub(super) fn session(&self) -> &str {
        &self.session
    }

    /// It as `zirc who` and `zirc sessions` have it of the machine `nick`, on `branch`:
    /// `None` when what it's built on isn't known.
    pub(super) fn built(self, nick: &str, branch: Option<String>) -> Option<Built> {
        Some(Built {
            nick: nick.to_owned(),
            session: (self.session != WATCHER_SESSION).then_some(self.session),
            branch,
            base: said_base(&self.base?),
            changed: self.paths.union(&self.committed).cloned().collect(),
        })
    }
}

/// A commit as a working set says what it's built on: the start of its id.
fn said_base(commit: &str) -> String {
    commit.chars().take(zirc_proto::BASE_DIGITS).collect()
}

/// What each place's set is.
type Sets = BTreeMap<Place, Said>;

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

/// The places whose sets differ between `told` and `now`, with what each is now: nothing
/// but whose it was for one that's gone.
fn changes(told: &Sets, now: &Sets) -> Vec<(Place, Said)> {
    let mut changes: Vec<_> = now
        .iter()
        .filter(|(place, set)| told.get(*place) != Some(*set))
        .map(|(place, set)| (place.clone(), set.clone()))
        .collect();
    changes.extend(
        told.iter()
            .filter(|(place, _)| !now.contains_key(*place))
            .map(|(place, set)| {
                let gone = Said {
                    session: set.session.clone(),
                    ..Said::default()
                };
                (place.clone(), gone)
            }),
    );
    changes
}

impl Daemon {
    /// What this machine says of its sets in `channel` now, a worktree at a time: those
    /// with uncommitted changes or commits of their own.
    pub(super) fn sets_said(&self, channel: &str) -> Vec<(Place, Said)> {
        self.repos
            .iter()
            .filter(|(_, watched)| watched.scope == channel)
            .flat_map(|(root, watched)| {
                watched.sets().into_iter().map(|set| {
                    let said = Said {
                        session: set.session.unwrap_or(WATCHER_SESSION).to_owned(),
                        paths: set.paths,
                        base: set.base,
                        committed: set.committed,
                        lines: set.lines,
                    };
                    ((worktree_name(root), set.branch), said)
                })
            })
            .filter(|(_, said)| !said.paths.is_empty() || !said.committed.is_empty())
            .collect()
    }

    /// This machine's sets in `channel` as they are now, by place.
    pub(super) fn sets_now(&self, channel: &str) -> Sets {
        self.sets_said(channel).into_iter().collect()
    }

    /// Tells `channel` the lines `said`, `session`'s set, changes, in as many parts as it
    /// takes. Unsigned, without the session's name, tool and picture: its working set says
    /// those, and a line has room for a file's lines only without them.
    pub(super) fn publish_lines(&mut self, channel: &str, session: &str, said: &Said) {
        let Some(event) = said.lines() else {
            return;
        };
        let parts =
            zirc_core::send::fitting_as(&zirc_proto::Message::new(session, event.clone()), event);
        for part in parts {
            self.publish_message(channel, zirc_proto::Message::new(session, part));
        }
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
            let changed = changes(told, &sets);
            let lines: Vec<(String, Said)> = changed
                .iter()
                .map(|(_, said)| (said.session().to_owned(), said.clone()))
                .collect();
            let events: Vec<(String, Event)> = changed
                .into_iter()
                .map(|(place, said)| {
                    let session = said.session().to_owned();
                    // What it's built on rides along where it fits the line; where it
                    // doesn't, the paths go without it, and the whole sets say it.
                    let paths_alone = Said {
                        base: None,
                        committed: BTreeSet::new(),
                        ..said.clone()
                    };
                    let event = said.event(place.clone(), false, zirc_proto::ALONE);
                    let whole = self.signed(&session, event.clone());
                    let fits = zirc_core::send::fitting_as(&whole, event.clone()).len() == 1;
                    if fits || paths_alone.paths.is_empty() {
                        (session, event)
                    } else {
                        (session, paths_alone.event(place, false, zirc_proto::ALONE))
                    }
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
            // Lines first, as when the sets are told whole ([`Daemon::announce`]).
            for (session, said) in lines {
                self.publish_lines(&channel, &session, &said);
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
                let said = Said {
                    session: (*session).to_owned(),
                    paths: paths.iter().map(|&path| path.to_owned()).collect(),
                    ..Said::default()
                };
                (place, said)
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
        let said = |session: &str, paths: &[&str]| Said {
            session: session.to_owned(),
            paths: paths.iter().map(|&p| p.to_owned()).collect(),
            ..Said::default()
        };
        assert_eq!(
            changes(&told, &now),
            vec![
                (place("three"), said("cc-3", &["c.rs"])),
                (place("two"), said("cc-2", &["b.rs", "d.rs"])),
            ]
        );
        let gone = sets(&[("one", "cc-1", &["a.rs"])]);
        assert_eq!(
            changes(&told, &gone),
            vec![(place("two"), said("cc-2", &[]))]
        );
        assert!(changes(&told, &told).is_empty());
        // A worktree that committed its work, or was rebased, is said again.
        let mut rebased = told.clone();
        let two = rebased.get_mut(&place("two")).unwrap();
        two.base = Some("b".repeat(40));
        two.committed = ["e.rs".to_owned()].into();
        assert_eq!(
            changes(&told, &rebased),
            vec![(place("two"), rebased[&place("two")].clone())]
        );
    }
}
