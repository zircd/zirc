//! Overlaps two sessions settled, as `zirc settle` says them ([`zirc_proto::woke::Settled`]):
//! one line in the agents' room naming both, the files and who lands first, for people to
//! see that agents sorted it out (the user's decision, 24 Nov 2026: agents say it, as zircd
//! can't read an order from their words). Said once: the side that says it second is told
//! the other already did. A settled overlap between two of this machine's sessions is no
//! news to either again while it's remembered, as a split said in an intent isn't
//! ([`Daemon::agreed`]).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use zirc_core::ipc::Response;
use zirc_core::presence::{self, LocalOverlap};
use zirc_proto::Message;
use zirc_proto::woke::{Settled, Side};

use super::{Daemon, git, now};

/// How long a settled overlap is remembered, in seconds
/// ([`zirc_core::clashes::SETTLED_FOR`]).
const REMEMBERED: u64 = zirc_core::clashes::SETTLED_FOR;

/// How many are remembered at most.
const KEPT: usize = 64;

/// The overlaps settled lately, here or as heard, each with when and the scope it's in.
#[derive(Debug, Default)]
pub(super) struct SettledLately(VecDeque<(u64, String, Settled)>);

impl SettledLately {
    fn keep(&mut self, scope: &str, settled: Settled, at: u64) {
        self.0
            .retain(|(then, _, _)| at.saturating_sub(*then) < REMEMBERED);
        if self.0.len() == KEPT {
            self.0.pop_front();
        }
        self.0.push_back((at, scope.to_owned(), settled));
    }

    /// The one between `a` and `b` in `scope` that covers every one of `paths`, if it's
    /// still remembered at `at`.
    fn covering(
        &self,
        scope: &str,
        (a, b): ((&str, &str), (&str, &str)),
        paths: &[String],
        at: u64,
    ) -> Option<&Settled> {
        self.0
            .iter()
            .filter(|(then, said_in, _)| at.saturating_sub(*then) < REMEMBERED && said_in == scope)
            .map(|(_, _, settled)| settled)
            .find(|settled| {
                let agreed: Vec<&str> = settled.paths.iter().map(String::as_str).collect();
                settled.between(a, b) && presence::touching(paths, &agreed).len() == paths.len()
            })
    }
}

impl Daemon {
    /// Says, as `session`, that it and the session `with` names settled their overlap in
    /// `paths` ([`zirc_core::ipc::Request::Settle`]): `first` lands first, `me`, `them` or
    /// either as it's named.
    pub(super) async fn settle(
        &mut self,
        repo: &Path,
        session: &str,
        with: &str,
        first: &str,
        paths: &[PathBuf],
        same_lines: bool,
    ) -> Result<Response> {
        let scope = self.watched(repo).await?.scope.clone();
        let root = git::root(repo).await?;
        let base = repo.canonicalize().unwrap_or_else(|_| repo.to_owned());
        let paths = paths
            .iter()
            .map(|path| git::relative(&root, &base, path))
            .collect::<Result<Vec<String>>>()?;
        anyhow::ensure!(!paths.is_empty(), "name the files both of you change");
        let me = self.connected()?.nick().to_owned();
        let known = self.known_sessions();
        let address = zirc_core::notes::resolve(with, &known)?;
        // The session by its id, as its machine knows it, and what it's called.
        let them = known
            .iter()
            .find(|known| {
                known.machine.eq_ignore_ascii_case(&address.machine)
                    && (known.session == address.session
                        || known.name.as_deref() == Some(address.session.as_str()))
            })
            .map_or_else(
                || Side {
                    machine: address.machine.clone(),
                    session: address.session.clone(),
                    name: None,
                },
                |known| Side {
                    machine: known.machine.clone(),
                    session: known.session.clone(),
                    name: known.name.clone(),
                },
            );
        let mine = Side {
            machine: me,
            session: session.to_owned(),
            name: self.name_of(session),
        };
        anyhow::ensure!(
            !them.is(&mine.machine, &mine.session),
            "that's this session: name the other one with --with"
        );
        let names = |side: &Side| {
            first == side.session
                || side.name.as_deref() == Some(first)
                || first == format!("{}/{}", side.machine, side.session)
        };
        let i_first = if first == "me" || names(&mine) {
            true
        } else if first == "them" || names(&them) {
            false
        } else {
            anyhow::bail!("--first is me, them, or one of the two sessions: not {first:?}");
        };
        let (first, second) = if i_first { (mine, them) } else { (them, mine) };
        let sides = (
            (first.machine.as_str(), first.session.as_str()),
            (second.machine.as_str(), second.session.as_str()),
        );
        if let Some(said) = self.settled_lately.covering(&scope, sides, &paths, now()) {
            return Ok(Response::Settled {
                text: said.event().human_text(),
                said: false,
            });
        }
        let settled = Settled {
            first,
            second,
            paths,
            same_lines,
        };
        let event = settled.event();
        let text = event.human_text();
        let message = Message {
            level: Some(zirc_proto::Level::Info),
            ..self.signed(session, event)
        };
        self.publish_out(&scope, message, None, None)
            .context("the line wasn't said")?;
        Ok(Response::Settled { text, said: true })
    }

    /// Remembers the settled overlap `response` says, if it's one, heard or said here.
    pub(super) fn settled_heard(&mut self, response: &Response) {
        if let Response::Delivery(delivery) = response
            && let Some(settled) = Settled::of(&delivery.message.event)
        {
            self.settled_lately.keep(&delivery.scope(), settled, now());
        }
    }

    /// Whether `session` on `me` and the session on the other side of `overlap` settled it
    /// lately, all its paths.
    pub(super) fn settled(&self, overlap: &LocalOverlap, me: &str, session: &str) -> bool {
        overlap
            .worktrees
            .iter()
            .filter(|worktree| !self.works_in(worktree, session))
            .filter_map(|worktree| self.session_of(worktree))
            .any(|other| {
                self.settled_lately
                    .covering(
                        &overlap.channel,
                        ((me, session), (me, other)),
                        &overlap.paths,
                        now(),
                    )
                    .is_some()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(session: &str) -> Side {
        Side {
            machine: "ada-mbp".into(),
            session: session.into(),
            name: None,
        }
    }

    #[test]
    fn covers_the_same_two_and_all_the_paths_while_remembered() {
        let mut lately = SettledLately::default();
        let settled = Settled {
            first: side("cc-1"),
            second: side("cc-2"),
            paths: vec!["web/src".into()],
            same_lines: false,
        };
        lately.keep("#web", settled, 100);
        let pair = (("ada-mbp", "cc-2"), ("ada-mbp", "cc-1"));
        let inside = ["web/src/a.rs".to_owned()];
        assert!(lately.covering("#web", pair, &inside, 200).is_some());
        // Another repository, another pair, a path it didn't name, or too long ago.
        assert!(lately.covering("#api", pair, &inside, 200).is_none());
        let other = (("ada-mbp", "cc-2"), ("ada-mbp", "cc-3"));
        assert!(lately.covering("#web", other, &inside, 200).is_none());
        let outside = ["web/src/a.rs".to_owned(), "docs/a.md".to_owned()];
        assert!(lately.covering("#web", pair, &outside, 200).is_none());
        assert!(
            lately
                .covering("#web", pair, &inside, 100 + REMEMBERED)
                .is_none()
        );
    }
}
