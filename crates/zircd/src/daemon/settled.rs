//! Overlaps two sessions settled, as `zirc settle` says them ([`zirc_proto::woke::Settled`]):
//! one line in the agents' room naming both, the files and who lands first, for people to
//! see that agents sorted it out (the user's decision, 24 Nov 2026: agents say it, as zircd
//! can't read an order from their words). Said once: the side that says it second is told
//! the other already did.
//!
//! Two sessions that settled are quiet to each other wherever their work meets, until the
//! first of them lands or the settle is forgotten: an overlap between them in files it didn't
//! name is no news ([`Daemon::settled`]), as a split said in an intent isn't
//! ([`Daemon::agreed`]), and neither wakes for the other's commits and pushes
//! ([`Daemon::settled_with`]). They agreed who lands first, and the second hears that landing.
//! Until 25 Nov 2026 a settle quieted its own files only, and never the commits: of 60 wakes
//! for overlaps in a day, 42 went to pairs that had settled already, one pair settling six
//! times in 80 minutes as it met in file after file.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use zirc_core::ipc::Response;
use zirc_core::presence;
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

    /// The one between `a` and `b` in `scope`, whatever its files, if it's still remembered
    /// at `at` and its first hasn't landed ([`Self::landed`]).
    fn standing(
        &self,
        scope: &str,
        (a, b): ((&str, &str), (&str, &str)),
        at: u64,
    ) -> Option<&Settled> {
        self.0
            .iter()
            .filter(|(then, said_in, _)| at.saturating_sub(*then) < REMEMBERED && said_in == scope)
            .map(|(_, _, settled)| settled)
            .find(|settled| settled.between(a, b))
    }

    /// Forgets what `session` on `machine` settled to land first in `scope`: it has landed,
    /// and what the two change from here on is news again.
    fn landed(&mut self, scope: &str, machine: &str, session: &str) {
        self.0.retain(|(_, said_in, settled)| {
            !(said_in == scope && settled.first.is(machine, session))
        });
    }
}

impl Daemon {
    /// Says, as `session`, that it and the session `with` names settled their overlap in
    /// `paths` ([`zirc_core::ipc::Request::Settle`]): `first` lands first, `me`, `them` or
    /// either as it's named, and what they agreed beyond that order.
    pub(super) async fn settle(
        &mut self,
        repo: &Path,
        session: &str,
        with: &str,
        first: &str,
        paths: &[PathBuf],
        (same_lines, agreed): (bool, zirc_proto::woke::Agreed),
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
            agreed,
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

    /// Remembers the settled overlap `response` says, if it's one, heard or said here; and
    /// forgets those a session settled to land first once CI says it landed.
    pub(super) fn settled_heard(&mut self, response: &Response) {
        let Response::Delivery(delivery) = response else {
            return;
        };
        if let Some(settled) = Settled::of(&delivery.message.event) {
            let scope = delivery.scope();
            self.settled_again(&scope, &settled);
            self.settled_lately.keep(&scope, settled, now());
        } else if let Some((scope, landed)) = super::moved::Landed::of(response)
            && let (Some(machine), Some(session)) = (landed.machine, landed.session)
        {
            self.settled_lately.landed(&scope, &machine, &session);
        }
    }

    /// Whether `session` on this machine, `me`, and `other` on `machine` have a settle
    /// standing in `scope`: whatever its files, until its first lands.
    pub(super) fn settled_with(
        &self,
        scope: &str,
        (me, session): (&str, &str),
        (machine, other): (&str, &str),
    ) -> bool {
        self.settled_lately
            .standing(scope, ((me, session), (machine, other)), now())
            .is_some()
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
            agreed: zirc_proto::woke::Agreed::Order,
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

    #[test]
    fn stands_between_the_two_wherever_they_meet_until_the_first_lands() {
        let mut lately = SettledLately::default();
        let settled = Settled {
            first: side("cc-1"),
            second: side("cc-2"),
            paths: vec!["web/src/a.rs".into()],
            same_lines: true,
            agreed: zirc_proto::woke::Agreed::Order,
        };
        lately.keep("#web", settled, 100);
        let pair = (("ada-mbp", "cc-2"), ("ada-mbp", "cc-1"));
        // A file it never named is covered by none, yet the pair's settle stands.
        assert!(
            lately
                .covering("#web", pair, &["docs/a.md".to_owned()], 200)
                .is_none()
        );
        assert!(lately.standing("#web", pair, 200).is_some());
        // Not for another pair, another repository, or once it's forgotten.
        let other = (("ada-mbp", "cc-2"), ("ada-mbp", "cc-3"));
        assert!(lately.standing("#web", other, 200).is_none());
        assert!(lately.standing("#api", pair, 200).is_none());
        assert!(lately.standing("#web", pair, 100 + REMEMBERED).is_none());
        // The second landing changes nothing; the first landing ends it.
        lately.landed("#web", "ada-mbp", "cc-2");
        assert!(lately.standing("#web", pair, 200).is_some());
        lately.landed("#api", "ada-mbp", "cc-1");
        assert!(lately.standing("#web", pair, 200).is_some());
        lately.landed("#web", "ada-mbp", "cc-1");
        assert!(lately.standing("#web", pair, 200).is_none());
    }
}
