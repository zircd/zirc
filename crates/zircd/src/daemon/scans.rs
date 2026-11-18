//! Reading watched repositories as they change, and what their changes say.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use zirc_core::hunks;
use zirc_core::ipc::{Delivery, Response, Worktree};
use zirc_core::presence::Conflict;
use zirc_proto::telemetry as usage;
use zirc_proto::telemetry::Event as Usage;
use zirc_proto::{Event, Message};

use crate::git;
use crate::git::{Change, Side};

use super::{
    ANNOUNCED, Daemon, Input, MAX_WAIT, QUIET, REJOIN_EVERY, SCANS, WATCHER_SESSION, Watched,
    events, now, post_room, wakes, warn_once,
};

/// What a scan starts from: the repository as it was last read, and what saying its changes
/// needs of it.
struct Before {
    snapshot: git::Snapshot,
    /// Whether its last scan was held ([`Watched::held`]): this one is believed.
    held: bool,
    web: Option<String>,
    default_branch: String,
}

/// What a scan found, read in a task of its own, Git's part in saying it included: the
/// daemon only applies it, so no request waits behind `git log` or `git diff`.
pub struct Scan {
    /// What it was read against: when the repository has been read otherwise since, the
    /// scan says nothing and it's read again.
    before: git::Snapshot,
    found: Found,
}

enum Found {
    /// It looks halfway removed ([`git::half_removed`]), and its last scan wasn't held.
    HalfRemoved,
    /// The repository as it is now, and what changed, in order.
    Changed(Box<git::Snapshot>, Vec<Changed>),
}

/// One change a scan found, as the daemon says it.
enum Changed {
    /// The working set: the daemon knows the whole channel's, and says it.
    Working,
    /// A HEAD that moved or a push, as the events to send ([`events`]).
    Events(Vec<Event>),
}

/// Reads the repository at `root`, and what changed there since `before`.
async fn read(root: &Path, before: Before) -> Result<Scan> {
    let after = git::snapshot(root).await?;
    let found = if !before.held && git::half_removed(&before.snapshot, &after) {
        Found::HalfRemoved
    } else {
        let mut changed = Vec::new();
        for change in git::diff(&before.snapshot, &after) {
            changed.push(if let Change::Working(_) = change {
                Changed::Working
            } else {
                let web = before.web.as_deref();
                Changed::Events(events(root, change, web, &before.default_branch).await)
            });
        }
        Found::Changed(Box::new(after), changed)
    };
    Ok(Scan {
        before: before.snapshot,
        found,
    })
}

impl Daemon {
    /// Scans the repositories whose files have settled since they were touched, and sends
    /// what changed.
    ///
    /// Reading a repository takes Git a while, so each is read in a task of
    /// its own, a few at a time, and what they find comes back as [`Input::Scanned`]:
    /// meanwhile requests are answered from what was read before.
    pub fn scan(&mut self) {
        let now = self.clock.now();
        self.announce_due(now);
        self.keep_typing(now);
        // A channel the server didn't let us join, as when it was coming back from a restart,
        // is tried again, so a machine is never left in none.
        if self.client.is_some() && now >= self.rejoin_at {
            self.rejoin_at = now + REJOIN_EVERY;
            let missing: Vec<String> = self
                .desired_channels()
                .into_iter()
                .filter(|channel| !self.joined.contains(channel))
                .collect();
            if !missing.is_empty() {
                tracing::info!("joining {} again", missing.join(" "));
                self.reconcile();
            }
        }
        let mut due: Vec<(Instant, PathBuf)> = self
            .repos
            .iter()
            .filter(|(root, _)| !self.scanning.contains(*root))
            .filter_map(|(root, watched)| {
                let (first, last) = watched.touched?;
                (now - last >= QUIET || now - first >= MAX_WAIT).then(|| (first, root.clone()))
            })
            .collect();
        due.sort();
        let room = SCANS.saturating_sub(self.scanning.len());
        for (_, root) in due.into_iter().take(room) {
            // A worktree that's been removed, as tools do when a session ends, is gone for
            // good: stop watching it and leave its channels.
            if !root.exists() {
                self.gone(&root);
                continue;
            }
            // Cleared first, so a scan that fails waits for the next change rather than being
            // retried every tick.
            if let Some(watched) = self.repos.get_mut(&root) {
                watched.touched = None;
            }
            let Some(watched) = self.repos.get(&root) else {
                continue;
            };
            let before = Before {
                snapshot: watched.snapshot.clone(),
                held: watched.held,
                web: watched.project.web.clone(),
                default_branch: watched.default_branch.clone(),
            };
            self.scanning.insert(root.clone());
            let inputs = self.inputs.clone();
            tokio::spawn(async move {
                let read = read(&root, before).await;
                let _ = inputs.send(Input::Scanned(root, read));
            });
        }
    }

    /// Waits, up to `wait`, for the scans and checks in flight to end. Each runs `git`, and
    /// zircd restarts by starting again in the same process: a child still running then is
    /// one the new zircd never waits for, left a zombie once it ends (one or two at each
    /// restart, 5 Nov 2026). What else comes meanwhile waits for the new zircd, as before.
    pub(super) async fn finish_scans(
        &mut self,
        received: &mut mpsc::UnboundedReceiver<Input>,
        wait: Duration,
    ) {
        let until = tokio::time::Instant::now() + wait;
        let mut held = Vec::new();
        while !self.scanning.is_empty() || self.checking > 0 {
            match tokio::time::timeout_at(until, received.recv()).await {
                Ok(Some(Input::Scanned(root, _))) => {
                    self.scanning.remove(&root);
                }
                Ok(Some(Input::PushChecked(_) | Input::Predicted(..) | Input::Fitted(_))) => {
                    self.checking = self.checking.saturating_sub(1);
                }
                Ok(Some(input)) => held.push(input),
                Ok(None) | Err(_) => break,
            }
        }
        // Back in order for whoever reads next: requests are answered as before.
        for input in held {
            if let Input::Request(..) = input {
                self.handle(input).await;
            }
        }
    }

    /// Stops watching the worktree at `root`, which has been removed, as tools do when a
    /// session ends, and leaves its channels.
    pub(super) fn gone(&mut self, root: &Path) {
        self.warned.forget(root);
        self.predictions.forget(root);
        let Some(watched) = self.repos.remove(root) else {
            return;
        };
        tracing::info!("{} is gone, so it's no longer watched", root.display());
        self.tell_gone(root, &watched);
        self.save();
        self.reconcile();
    }

    /// Tells the sessions added with `watched`, the checkout at `root` that's gone, that it's
    /// no longer watched for them, and how to be watched where they work now. Said on this
    /// machine alone: it's news to nobody else.
    pub(super) fn tell_gone(&mut self, root: &Path, watched: &Watched) {
        // Its changes went with it: so do the intents zircd said for them.
        let sessions: Vec<String> = watched.sessions.names().map(str::to_owned).collect();
        self.withdraw_auto_intents(watched.agents_channel(), &sessions);
        let me = self.client.as_ref().map(|client| client.nick().to_owned());
        for session in watched.sessions.names() {
            let body = format!(
                "{session}: {} is gone, so it's no longer watched for you; run `zirc repo add \
                 --session {session}` where you work now",
                root.display()
            );
            self.tell_watchers(&Response::Delivery(Delivery::new(
                watched.channel.clone(),
                me.clone().unwrap_or_default(),
                Message::new(WATCHER_SESSION, Event::Text { body }),
            )));
        }
    }

    /// What reading the repository at `root` found: what changed, sent, or why it failed,
    /// warned once.
    pub(super) fn scan_done(&mut self, root: &Path, read: Result<Scan>) {
        match read {
            Ok(scan) => {
                self.warned.ok(warn_once::About::Scan, root);
                self.scanned(root, scan);
                self.followed_branches_now();
            }
            // Removed while it was read: gone for good, as at the next scan.
            Err(_) if !root.exists() => self.gone(root),
            Err(err) => {
                let cause = format!("{err:#}");
                let message = format!("scanning {} failed: {cause}", root.display());
                self.warned
                    .warn(warn_once::About::Scan, root, &cause, &message);
            }
        }
    }

    /// Sends what `scan` found changed in the repository at `root`.
    pub(super) fn scanned(&mut self, root: &Path, scan: Scan) {
        let now = self.clock.now();
        let Some(watched) = self.repos.get_mut(root) else {
            return;
        };
        let again = |watched: &mut Watched| watched.touched = Some((now, now));
        // Read otherwise since the scan began: what it found changed is against what's no
        // longer so.
        if watched.snapshot != scan.before {
            again(watched);
            return;
        }
        let (after, changes) = match scan.found {
            // A worktree being removed is read again before it's believed: by the next scan
            // it's gone, and never said to be about to change every file it had.
            Found::HalfRemoved => {
                tracing::info!("{} looks halfway removed: reading it again", root.display());
                watched.held = true;
                again(watched);
                return;
            }
            Found::Changed(after, changes) => (after, changes),
        };
        watched.held = false;
        tracing::debug!("scanned {}: {} changes", root.display(), changes.len());
        // Built on another commit now, as after a rebase: what landed before it is behind it.
        let rebuilt: Vec<String> = if watched.snapshot.base == after.base {
            Vec::new()
        } else {
            watched.sessions.names().map(str::to_owned).collect()
        };
        // The remote's default branch as the repository has it now: moved by a fetch,
        // unless it's this worktree's own push, said as one.
        let remote = format!("origin/{}", watched.default_branch);
        let (was, is) = (
            watched.snapshot.remotes.get(&remote).cloned(),
            after.remotes.get(&remote).cloned(),
        );
        let fetched = is.filter(|is| was.as_ref() != Some(is) && after.head.as_ref() != Some(is));
        // The remote's branches the repository had and has no more: deleted from here, or
        // pruned by a fetch.
        let gone: Vec<(String, String)> = watched
            .snapshot
            .remotes
            .iter()
            .filter(|(name, _)| !after.remotes.contains_key(*name))
            .filter_map(|(name, tip)| Some((name.strip_prefix("origin/")?.to_owned(), tip.clone())))
            .collect();
        watched.snapshot = *after;
        self.rebuilt(&rebuilt);
        if !gone.is_empty() {
            self.stacks_gone(root, gone);
        }
        if let Some(head) = fetched {
            self.default_seen(root, was, head);
        }
        let Some(watched) = self.repos.get_mut(root) else {
            return;
        };
        (watched.room, watched.areas) =
            post_room::rooms_of(root, &watched.project, &mut self.warned);
        let channel = watched.channel.clone();
        let agents = watched.agents_channel().to_owned();
        for change in changes {
            let events = match change {
                Changed::Working => {
                    self.auto_intend(root);
                    self.announce_soon(&channel);
                    self.check_overlaps(&channel);
                    continue;
                }
                Changed::Events(events) => events,
            };
            for event in events {
                let Some(watched) = self.repos.get_mut(root) else {
                    break;
                };
                if let Event::Commit { subject, .. } = &event {
                    if watched.announced.contains(subject) {
                        continue;
                    }
                    watched.announced.push_back(subject.clone());
                    if watched.announced.len() > ANNOUNCED {
                        watched.announced.pop_front();
                    }
                }
                let session = watched.signer().to_owned();
                // What a session did is agents' news: in the room, counted as the repository's.
                self.publish(&agents, &session, event.clone());
                if let Some(how) = wakes::acted_by(&event) {
                    self.session_acted(&session, how);
                }
                let me = self.client.as_ref().map(|client| client.nick().to_owned());
                self.check_push(&channel, &me.unwrap_or_default(), &event);
                self.check_undone(root, &session, &event);
                self.note_stack(root, &session, &event);
            }
        }
    }

    /// Checks whether `event`, a push by `nick`, changes the same lines this machine's
    /// worktrees in `channel` do: a conflict ahead for whichever merges second. Git says, in a
    /// task of its own ([`clashes`]), and what it finds comes back as
    /// [`Input::PushChecked`]. Only a push whose commits are in the repository already can be
    /// compared, as another worktree's on this machine are; zircd doesn't fetch.
    pub(super) fn check_push(&mut self, channel: &str, nick: &str, event: &Event) {
        let Event::Push {
            branch,
            head,
            paths,
            base,
            lines,
            ..
        } = event
        else {
            return;
        };
        let against: Vec<Against> = self
            .repos
            .iter()
            .filter(|(_, watched)| watched.channel == channel)
            .map(|(root, watched)| Against {
                worktree: Worktree {
                    path: root.clone(),
                    branch: watched.snapshot.branch.clone(),
                },
                clean: watched.snapshot.dirty.is_empty(),
                default_pushed: watched.default_branch == *branch,
            })
            .collect();
        if against.is_empty() {
            return;
        }
        let push = Push {
            branch: branch.clone(),
            head: head.clone(),
            paths: paths.clone(),
            onto: base.clone(),
            lines: lines.clone(),
        };
        let (channel, nick) = (channel.to_owned(), nick.to_owned());
        let inputs = self.inputs.clone();
        self.checking += 1;
        tokio::spawn(async move {
            let found = clashes(&push, against).await;
            let _ = inputs.send(Input::PushChecked(Clashes {
                channel,
                nick,
                branch: push.branch,
                found,
            }));
        });
    }

    /// Tells the watchers of this machine's worktrees what a push was found to clash with
    /// ([`Daemon::check_push`]), each path once.
    pub(super) fn push_checked(&mut self, clashes: Clashes) {
        self.checking = self.checking.saturating_sub(1);
        let Clashes {
            channel,
            nick,
            branch,
            found,
        } = clashes;
        for (worktree, same_lines) in found {
            let clashing: BTreeSet<String> = same_lines.keys().cloned().collect();
            let key = format!("{}\n{nick}\n{branch}", worktree.path.display());
            let new = self.presence.new_conflicts(&key, &clashing, now());
            let same_lines: SameLines = same_lines
                .into_iter()
                .filter(|(path, _)| new.contains(path))
                .collect();
            if same_lines.is_empty() {
                continue;
            }
            let conflict = Response::Conflict(Conflict {
                channel: channel.clone(),
                nick: nick.clone(),
                branch: branch.clone(),
                worktree,
                same_lines,
                why: None,
            });
            tracing::info!("conflict ahead in {channel}: {conflict:?}");
            self.tell_watchers(&conflict);
            self.noticed(&conflict, None, true, false);
            self.count(&Usage::ConflictAheadNotified(
                usage::ConflictAheadNotified {},
            ));
        }
    }
}

/// The lines two changes share, by path: where each run starts and how long it is.
type SameLines = BTreeMap<String, Vec<[u32; 2]>>;

/// A push, as checking it against this machine's worktrees needs it ([`Event::Push`]).
struct Push {
    branch: String,
    head: String,
    paths: Vec<String>,
    /// What it was pushed onto, when it says the lines it changes.
    onto: Option<String>,
    lines: SameLines,
}

/// A worktree to check a push against, as the daemon knew it when the push came.
struct Against {
    worktree: Worktree,
    /// Whether it has nothing uncommitted.
    clean: bool,
    /// Whether the push is of its repository's default branch.
    default_pushed: bool,
}

/// What a push was found to clash with ([`Daemon::check_push`]).
pub struct Clashes {
    channel: String,
    nick: String,
    branch: String,
    /// Each worktree whose own work changes the same lines, and those lines by path.
    found: Vec<(Worktree, SameLines)>,
}

/// The lines `push` changes as each of `against` does too, of its own work.
async fn clashes(push: &Push, against: Vec<Against>) -> Vec<(Worktree, SameLines)> {
    let mut found = Vec::new();
    for against in against {
        let root = &against.worktree.path;
        let same_lines = if git::has_commit(root, &push.head).await {
            let Some(same_lines) = local_same_lines(push, &against).await else {
                continue;
            };
            same_lines
        } else {
            // Without its commits, as on another machine that hasn't fetched: by the lines
            // the push says it changed, when this worktree's work is on what it was
            // pushed onto.
            let Some(onto) = &push.onto else {
                continue;
            };
            if push.lines.is_empty()
                || git::merge_base(root, "HEAD", onto).await.as_ref() != Some(onto)
            {
                continue;
            }
            let theirs: hunks::Hunks = push
                .lines
                .iter()
                .map(|(path, lines)| {
                    let hunks = lines
                        .iter()
                        .map(|&[start, len]| hunks::Hunk::at(start, len))
                        .collect();
                    (path.clone(), hunks)
                })
                .collect();
            let paths: Vec<String> = push.lines.keys().cloned().collect();
            let ours = git::hunks(root, onto, None, &paths).await;
            hunks::predict(&paths, &ours, &theirs).same_lines
        };
        // Only the worktree's own work: a branch cut before what the worktree has from the
        // default branch changes those lines too, but that's no conflict of this one's.
        let own = git::own_paths(root).await;
        let mut same_lines = same_lines;
        same_lines.retain(|path, _| own.as_ref().is_none_or(|own| own.contains(path)));
        if !same_lines.is_empty() {
            found.push((against.worktree, same_lines));
        }
    }
    found
}

/// The lines `push`, which the worktree `against` has the commits of, changes as that
/// worktree does too, as rebasing its own work onto the push's would say. `None` when
/// there's nothing to compare: the worktree has the push already, or its own work is in it
/// under other shas and nothing's uncommitted.
async fn local_same_lines(push: &Push, against: &Against) -> Option<SameLines> {
    let (root, head) = (against.worktree.path.as_path(), push.head.as_str());
    // A worktree that has the push already, as the one that pushed it does, has nothing
    // to merge. What it has is what its files came from, whatever its branch says.
    let mine = git::files(root).await.ok()?.base?;
    let base = git::merge_base(root, &mine, head).await?;
    if base == head {
        return None;
    }
    // Its own work already in the push, as when it landed under another sha, and nothing
    // uncommitted: nothing of its own left to clash.
    if against.clean && git::all_in(root, head).await {
        return None;
    }
    // A branch's first push lists no paths: what changed since the base is the same.
    let changed;
    let paths = if push.paths.is_empty() {
        changed = git::changed_paths(root, &base, head).await;
        &changed
    } else {
        &push.paths
    };
    let ours_base = git::fork(root, &mine).await.unwrap_or_else(|| base.clone());
    // A push of the default branch is all the pusher's; another branch's is its own from
    // where it left the default branch, and what it has of that branch beyond this
    // worktree isn't the pusher's: the default branch's own push said it already.
    let theirs_base = if against.default_pushed {
        git::merge_base(root, &ours_base, head).await?
    } else {
        git::fork(root, head).await.unwrap_or_else(|| base.clone())
    };
    let ours_tree = git::own_tree(root, &ours_base, paths).await?;
    let ours = Side {
        base: &ours_base,
        tree: &ours_tree,
    };
    let theirs = Side {
        base: &theirs_base,
        tree: head,
    };
    Some(git::rebase(root, ours, theirs, paths).await?.same_lines)
}
