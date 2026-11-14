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
use std::path::{Path, PathBuf};

use zirc_core::fit::{Fit, Shared};
use zirc_core::hunks::{self, Hunk, Hunks};
use zirc_core::ipc::{Delivery, Response, Why};
use zirc_core::presence::{self, LocalOverlap};
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
}

/// Of the files two sides both change, the lines both change in each that has some, and
/// the files both change in different parts.
pub(super) type Predicted = (BTreeMap<String, Vec<[u32; 2]>>, BTreeSet<String>);

/// A worktree here, by its root, and another machine's session, by its nick and session.
pub(super) type RemoteKey = (PathBuf, String, String);

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
        self.remote.retain(|(here, _, _), _| here != root);
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
        told
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
        let home = self.home_channel(&delivery.channel);
        let channel = home.as_str();
        let intended: Vec<&str> = paths.iter().map(String::as_str).collect();
        let sender = delivery.message.session.as_str();
        let local = delivery.from == me;
        let theirs: BTreeSet<String> = if local {
            self.session_paths(channel, sender)
        } else {
            self.presence
                .members(channel)
                .into_iter()
                .filter(|member| {
                    member.nick == delivery.from && member.session.as_deref() == Some(sender)
                })
                .flat_map(|member| member.paths)
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

    /// Predicts, in a task of its own, each worktree in `channel` here against the lines
    /// each other machine's session there said, where both change some of the same files
    /// ([`Input::RemotePredicted`]).
    pub(super) fn predict_remote(&mut self, channel: &str) {
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return;
        };
        let said = self.presence.all_lines(channel);
        let jobs: Vec<Job> = self
            .repos
            .iter()
            .filter(|(_, watched)| watched.channel == channel)
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
        if jobs.is_empty() {
            return;
        }
        let inputs = self.inputs.clone();
        self.checking += 1;
        tokio::spawn(async move {
            let mut predicted = Vec::new();
            for job in jobs {
                if let Some(it) = predict_against(&job).await {
                    predicted.push((job.key, it));
                }
            }
            let _ = inputs.send(Input::RemotePredicted(predicted));
        });
    }

    /// Notes what [`Daemon::predict_remote`] found.
    pub(super) fn remote_predicted(&mut self, predicted: Vec<(RemoteKey, Predicted)>) {
        self.checking = self.checking.saturating_sub(1);
        for (key, it) in predicted {
            self.predictions.remote.insert(key, it);
        }
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
