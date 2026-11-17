//! What a session hears of the default branch at its next wake, and never wakes for: that
//! landings have gone onto it since the session's work was built, and that one of the
//! repository's states is broken, or fine again.
//!
//! A landing changes files a session depends on far more often than its own: the session's
//! tests then run against old code, and it finds out when it lands. Whether that matters is
//! the session's to judge, when it's doing something anyway, so each landing is counted for
//! every session working in the repository and said as one note with the next line that
//! wakes it ([`super::later`]), until its work is built on something newer. A state said
//! broken names whoever is to fix it, whom it wakes; everyone else working there hears it
//! the same way, and that it's fine again only if they heard it broken.
//!
//! All of that comes from CI saying its landings in the channel. Where no CI says anything,
//! as for someone alone who hasn't wired theirs in, or a merge made on the Git host, zircd
//! looks itself: every [`EVERY`] it asks origin for the default branch's head
//! ([`Daemon::check_default`]), and a head that moved with nobody having said so, seen
//! twice so CI's own word comes first, is fetched into a ref of zircd's own, never the
//! person's, and read for what it changed. A session changing those files is told now, with
//! the rebase; the rest get the note above.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use zirc_core::ipc::{Delivery, Response, Why};
use zirc_core::sessions::LANDING_KIND;
use zirc_proto::state::{Level, State};
use zirc_proto::{Event, Message};

use super::{Daemon, Input, WATCHER_SESSION, in_repo};
use crate::git;

/// How often origin is asked for each repository's default branch's head. Where CI says
/// its landings, a move is said once it's seen twice, a minute on at most; where none ever
/// has, at once, as nothing else will say it: said a minute or two late, a session had found
/// the move from its own refused landing first (the agent evals' PN1 probe, 17 Nov 2026).
const EVERY: Duration = Duration::from_secs(30);

/// How long one ask of origin, or one fetch from it, may take.
const ASK_FOR: Duration = Duration::from_secs(30);

/// Where zircd keeps the default branch as it fetched it: no ref of the person's.
const KEPT: &str = "refs/zirc/origin";

/// How many landings are counted for a session at most.
const COUNTED: usize = 500;

/// What's kept to say these notes.
#[derive(Debug, Default)]
pub(super) struct Moved {
    /// The commits landed since each session's work was built, oldest first, by session.
    landed: BTreeMap<String, Vec<String>>,
    /// The states a session has a note of as broken, or heard so: session, repository's
    /// channel and state's name.
    broken: BTreeSet<(String, String, String)>,
    /// Each repository's default branch's head on origin as last known, by channel: as CI
    /// or a push said it, or as origin answered.
    heads: BTreeMap<String, String>,
    /// A head origin answered that nobody has said yet, by channel: said by zircd itself
    /// if origin answers it again.
    unsaid: BTreeMap<String, String>,
    /// When origin was last asked.
    asked: Option<std::time::Instant>,
    /// The channels of the repositories whose CI has said a landing: there, CI's word on a
    /// move is waited for before zircd says it.
    ci_says: BTreeSet<String>,
}

/// What a default branch that moved brought, as zircd read it once fetched.
#[derive(Debug)]
pub struct Brought {
    root: PathBuf,
    channel: String,
    head: String,
    subjects: Vec<String>,
    paths: Vec<String>,
}

/// A landing CI says landed: its commit, the session that queued it and what it's about.
struct Landed {
    commit: String,
    session: Option<String>,
    subject: Option<String>,
}

impl Landed {
    /// The landing `response` says landed, if it's CI saying one did.
    fn of(response: &Response) -> Option<(&str, Self)> {
        let Response::Delivery(delivery) = response else {
            return None;
        };
        let Event::Custom { kind, data, text } = &delivery.message.event else {
            return None;
        };
        let ci = zirc_proto::ci::Data::of(data);
        if kind != LANDING_KIND
            || ci.state.as_deref() != Some("passed")
            || !text.contains(" landed ")
        {
            return None;
        }
        let landed = Self {
            commit: ci.commit?,
            session: ci.session,
            subject: ci.subject,
        };
        Some((&delivery.channel, landed))
    }
}

/// A commit as people say it: its first seven characters.
fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// What a session is told of `count` landings on `branch` since its work was built on
/// `base`, the newest of them `commit`, about `subject`.
fn moved_line(
    session: &str,
    branch: &str,
    count: usize,
    base: Option<&str>,
    commit: &str,
    subject: Option<&str>,
) -> String {
    let landings = if count == 1 { "landing" } else { "landings" };
    let since = base.map_or_else(String::new, |base| {
        format!(" since {}, which your work is built on", short(base))
    });
    let about = subject.map_or_else(String::new, |subject| format!(" \"{subject}\""));
    format!(
        "{session}: {branch} moved: {count} {landings}{since}; the newest is {}{about}",
        short(commit)
    )
}

/// What a session changing `theirs` hears once `branch` moved on origin, in what `brought`
/// says of it, with nobody having said so.
fn moved_under_line(session: &str, branch: &str, brought: &Brought, theirs: &[&str]) -> String {
    const SHOWN: usize = 5;
    let count = brought.subjects.len();
    let commits = match count {
        0 => String::new(),
        1 => " (1 commit".to_owned(),
        n => format!(" ({n} commits"),
    };
    let newest = brought.subjects.last().map_or_else(
        || {
            if count == 0 {
                String::new()
            } else {
                ")".to_owned()
            }
        },
        |subject| {
            format!(
                "{}the newest is \"{subject}\")",
                if count == 0 { " (" } else { "; " }
            )
        },
    );
    let more = theirs.len().saturating_sub(SHOWN);
    let files = theirs[..theirs.len().min(SHOWN)].join(", ");
    let and_more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    format!(
        "{session}: {branch} moved on origin to {}{commits}{newest}, changing files you're \
         changing: {files}{and_more}. Rebase onto it before you build on or test those (`git \
         fetch origin {branch} && git rebase origin/{branch}`), and keep both its changes and \
         yours where they meet",
        short(&brought.head)
    )
}

/// `branch`'s head on origin, as the repository at `root` has origin: `None` when it can't
/// be asked or has none.
async fn head_on_origin(root: &Path, branch: &str) -> Option<String> {
    let head = format!("refs/heads/{branch}");
    let ask = tokio::process::Command::new("git")
        .args(["ls-remote", "--heads", "origin", &head])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(ASK_FOR, ask).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let said = String::from_utf8_lossy(&out.stdout);
    let commit = said.split_whitespace().next()?;
    (commit.len() >= 7).then(|| commit.to_owned())
}

/// Fetches `branch` from origin into zircd's own ref of it ([`KEPT`]), leaving the person's
/// branches and their remote's refs as they are: whether it did.
async fn fetched(root: &Path, branch: &str) -> bool {
    let spec = format!("+refs/heads/{branch}:{KEPT}/{branch}");
    let fetch = tokio::process::Command::new("git")
        // With no refmap, Git would move the person's own `origin/<branch>` as well.
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--refmap=",
            "origin",
            &spec,
        ])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output();
    matches!(tokio::time::timeout(ASK_FOR, fetch).await, Ok(Ok(out)) if out.status.success())
}

/// Whether `response` is a note that the default branch moved.
fn is_moved(response: &Response) -> bool {
    matches!(response, Response::Delivery(delivery) if delivery.why == Some(Why::DefaultMoved))
}

/// Whether `response` is `state` said broken.
fn says_broken(response: &Response, state: &State) -> bool {
    matches!(response, Response::Delivery(delivery)
    if State::of(&delivery.message.event).is_some_and(|said| {
        said.repo == state.repo && said.name == state.name && said.level == Level::Broken
    }))
}

impl Daemon {
    /// The sessions working in the repository `channel` is of that have watched, each with
    /// its default branch and the commit its work is built on.
    fn working_in(&self, channel: &str) -> Vec<(String, String, Option<String>)> {
        let known: BTreeSet<&str> = self
            .watchers
            .iter()
            .filter_map(|watcher| watcher.session.as_deref())
            .chain(self.missed.keys().map(String::as_str))
            .collect();
        self.repos
            .values()
            .filter(|watched| in_repo(channel, &watched.channel))
            .flat_map(|watched| {
                watched.sessions.names().map(|session| {
                    (
                        session.to_owned(),
                        watched.default_branch.clone(),
                        watched.snapshot.base.clone(),
                    )
                })
            })
            .filter(|(session, _, _)| known.contains(session.as_str()))
            .collect()
    }

    /// Keeps what `response` says of the default branch for the sessions it doesn't wake
    /// (`woken` are those it does), for their next wake: see the module's doc.
    pub(super) fn note_quietly(&mut self, response: &Response, woken: &BTreeSet<String>) {
        self.heard_head(response);
        if let Some((channel, landed)) = Landed::of(response) {
            let channel = channel.to_owned();
            self.note_landed(&channel, &landed, woken);
        } else if let Response::Delivery(delivery) = response
            && let Some(state) = State::of(&delivery.message.event)
        {
            self.note_state(response, &state, woken);
        }
    }

    /// Keeps the default branch's head `response` says, CI's landing or a push of it, so
    /// zircd doesn't say the same move again from what origin answers.
    fn heard_head(&mut self, response: &Response) {
        let said = match response {
            Response::Delivery(delivery) => match &delivery.message.event {
                Event::Push { branch, head, .. } => self
                    .repos
                    .values()
                    .any(|w| in_repo(&delivery.channel, &w.channel) && w.default_branch == *branch)
                    .then(|| (delivery.channel.clone(), head.clone())),
                _ => Landed::of(response)
                    .map(|(channel, landed)| (channel.to_owned(), landed.commit)),
            },
            _ => None,
        };
        let Some((channel, head)) = said else {
            return;
        };
        let by_ci = Landed::of(response).is_some();
        if let Some(repo) = self.repos.values().find(|w| in_repo(&channel, &w.channel)) {
            let channel = repo.channel.clone();
            self.moved.unsaid.remove(&channel);
            if by_ci {
                self.moved.ci_says.insert(channel.clone());
            }
            self.moved.heads.insert(channel, head);
        }
    }

    /// Asks origin, off the loop, for the head of each watched repository's default branch,
    /// once [`EVERY`]: one ask a repository, whatever its worktrees, and only where a session
    /// works.
    pub(super) fn check_default(&mut self) {
        let now = self.clock.now();
        if self
            .moved
            .asked
            .is_some_and(|at| now.duration_since(at) < EVERY)
        {
            return;
        }
        self.moved.asked = Some(now);
        // Only where a session with a watch works: nobody else would hear it.
        let mut asks: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
        for (root, watched) in &self.repos {
            if !asks.contains_key(&watched.channel) && !self.working_in(&watched.channel).is_empty()
            {
                asks.insert(
                    watched.channel.clone(),
                    (root.clone(), watched.default_branch.clone()),
                );
            }
        }
        if asks.is_empty() {
            return;
        }
        let inputs = self.inputs.clone();
        tokio::spawn(async move {
            let mut heads = Vec::new();
            for (channel, (root, branch)) in asks {
                if let Some(head) = head_on_origin(&root, &branch).await {
                    heads.push((channel, root, branch, head));
                }
            }
            let _ = inputs.send(Input::DefaultHeads(heads));
        });
    }

    /// What origin answered: a head that's new and unsaid is kept to see again, and one seen
    /// again is fetched and read, off the loop.
    pub(super) fn default_heads(&mut self, heads: Vec<(String, PathBuf, String, String)>) {
        for (channel, root, branch, head) in heads {
            let Some(known) = self.moved.heads.get(&channel).cloned() else {
                // The first answer is where things stand, not a move.
                self.moved.heads.insert(channel, head);
                continue;
            };
            if known == head {
                self.moved.unsaid.remove(&channel);
                continue;
            }
            // Where CI says its landings, its word comes first: seen again, it's unsaid.
            if self.moved.ci_says.contains(&channel)
                && self.moved.unsaid.get(&channel) != Some(&head)
            {
                self.moved.unsaid.insert(channel, head);
                continue;
            }
            self.moved.unsaid.remove(&channel);
            self.moved.heads.insert(channel.clone(), head.clone());
            tracing::info!("{branch} moved to {head} on origin, and nobody said so");
            let inputs = self.inputs.clone();
            tokio::spawn(async move {
                if !fetched(&root, &branch).await {
                    return;
                }
                let subjects = git::pushed_subjects(&root, Some(&known), &head).await;
                let paths = git::pushed_paths(&root, Some(&known), &head).await;
                let _ = inputs.send(Input::DefaultMoved(Brought {
                    root,
                    channel,
                    head,
                    subjects,
                    paths,
                }));
            });
        }
    }

    /// Says a default branch's move nobody else said: now to each session changing files it
    /// changed, and as the quiet note to the rest working there.
    pub(super) fn default_moved(&mut self, brought: &Brought) {
        let Some(branch) = self
            .repos
            .get(&brought.root)
            .map(|watched| watched.default_branch.clone())
        else {
            return;
        };
        let mut woken = BTreeSet::new();
        for (session, _, base) in self.working_in(&brought.channel) {
            if base.as_deref() == Some(brought.head.as_str()) {
                continue;
            }
            let mine = self.paths_in(&brought.channel, Some(&session));
            let theirs: Vec<&str> = brought
                .paths
                .iter()
                .map(String::as_str)
                .filter(|path| mine.contains(*path))
                .collect();
            if theirs.is_empty() {
                continue;
            }
            let body = moved_under_line(&session, &branch, brought, &theirs);
            self.tell_dropped(&brought.channel, body);
            woken.insert(session);
        }
        let landed = Landed {
            commit: brought.head.clone(),
            session: None,
            subject: brought.subjects.last().cloned(),
        };
        self.note_landed(&brought.channel, &landed, &woken);
    }

    fn note_landed(&mut self, channel: &str, landed: &Landed, woken: &BTreeSet<String>) {
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        for (session, branch, base) in self.working_in(channel) {
            // Its own landing, or one its work has already.
            if landed.session.as_deref() == Some(session.as_str())
                || base.as_deref() == Some(landed.commit.as_str())
            {
                continue;
            }
            let counted = self.moved.landed.entry(session.clone()).or_default();
            if counted.contains(&landed.commit) {
                continue;
            }
            counted.push(landed.commit.clone());
            if counted.len() > COUNTED {
                counted.remove(0);
            }
            let count = counted.len();
            // The landing's own line, as one that changes the session's files, says it now.
            if woken.contains(&session) {
                continue;
            }
            let body = moved_line(
                &session,
                &branch,
                count,
                base.as_deref(),
                &landed.commit,
                landed.subject.as_deref(),
            );
            let note = Response::Delivery(Delivery::new(
                channel.to_owned(),
                me.clone(),
                Message::new(WATCHER_SESSION, Event::Text { body }),
            ));
            // One note, of all of them: the one before it is no longer kept.
            if let Some(kept) = self.later.get_mut(&session) {
                kept.retain(|response| !is_moved(response));
            }
            self.keep_for_later(&session, &note, Why::DefaultMoved);
        }
    }

    fn note_state(&mut self, response: &Response, state: &State, woken: &BTreeSet<String>) {
        for (session, _, _) in self.working_in(&state.repo) {
            let key = (session.clone(), state.repo.clone(), state.name.clone());
            match state.level {
                Level::Broken => {
                    if self.moved.broken.insert(key) && !woken.contains(&session) {
                        self.keep_for_later(&session, response, Why::StateChanged);
                    }
                }
                Level::Ok => {
                    if !self.moved.broken.remove(&key) {
                        continue;
                    }
                    // Broken and fine again before the session heard either: neither is said.
                    let kept = self.later.entry(session.clone()).or_default();
                    let before = kept.len();
                    kept.retain(|response| !says_broken(response, state));
                    let unheard = kept.len() < before;
                    if kept.is_empty() {
                        self.later.remove(&session);
                    }
                    if !unheard && !woken.contains(&session) {
                        self.keep_for_later(&session, response, Why::StateChanged);
                    }
                }
                Level::Warning => {}
            }
        }
    }

    /// Notes that the work of `sessions` is built on another commit now, as after a rebase:
    /// the landings counted for them are behind it, and a note of them not yet heard is
    /// dropped.
    pub(super) fn rebuilt(&mut self, sessions: &[String]) {
        for session in sessions {
            self.moved.landed.remove(session);
            if let Some(kept) = self.later.get_mut(session) {
                kept.retain(|response| !is_moved(response));
                if kept.is_empty() {
                    self.later.remove(session);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_many_landings_since_what_the_work_is_built_on() {
        let base = "b".repeat(40);
        let commit = "1a2b3c4d5e".repeat(4);
        assert_eq!(
            moved_line("cc-1", "main", 1, Some(&base), &commit, Some("Fix login")),
            "cc-1: main moved: 1 landing since bbbbbbb, which your work is built on; the newest is 1a2b3c4 \"Fix login\""
        );
        assert_eq!(
            moved_line("cc-1", "main", 3, None, &commit, None),
            "cc-1: main moved: 3 landings; the newest is 1a2b3c4"
        );
    }

    fn landing(state: &str, text: &str) -> Response {
        Response::Delivery(Delivery::new(
            "#api/landings",
            "zirc-ci",
            Message::new(
                "ci",
                Event::Custom {
                    kind: LANDING_KIND.into(),
                    data: serde_json::json!({
                        "state": state, "session": "cc-2", "commit": "c".repeat(40),
                        "subject": "Fix login",
                    }),
                    text: text.into(),
                },
            ),
        ))
    }

    #[test]
    fn only_a_landing_that_landed_counts() {
        let said = landing("passed", "cc-2: landed login as ccccccc (build 7)");
        let (channel, landed) = Landed::of(&said).unwrap();
        assert_eq!(channel, "#api/landings");
        assert_eq!(landed.commit, "c".repeat(40));
        assert_eq!(landed.session.as_deref(), Some("cc-2"));
        assert_eq!(landed.subject.as_deref(), Some("Fix login"));
        // Queued, failed, or the Apple apps' word on it: not a landing landed.
        for (state, text) in [
            ("pending", "cc-2: queued login (build 7)"),
            ("failed", "cc-2: build 7 failed"),
            (
                "passed",
                "cc-2 (apple/login): build 7's Apple apps compile on ccccccc",
            ),
        ] {
            assert!(Landed::of(&landing(state, text)).is_none(), "{text}");
        }
    }
}
