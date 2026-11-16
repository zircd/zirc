//! A session's branch that takes out what someone else landed, as a conflict's resolution
//! does when it keeps only its own side: the tests still pass, so CI never says. Each commit
//! and push of a session's is read for it ([`crate::git::undone`]), off the loop, and the
//! session is told once for each landing it takes out, before or as it lands.

use std::path::Path;

use zirc_proto::Event;

use super::{Daemon, Input, WATCHER_SESSION};
use crate::git::{self, Undone};

impl Daemon {
    /// Reads, off the loop, whether `event`, a commit or push of `session`'s in the
    /// repository at `root`, takes out another's landing.
    pub(super) fn check_undone(&mut self, root: &Path, session: &str, event: &Event) {
        let head = match event {
            Event::Commit { sha, .. } => sha.clone(),
            Event::Push { head, .. } => head.clone(),
            _ => return,
        };
        if session == WATCHER_SESSION {
            return;
        }
        let Some(watched) = self.repos.get(root) else {
            return;
        };
        let (root, default) = (root.to_owned(), watched.default_branch.clone());
        let (channel, session) = (watched.channel.clone(), session.to_owned());
        let inputs = self.inputs.clone();
        tokio::spawn(async move {
            let found = git::undone(&root, &default, &head).await;
            if !found.is_empty() {
                let _ = inputs.send(Input::UndoneChecked(channel, session, found));
            }
        });
    }

    /// Tells `session` of each landing in `found` its branch takes out, once each.
    pub(super) fn undone_checked(&mut self, channel: &str, session: &str, found: Vec<Undone>) {
        for undone in found {
            let key = (session.to_owned(), undone.sha.clone(), undone.path.clone());
            if !self.undone_told.insert(key) {
                continue;
            }
            tracing::info!("{session}'s branch takes out {}", undone.sha);
            self.tell_dropped(channel, undone_line(session, &undone));
        }
    }
}

/// What a session whose branch takes out `undone` hears.
fn undone_line(session: &str, undone: &Undone) -> String {
    let short = undone.sha.get(..7).unwrap_or(&undone.sha);
    format!(
        "{session}: your branch takes out what {} landed in {short} ({}): in {}, `{}` is gone, \
         and nothing in its place keeps what it said. Keep their change as well as yours, unless \
         undoing it is your task; if yours has landed already, land a fix that brings theirs back",
        undone.author, undone.subject, undone.path, undone.line
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_whose_landing_is_taken_out_and_what_to_do() {
        let line = undone_line(
            "alice",
            &Undone {
                sha: "abcdef0123".into(),
                author: "Carol".into(),
                subject: "The summary names its numbers".into(),
                path: "shop/report.py".into(),
                line: "return f\"orders: {n}\"".into(),
            },
        );
        assert!(line.starts_with("alice: your branch takes out what Carol landed in abcdef0"));
        assert!(line.contains("Keep their change as well as yours"));
    }
}
