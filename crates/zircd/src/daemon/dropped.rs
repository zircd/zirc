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
                let body = dropped_line(&session, &branch, &tip, &default);
                self.tell_dropped(&channel, body);
                told = true;
            }
        }
        if told {
            self.save_awaits();
        }
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
fn dropped_line(session: &str, branch: &str, tip: &str, default: &str) -> String {
    let short = tip.get(..7).unwrap_or(tip);
    format!(
        "{session}: {branch}, which your work is stacked on, was deleted from origin without \
         landing: dropped, so don't land it yourself. `zirc stack --rebase` rebases yours onto \
         {default} past it (`git fetch origin {default} && git rebase --onto origin/{default} \
         {short}`); then keep in yours what it needs of {branch}'s work, and ask whose it was \
         if you're unsure"
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
        let line = dropped_line("sam-receipt", "money", "abcdef0123", "main");
        assert!(line.starts_with("sam-receipt: money, which your work is stacked on"));
        assert!(line.contains("don't land it yourself"));
        assert!(line.contains("git rebase --onto origin/main abcdef0"));
    }
}
