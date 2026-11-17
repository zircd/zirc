//! A branch a session is stacked on that's deleted from origin without landing: dropped, as a
//! team drops work it decided against. Nothing else says so: CI never speaks of it, a push
//! moves no head, and the session's own clone keeps its copy of the branch until a fetch
//! prunes it. So the session waited on it for good, and one, unwoken, landed the dropped
//! branch itself to free its own (the agent evals' S5, 16 Nov 2026).
//!
//! Every [`EVERY`], zircd asks origin, from a repository of each such session's, whether the
//! branches stacked on that CI hasn't said landed are still there ([`Daemon::check_stacks`]),
//! off its loop. A branch gone wakes each session stacked on it, once, with what to do: rebase
//! past it onto the default branch, keep what it needs of it in its own, and never land it in
//! its place ([`Daemon::stacks_checked`]). One pushed again is there after all.

use std::path::PathBuf;
use std::time::Duration;

use zirc_core::ipc::{Delivery, Response};
use zirc_proto::{Event, Message};

use super::{Daemon, Input, WATCHER_SESSION};

/// How often origin is asked about the branches sessions are stacked on.
const EVERY: Duration = Duration::from_mins(1);

/// How many of the last things said are kept for what was said of a dropped branch, and how
/// much of one its notice quotes.
const SAID_KEPT: usize = 200;
const SAID_SHOWN: usize = 300;

/// What a session hears of a base that's dropped, by which the wake breaker knows the line
/// ([`about_a_drop`]).
const DROPPED: &str = "was deleted from origin without landing";

/// Whether `response` is zircd telling `session` the branch it's stacked on was dropped:
/// what its stack waits on, so never held by the wake breaker. Held, behind three overlaps
/// and three catch-ups in ten minutes, it left a session waiting the run out for a branch
/// that wouldn't land (the agent evals' PL2 probe, 17 Nov 2026).
pub(super) fn about_a_drop(response: &Response, session: &str) -> bool {
    let Response::Delivery(delivery) = response else {
        return false;
    };
    let Event::Text { body } = &delivery.message.event else {
        return false;
    };
    delivery.message.session == WATCHER_SESSION
        && body.contains(DROPPED)
        && zirc_proto::addressed(body).is_some_and(|(to, _)| to.eq_ignore_ascii_case(session))
}

/// How long one ask of origin may take.
const ASK_FOR: Duration = Duration::from_secs(20);

impl Daemon {
    /// Asks origin, off the loop, whether the branches sessions here are stacked on, not
    /// landed, are still there, once [`EVERY`].
    pub(super) fn check_stacks(&mut self) {
        let now = self.clock.now();
        if self
            .stacks_checked
            .is_some_and(|at| now.duration_since(at) < EVERY)
        {
            return;
        }
        self.stacks_checked = Some(now);
        let mut asks: Vec<(PathBuf, String)> = Vec::new();
        for (session, branch) in self.awaits.stacks_to_check() {
            let Some(root) = self
                .repos
                .iter()
                .find(|(_, watched)| watched.is_of(&session))
                .map(|(root, _)| root.clone())
            else {
                continue;
            };
            if !asks.iter().any(|(r, b)| *r == root && *b == branch) {
                asks.push((root, branch));
            }
        }
        if asks.is_empty() {
            return;
        }
        let inputs = self.inputs.clone();
        tokio::spawn(async move {
            let mut gone = Vec::new();
            for (root, branch) in asks {
                if absent(&root, &branch).await {
                    gone.push((root, branch));
                }
            }
            let _ = inputs.send(Input::StacksChecked(gone));
        });
    }

    /// Keeps `session`'s push of a branch built on another's, not landed, as its stack on
    /// it, as `zirc stack <branch>` would: a session says its stack late, when it's about
    /// to land, or never, and one whose base was deleted by then was refused (`couldn't
    /// fetch`), kept no stack, and waited for good on a landing that wouldn't come (the
    /// agent evals' PL2 probe, 17 Nov 2026). The base's head is read off the loop.
    pub(super) fn note_stack(&mut self, root: &std::path::Path, session: &str, event: &Event) {
        let Event::Push { on: Some(on), .. } = event else {
            return;
        };
        // A worktree zircd found itself has no session of its own yet, until one runs a
        // command there: it's the one session of its repository's, when there's only one.
        let session = if session == WATCHER_SESSION {
            let Some(only) = self.lone_session_of(root) else {
                return;
            };
            only
        } else {
            session.to_owned()
        };
        let (root, on) = (root.to_owned(), on.clone());
        let inputs = self.inputs.clone();
        tokio::spawn(async move {
            let head = format!("refs/remotes/origin/{on}");
            let asked = tokio::process::Command::new("git")
                .args(["rev-parse", "--verify", "--quiet", &head])
                .current_dir(&root)
                .kill_on_drop(true)
                .output();
            if let Ok(Ok(out)) = tokio::time::timeout(ASK_FOR, asked).await
                && out.status.success()
            {
                let tip = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                let _ = inputs.send(Input::StackFound(session, on, tip));
            }
        });
    }

    /// The one session working in the repository `root` is a checkout or worktree of, across
    /// its worktrees: none when there are several, or no one.
    fn lone_session_of(&self, root: &std::path::Path) -> Option<String> {
        let common = &self.repos.get(root)?.common;
        let mut sessions = self
            .repos
            .values()
            .filter(|watched| watched.common == *common)
            .flat_map(|watched| watched.sessions.names());
        let only = sessions.next()?;
        sessions.all(|other| other == only).then(|| only.to_owned())
    }

    /// Notes the stack a push showed, unless the session is stacked on that branch already,
    /// as it said or an earlier push showed: what's known of that one stays.
    pub(super) fn stack_found(&mut self, session: &str, on: &str, tip: &str) {
        if tip.is_empty() || self.awaits.stacks(session).iter().any(|s| s.branch == on) {
            return;
        }
        tracing::info!("{session} pushed a branch stacked on {on} at {tip}");
        self.awaits.stack(session, on, tip);
        self.save_awaits();
        self.read_base(session, on, tip);
    }

    /// Reads, off the loop, the paths `branch` changes at `tip`, which `session` is stacked
    /// on, from a repository of the session's: an overlap in only those is with what it
    /// built on. Told `alice-mac is also changing shop/checkout/pricing.py` of each file of
    /// the branch it was asked to build on, 28 times in one run, a session took its owner
    /// to be still at work on them, asked her 13 times and waited (the agent evals' PL2
    /// probe, 17 Nov 2026).
    pub(super) fn read_base(&mut self, session: &str, branch: &str, tip: &str) {
        let Some((root, watched)) = self.repos.iter().find(|(_, w)| w.is_of(session)) else {
            return;
        };
        let range = format!("origin/{}...{tip}", watched.default_branch);
        let root = root.clone();
        let (session, branch, tip) = (session.to_owned(), branch.to_owned(), tip.to_owned());
        let inputs = self.inputs.clone();
        tokio::spawn(async move {
            let asked = tokio::process::Command::new("git")
                .args(["diff", "--name-only", &range])
                .current_dir(&root)
                .kill_on_drop(true)
                .output();
            if let Ok(Ok(out)) = tokio::time::timeout(ASK_FOR, asked).await
                && out.status.success()
            {
                let paths = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect();
                let _ = inputs.send(Input::BaseRead(session, branch, tip, paths));
            }
        });
    }

    /// Origin no longer has `gone`'s branches: each session stacked on one, as dropped, is
    /// told what to do, once.
    pub(super) fn stacks_checked(&mut self, gone: Vec<(PathBuf, String)>) {
        let mut told = false;
        for (root, branch) in gone {
            let Some(watched) = self.repos.get(&root) else {
                continue;
            };
            let channel = watched.channel.clone();
            let default = watched.default_branch.clone();
            let sessions: Vec<String> = watched.sessions.names().map(str::to_owned).collect();
            for session in sessions {
                let Some(tip) = self.awaits.dropped(&session, &branch) else {
                    continue;
                };
                tracing::info!("{branch}, which {session} is stacked on, was dropped");
                let said = self.said_of(&branch, &session);
                let body = dropped_line(&session, &branch, &tip, &default, said.as_ref());
                self.tell_dropped(&channel, body);
                told = true;
            }
        }
        if told {
            self.save_awaits();
        }
    }

    /// Keeps what `response` says in words, if it does, for [`Daemon::said_of`]: not zircd's
    /// own notices, which are no one's word on a branch.
    pub(super) fn keep_said(&mut self, response: &Response) {
        let Response::Delivery(delivery) = response else {
            return;
        };
        let Event::Text { body } = &delivery.message.event else {
            return;
        };
        if delivery.message.session == WATCHER_SESSION {
            return;
        }
        let who = if delivery.message.session.is_empty() {
            delivery.from.clone()
        } else {
            format!("{}/{}", delivery.from, delivery.message.session)
        };
        self.said_lately.push_back((who, body.clone()));
        if self.said_lately.len() > SAID_KEPT {
            self.said_lately.pop_front();
        }
    }

    /// What was last said that names `branch`, by anyone but `session`, and who said it:
    /// why it was dropped, when its owner said so. A session told only that its base was
    /// gone asked its person whether that was meant, where one that read the owner's `the
    /// team decided against the move` built on without it (the agent evals' PL2 probe, 17
    /// Nov 2026); and the owner's word may come before zircd knows who's stacked on what.
    fn said_of(&self, branch: &str, session: &str) -> Option<(String, String)> {
        self.said_lately
            .iter()
            .rev()
            .find(|(who, body)| {
                !who.ends_with(&format!("/{session}")) && super::deliver::names_branch(body, branch)
            })
            .cloned()
    }

    /// Tells the session `body` names, in a line of zircd's own.
    pub(super) fn tell_dropped(&mut self, channel: &str, body: String) {
        let from = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        self.tell_watchers(&Response::Delivery(Delivery::new(
            channel,
            from,
            Message::new(WATCHER_SESSION, Event::Text { body }),
        )));
    }
}

/// What a session stacked on `branch`, built on its head `tip`, hears once it's dropped.
fn dropped_line(
    session: &str,
    branch: &str,
    tip: &str,
    default: &str,
    said: Option<&(String, String)>,
) -> String {
    let short = tip.get(..7).unwrap_or(tip);
    // What its owner said of it, in a line, cut where it runs long.
    let said = said.map_or_else(String::new, |(who, body)| {
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let cut: String = body.chars().take(SAID_SHOWN).collect();
        let more = if cut.len() < body.len() { "…" } else { "" };
        format!(". {who} said of it: \"{cut}{more}\"")
    });
    format!(
        "{session}: {branch}, which your work is stacked on, {DROPPED}: dropped, so it won't \
         land, and don't land it yourself. Don't wait for it or \
         for anyone's word: build yours on {default} without it. `zirc stack --rebase` rebases \
         yours onto {default} past it (`git fetch origin {default} && git rebase --onto \
         origin/{default} {short}`); where that conflicts, or yours used what {branch} added, \
         do it again in yours as {default} has it, then land{said}"
    )
}

/// Whether origin, as the repository at `root` has it, answers that it has no `branch`: never
/// when it can't be asked.
async fn absent(root: &std::path::Path, branch: &str) -> bool {
    let head = format!("refs/heads/{branch}");
    let ask = tokio::process::Command::new("git")
        .args(["ls-remote", "--heads", "origin", &head])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(ASK_FOR, ask).await {
        Ok(Ok(out)) => out.status.success() && out.stdout.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_to_do_about_a_dropped_base() {
        let line = dropped_line("sam-receipt", "money", "abcdef0123", "main", None);
        assert!(line.starts_with("sam-receipt: money, which your work is stacked on"));
        assert!(line.contains("don't land it yourself"));
        assert!(line.contains("git rebase --onto origin/main abcdef0"));
        assert!(line.ends_with("then land"), "{line}");
    }

    #[test]
    fn says_what_was_said_of_a_dropped_base() {
        let said = (
            "alice-mac/alice".to_owned(),
            "`money` is dropped:\nwe keep cents".to_owned(),
        );
        let line = dropped_line("sam-receipt", "money", "abcdef0123", "main", Some(&said));
        assert!(
            line.ends_with(". alice-mac/alice said of it: \"`money` is dropped: we keep cents\""),
            "{line}"
        );
    }
}
