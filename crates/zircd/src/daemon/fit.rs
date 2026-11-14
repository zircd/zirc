//! How another session's intent meets the work of each session hearing it
//! ([`zirc_core::fit`]): the files each has changed under its paths, from this machine's
//! worktrees and other machines' working sets, and for a file two worktrees here both
//! change, whether Git said their lines are the same, as last predicted.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use zirc_core::fit::{Fit, Shared};
use zirc_core::ipc::{Delivery, Response, Why};
use zirc_core::presence::{self, LocalOverlap};
use zirc_proto::Event;

use super::{Daemon, now};

/// What Git last said of each pair of worktrees here that change some of the same files,
/// by their roots, sorted: of those files, the lines each changes the same, and the ones
/// each changes in different parts.
#[derive(Debug, Default)]
pub(super) struct Predictions {
    pairs: BTreeMap<[PathBuf; 2], Predicted>,
}

/// Of the files a pair of worktrees both change, the lines both change in each that has
/// some, and the files both change in different parts.
type Predicted = (BTreeMap<String, Vec<[u32; 2]>>, BTreeSet<String>);

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
                let how = if local && mine.contains(path) {
                    ours.iter()
                        .flat_map(|a| others.iter().map(move |b| (a, b)))
                        .map(|(a, b)| self.predictions.shared(a, b, path))
                        .find(|how| *how != Shared::Unknown)
                        .unwrap_or(Shared::Unknown)
                } else {
                    Shared::Unknown
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
}
