//! How another session's intent meets the work of each session hearing it
//! ([`zirc_core::fit`]): the files each has changed under its paths, from this machine's
//! worktrees and other machines' working sets, and for a file both change, whether Git said
//! their lines are the same, as last predicted: of two worktrees here, or of one here and
//! the lines another machine's session said ([`zirc_proto::lines`]).
//!
//! Another machine's lines are of the commit its work is built on. Counted from that commit,
//! a worktree here built on another has what landed between the two as changes of its own
//! too; those hunks are the same ones the two commits' own diff has, so they're taken out,
//! and what's left is its own work, in the same lines' numbers as theirs. Without that
//! commit, as before fetching it, it can't tell.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use zirc_core::clashes::Meets;
use zirc_core::fit::{Fit, Shared, Verdict};
use zirc_core::hunks::{self, Hunk, Hunks};
use zirc_core::ipc::{Delivery, Response, Why};
use zirc_core::presence::{self, LocalOverlap, Overlap};
use zirc_proto::Event;

use super::{Daemon, Input, now};
use crate::git;

/// What Git last said of each pair of worktrees here that change some of the same files,
/// by their roots, sorted: of those files, the lines each changes the same, and the ones
/// each changes in different parts.
#[derive(Debug, Default)]
pub(super) struct Predictions {
    pairs: BTreeMap<[PathBuf; 2], Predicted>,
    /// The same of a worktree here and another machine's session.
    remote: BTreeMap<RemoteKey, Predicted>,
    /// For each pair, what its prediction was made from ([`stamp`]): it's made again only
    /// once that changes.
    stamps: BTreeMap<[PathBuf; 2], u64>,
}

/// Of the files two sides both change, the lines both change in each that has some, and
/// the files both change in different parts.
pub(crate) type Predicted = (BTreeMap<String, Vec<[u32; 2]>>, BTreeSet<String>);

/// A worktree here, by its root, and another machine's session, by its nick and session.
pub(crate) type RemoteKey = (PathBuf, String, String);

/// What a task predicting lines for intents' fits found.
#[derive(Debug)]
pub(crate) enum Found {
    /// Of worktrees here in a channel and other machines' sessions, from the lines those
    /// said ([`Daemon::predict_remote`]): nothing where what theirs are counted from isn't
    /// here.
    Remote(String, Vec<(RemoteKey, Option<Predicted>)>),
    /// Of pairs of worktrees here, with what each was made from ([`Daemon::refresh_pairs`]).
    Pairs(Vec<(u64, LocalOverlap)>),
}

/// What predicting a worktree here against another machine's lines takes.
struct Job {
    key: RemoteKey,
    base: String,
    files: Vec<String>,
    lines: zirc_proto::lines::Lines,
}

impl Predictions {
    /// Notes what was predicted of `overlap`'s worktrees, in place of what was before.
    pub(super) fn note(&mut self, overlap: &LocalOverlap) {
        let mut key = [
            overlap.worktrees[0].path.clone(),
            overlap.worktrees[1].path.clone(),
        ];
        key.sort();
        let apart = overlap.apart.iter().cloned().collect();
        self.pairs.insert(key, (overlap.same_lines.clone(), apart));
    }

    /// Forgets the pairs that the worktree at `root`, gone, was in.
    pub(super) fn forget(&mut self, root: &Path) {
        self.pairs.retain(|[a, b], _| a != root && b != root);
        self.stamps.retain(|[a, b], _| a != root && b != root);
        self.remote.retain(|(here, _, _), _| here != root);
    }

    /// Gives `overlap`, a worktree here's with another machine, what was last predicted of
    /// its paths against each of that machine's sessions: the lines both change, the paths
    /// changed apart, and the sessions compared.
    pub(super) fn dress(&self, overlap: &mut Overlap) {
        let Some(root) = overlap.worktree.as_ref().map(|worktree| &worktree.path) else {
            return;
        };
        let mut sessions = BTreeSet::new();
        for ((here, nick, session), (same, apart)) in &self.remote {
            if here != root || *nick != overlap.nick {
                continue;
            }
            for path in &overlap.paths {
                if let Some(lines) = same.get(path) {
                    let all = overlap.same_lines.entry(path.clone()).or_default();
                    all.extend(lines);
                    all.sort_unstable();
                    all.dedup();
                } else if apart.contains(path) {
                    overlap.apart.push(path.clone());
                } else {
                    continue;
                }
                sessions.insert(session.clone());
            }
        }
        overlap.apart.sort_unstable();
        overlap.apart.dedup();
        overlap
            .apart
            .retain(|path| !overlap.same_lines.contains_key(path));
        overlap.sessions = sessions.into_iter().collect();
    }

    /// How the worktree at `root` and `session` on `nick` both change `path`, as last
    /// predicted from the lines it said.
    fn shared_remote(&self, root: &Path, nick: &str, session: &str, path: &str) -> Shared {
        let key = (root.to_path_buf(), nick.to_owned(), session.to_owned());
        match self.remote.get(&key) {
            Some((same, _)) if same.contains_key(path) => Shared::SameLines(same[path].clone()),
            Some((_, apart)) if apart.contains(path) => Shared::Apart,
            _ => Shared::Unknown,
        }
    }

    /// How the worktrees at `a` and `b` both change `path`, as last predicted.
    fn shared(&self, a: &Path, b: &Path, path: &str) -> Shared {
        let mut key = [a.to_path_buf(), b.to_path_buf()];
        key.sort();
        match self.pairs.get(&key) {
            Some((same, _)) if same.contains_key(path) => Shared::SameLines(same[path].clone()),
            Some((_, apart)) if apart.contains(path) => Shared::Apart,
            _ => Shared::Unknown,
        }
    }
}

impl Daemon {
    /// `response` as told to `session` for `why`: an intent comes with how it meets the
    /// session's work.
    pub(super) fn dressed(
        &self,
        response: &Response,
        why: Option<Why>,
        session: Option<&str>,
    ) -> Response {
        let mut told = super::with_why(response.clone(), why);
        if why == Some(Why::Intent)
            && let (Response::Delivery(delivery), Some(session)) = (&mut told, session)
        {
            delivery.fit = self.intent_fit(delivery, session).map(Box::new);
        }
        // CI on a branch the session pushed, which zircd waits on for it: its hint is its
        // own build's, not one for a landing it said it waits on.
        if why == Some(Why::Awaited)
            && let (Response::Delivery(delivery), Some(session)) = (&mut told, session)
            && let zirc_proto::Event::Custom { kind, data, .. } = &delivery.message.event
            && kind == zirc_proto::ci::LANDING_KIND
            && let Some(branch) = zirc_proto::ci::Data::of(data).branch
        {
            delivery.own_branch = self.awaits.owns(session, &branch);
        }
        told
    }

    /// Whether `response`, another session's intent, needs nothing of `session` now by how
    /// it meets its work: all but one where both have changed the same lines of a file, or,
    /// from another machine, the same file, whose lines this one can't compare. Such an
    /// intent waits for the session's next wake rather than making one. A plan, with nothing
    /// changed under it yet, woke every session working in the folders it named, which then
    /// said there was nothing to do; and between this machine's own worktrees a file both
    /// change is compared as it changes, which wakes them if the lines meet
    /// ([`Daemon::predicted`]). The user's choice, 24 Nov 2026: "only actual range overlaps
    /// wake other agents".
    pub(super) fn quiet_intent(&self, response: &Response, session: &str) -> bool {
        let Response::Delivery(delivery) = response else {
            return false;
        };
        let local = self
            .client
            .as_ref()
            .is_some_and(|client| client.nick() == delivery.from);
        self.intent_fit(delivery, session)
            .is_some_and(|fit| match fit.verdict() {
                Verdict::SameLines => false,
                Verdict::SameFiles => local,
                _ => true,
            })
    }

    /// Whether `response`, a push that changes files `session` changes, needs nothing of it
    /// now: this machine made the push, so it has its commits and compares their lines with
    /// each worktree's own, waking the session if they meet (`scans.rs`, a conflict ahead);
    /// and it isn't a branch its own is built on. Such a push waits for the session's next wake, where its hint
    /// says to update before changing those files again. Every landing in a file a session
    /// changed woke it, 91 of a week's 216 wakes for others' work (20 to 25 Nov 2026), most
    /// for lines it never touched. The user's choice, 25 Nov 2026: "Note it, wake only for
    /// lines". A push heard from another machine still wakes: without its commits the lines
    /// can be compared only when this worktree is built on exactly what it was pushed onto.
    pub(super) fn quiet_landing(&self, response: &Response, session: &str) -> bool {
        let Response::Delivery(delivery) = response else {
            return false;
        };
        let Event::Push { branch, head, .. } = &delivery.message.event else {
            return false;
        };
        self.client
            .as_ref()
            .is_some_and(|client| client.nick() == delivery.from)
            && !self.awaits.moves_stack(session, branch, head)
    }

    /// Keeps for later, rather than waking them, the watching sessions whose reason in
    /// `reasons` is `held` and of whom `quiet` says `response` needs nothing now: an intent
    /// ([`Daemon::quiet_intent`]), a landing ([`Daemon::quiet_landing`]), or an overlap
    /// whose lines couldn't be compared ([`noted_overlap`]).
    pub(super) fn hold(
        &mut self,
        response: &Response,
        reasons: &mut [Option<Why>],
        held: Why,
        quiet: fn(&Self, &Response, &str) -> bool,
    ) {
        let quiet: Vec<String> = self
            .watchers
            .iter()
            .zip(reasons.iter_mut())
            .filter_map(|(watcher, why)| {
                let session = watcher.session.clone()?;
                if *why != Some(held) || !quiet(self, response, &session) {
                    return None;
                }
                *why = None;
                Some(session)
            })
            .collect();
        for session in quiet {
            let dressed = self.dressed(response, Some(held), Some(&session));
            self.keep_for_later(&session, &dressed, held);
        }
    }

    /// How `delivery`, another session's intent, meets the work of `session` here.
    pub(super) fn intent_fit(&self, delivery: &Delivery, session: &str) -> Option<Fit> {
        let Event::Intent { paths, .. } = &delivery.message.event else {
            return None;
        };
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())?;
        let home = delivery.scope();
        let channel = home.as_str();
        let intended: Vec<&str> = paths.iter().map(String::as_str).collect();
        let sender = delivery.message.session.as_str();
        let local = delivery.from == me;
        let theirs: BTreeSet<String> = if local {
            self.session_paths(channel, sender)
        } else {
            // Its uncommitted paths, and what its commits change that hasn't landed.
            let changing = self.presence.members(channel).into_iter().filter(|member| {
                member.nick == delivery.from && member.session.as_deref() == Some(sender)
            });
            let built = self.presence.built(channel).into_iter().filter(|built| {
                built.nick == delivery.from && built.session.as_deref() == Some(sender)
            });
            changing
                .flat_map(|member| member.paths)
                .chain(built.flat_map(|built| built.changed))
                .collect()
        };
        let theirs: Vec<String> = presence::touching(&theirs, &intended);
        let all_mine = self.session_paths(channel, session);
        let mine: Vec<String> = presence::touching(&all_mine, &intended);
        let named: Vec<String> = mine
            .iter()
            .filter(|path| paths.contains(path))
            .cloned()
            .collect();
        // Of their files, the ones this session itself said it's about to change, by name.
        let planned: BTreeSet<String> = self
            .presence
            .intents(channel, now())
            .into_iter()
            .filter(|intent| intent.nick == me && intent.session == session)
            .flat_map(|intent| intent.paths)
            .collect();
        let worktrees = |of: &str| -> Vec<PathBuf> {
            self.repos
                .iter()
                .filter(|(_, watched)| watched.is_of(of))
                .map(|(root, _)| root.clone())
                .collect()
        };
        let (ours, others) = (worktrees(session), worktrees(sender));
        let shared = theirs
            .iter()
            .filter(|path| mine.contains(path) || planned.contains(*path))
            .map(|path| {
                let how = if !mine.contains(path) {
                    Shared::Unknown
                } else if local {
                    ours.iter()
                        .flat_map(|a| others.iter().map(move |b| (a, b)))
                        .map(|(a, b)| self.predictions.shared(a, b, path))
                        .find(|how| *how != Shared::Unknown)
                        .unwrap_or(Shared::Unknown)
                } else {
                    ours.iter()
                        .map(|root| {
                            self.predictions
                                .shared_remote(root, &delivery.from, sender, path)
                        })
                        .find(|how| *how != Shared::Unknown)
                        .unwrap_or(Shared::Unknown)
                };
                (path.clone(), how)
            })
            .collect();
        Some(Fit {
            theirs,
            mine,
            shared,
            named,
        })
    }

    /// Predicts again, in a task of its own, each pair of worktrees in `channel` here that
    /// change some of the same files, where what they change there has changed since it
    /// was last predicted ([`Found::Pairs`]). Only overlaps that were news were
    /// predicted before, so a pair told of once kept its first prediction, or none: a
    /// quarter of intents between two sessions here said "lines unknown" (15 Nov 2026).
    pub(super) fn refresh_pairs(&mut self, channel: &str) {
        let local = self.local(channel);
        let mut due = Vec::new();
        for (i, (a, _, a_paths)) in local.iter().enumerate() {
            for (b, _, b_paths) in &local[i + 1..] {
                let shared: Vec<String> = a_paths.intersection(b_paths).cloned().collect();
                if shared.is_empty() || a.path == b.path {
                    continue;
                }
                let mut key = [a.path.clone(), b.path.clone()];
                key.sort();
                let stamp = self.stamp(&key, &shared);
                if self.predictions.stamps.get(&key) == Some(&stamp) {
                    continue;
                }
                let mut worktrees = [a.clone(), b.clone()];
                worktrees.sort();
                due.push((
                    stamp,
                    LocalOverlap {
                        channel: channel.to_owned(),
                        worktrees,
                        paths: shared,
                        same_lines: BTreeMap::new(),
                        apart: Vec::new(),
                        why: None,
                    },
                ));
            }
        }
        if due.is_empty() {
            return;
        }
        let inputs = self.inputs.clone();
        self.checking += 1;
        tokio::spawn(async move {
            for (_, overlap) in &mut due {
                super::predict(overlap).await;
            }
            let _ = inputs.send(Input::Fitted(Found::Pairs(due)));
        });
    }

    /// Notes what a task predicting lines found.
    pub(super) fn fitted(&mut self, found: Found) {
        self.checking = self.checking.saturating_sub(1);
        match found {
            Found::Remote(channel, predicted) => {
                for (key, it) in predicted {
                    match it {
                        Some(it) => self.predictions.remote.insert(key, it),
                        None => self.predictions.remote.remove(&key),
                    };
                }
                self.tell_remote(&channel);
            }
            Found::Pairs(predicted) => self.pairs_predicted(predicted),
        }
    }

    /// Notes what [`Daemon::refresh_pairs`] found.
    fn pairs_predicted(&mut self, predicted: Vec<(u64, LocalOverlap)>) {
        for (stamp, overlap) in predicted {
            self.predictions.note(&overlap);
            let mut key = [
                overlap.worktrees[0].path.clone(),
                overlap.worktrees[1].path.clone(),
            ];
            key.sort();
            self.predictions.stamps.insert(key, stamp);
        }
    }

    /// What a prediction of the worktrees at `key` for `shared` is made from: the files,
    /// and what each worktree is built on and changes in them.
    fn stamp(&self, key: &[PathBuf; 2], shared: &[String]) -> u64 {
        let mut hasher = DefaultHasher::new();
        shared.hash(&mut hasher);
        for root in key {
            if let Some(watched) = self.repos.get(root) {
                let snapshot = &watched.snapshot;
                (&snapshot.base, &snapshot.head).hash(&mut hasher);
                for path in shared {
                    snapshot.lines.get(path).hash(&mut hasher);
                    snapshot.dirty.contains(path).hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }

    /// Predicts, in a task of its own, each worktree in `channel` here against the lines
    /// each other machine's session there said, where both change some of the same files
    /// ([`Found::Remote`]). Whether there's any to predict: then what's found tells the
    /// overlaps ([`Daemon::tell_remote`]).
    pub(super) fn predict_remote(&mut self, channel: &str) -> bool {
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return false;
        };
        let said = self.presence.all_lines(channel);
        let jobs: Vec<Job> = self
            .repos
            .iter()
            .filter(|(_, watched)| watched.scope == channel)
            .filter_map(|(root, watched)| {
                let base = watched.snapshot.base.clone()?;
                let own: BTreeSet<&String> = watched
                    .snapshot
                    .dirty
                    .iter()
                    .chain(&watched.snapshot.committed)
                    .collect();
                Some((root, base, own))
            })
            .flat_map(|(root, base, own)| {
                said.iter()
                    .filter(|(nick, _, lines)| *nick != me && !lines.base.is_empty())
                    .filter_map(move |(nick, session, lines)| {
                        let files: Vec<String> = lines
                            .lines
                            .keys()
                            .filter(|path| own.contains(path))
                            .cloned()
                            .collect();
                        (!files.is_empty()).then(|| Job {
                            key: (root.clone(), nick.clone(), session.clone()),
                            base: base.clone(),
                            files,
                            lines: lines.clone(),
                        })
                    })
            })
            .collect();
        // A session that no longer changes a worktree's files has nothing predicted of it.
        let here: BTreeSet<&PathBuf> = self
            .repos
            .iter()
            .filter(|(_, watched)| watched.scope == channel)
            .map(|(root, _)| root)
            .collect();
        self.predictions
            .remote
            .retain(|key, _| !here.contains(&key.0) || jobs.iter().any(|job| job.key == *key));
        if jobs.is_empty() {
            return false;
        }
        let inputs = self.inputs.clone();
        let channel = channel.to_owned();
        self.checking += 1;
        tokio::spawn(async move {
            let mut predicted = Vec::new();
            for job in jobs {
                let it = predict_against(&job).await;
                predicted.push((job.key, it));
            }
            let _ = inputs.send(Input::Fitted(Found::Remote(channel, predicted)));
        });
        true
    }
}

/// Which of `job`'s files the worktree changes on the same lines as the other machine's
/// session, and which in different parts: its own work counted from the commit theirs is,
/// what landed between the two left out. `None` without that commit here.
async fn predict_against(job: &Job) -> Option<Predicted> {
    let root = job.key.0.as_path();
    let theirs_base = job.lines.base.as_str();
    if !git::has_commit(root, theirs_base).await {
        return None;
    }
    let counted = git::hunks(root, theirs_base, None, &job.files).await;
    let landed = git::hunks(root, theirs_base, Some(&job.base), &job.files).await;
    let ours = hunks::without(&counted, &landed);
    let theirs: Hunks = job
        .files
        .iter()
        .filter_map(|path| {
            let spans = job.lines.lines.get(path)?;
            Some((
                path.clone(),
                spans
                    .iter()
                    .map(|&[start, len]| Hunk::at(start, len))
                    .collect(),
            ))
        })
        .collect();
    let prediction = hunks::predict(&job.files, &ours, &theirs);
    Some((
        prediction.same_lines,
        prediction.apart.into_iter().collect(),
    ))
}

/// Whether `response` is an overlap whose lines couldn't be compared: a note for the
/// session's next wake, as nothing says they meet (the user's choice, 25 Nov 2026: "Note
/// it").
pub(super) fn noted_overlap(response: &Response) -> bool {
    super::announce::meets(response) == Some(Meets::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs git in `dir`, as a test author, and what it prints.
    fn run(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@zirc.dev"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn numbered(changed: &[(u32, &str)]) -> String {
        (1..=60)
            .map(|n| {
                changed
                    .iter()
                    .find(|(line, _)| *line == n)
                    .map_or_else(|| format!("line {n}\n"), |(_, text)| format!("{text}\n"))
            })
            .collect()
    }

    #[tokio::test]
    async fn counts_its_own_work_from_their_base_without_what_landed_since() {
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path();
        run(here, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(here.join("a.txt"), numbered(&[])).unwrap();
        run(here, &["add", "a.txt"]);
        run(here, &["commit", "--quiet", "-m", "start"]);
        let theirs_base = run(here, &["rev-parse", "HEAD"]);
        // Line 5 lands after their work was built; then this worktree changes line 50.
        std::fs::write(here.join("a.txt"), numbered(&[(5, "landed")])).unwrap();
        run(here, &["commit", "--quiet", "-am", "landed"]);
        let ours_base = run(here, &["rev-parse", "HEAD"]);
        std::fs::write(here.join("a.txt"), numbered(&[(5, "landed"), (50, "mine")])).unwrap();
        let job = |spans: Vec<[u32; 2]>| Job {
            key: (here.to_path_buf(), "beta".into(), "cc-b".into()),
            base: ours_base.clone(),
            files: vec!["a.txt".into()],
            lines: zirc_proto::lines::Lines {
                base: theirs_base[..7].to_owned(),
                continued: false,
                lines: [("a.txt".to_owned(), spans)].into(),
            },
        };
        let same = |spans| async move {
            let (same, apart) = predict_against(&job(spans)).await.unwrap();
            (same.contains_key("a.txt"), apart.contains("a.txt"))
        };
        // Theirs on line 50: the same lines.
        assert_eq!(same(vec![[50, 1]]).await, (true, false));
        // On line 5, which landed and isn't this worktree's own: elsewhere.
        assert_eq!(same(vec![[5, 1]]).await, (false, true));
        // On line 20: elsewhere.
        assert_eq!(same(vec![[20, 1]]).await, (false, true));
    }

    #[tokio::test]
    async fn without_their_base_it_cant_tell() {
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path();
        run(here, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(here.join("a.txt"), numbered(&[])).unwrap();
        run(here, &["add", "a.txt"]);
        run(here, &["commit", "--quiet", "-m", "start"]);
        let job = Job {
            key: (here.to_path_buf(), "beta".into(), "cc-b".into()),
            base: run(here, &["rev-parse", "HEAD"]),
            files: vec!["a.txt".into()],
            lines: zirc_proto::lines::Lines {
                base: "0123456".into(),
                continued: false,
                lines: [("a.txt".to_owned(), vec![[1, 1]])].into(),
            },
        };
        assert_eq!(predict_against(&job).await, None);
    }
}
