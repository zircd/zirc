//! What a repository looks like right now, read through the `git` CLI with read-only
//! commands, and what changed between two looks.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Context as _, Result, bail};
use tokio::process::Command;
use zirc_core::hunks::Hunks;

/// A repository's state, as far as zirc cares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The checked-out branch, unless HEAD is detached.
    pub branch: Option<String>,
    /// The commit HEAD points at, unless there are no commits yet.
    pub head: Option<String>,
    /// Remote-tracking branches, as `origin/main`, and their commits.
    pub remotes: BTreeMap<String, String>,
    /// The local branches.
    pub branches: BTreeSet<String>,
    /// Paths with uncommitted changes, relative to the root, untracked ones included and
    /// ignored ones not, that are someone's work: a path that's the same as on the remote's
    /// default branch isn't, as in a worktree caught halfway through updating to it.
    pub dirty: BTreeSet<String>,
    /// Of the `dirty` paths, those missing from the working tree: deleted, or not yet
    /// written.
    pub missing: BTreeSet<String>,
    /// The commit the work here is built on: the newest commit of the remote's default
    /// branch that HEAD has ([`fork`]). `None` without commits or a remote default branch.
    pub base: Option<String>,
    /// The paths HEAD's own commits since `base` change, beyond the `dirty` ones: none once
    /// those commits are all on the default branch, as after a landing.
    pub committed: BTreeSet<String>,
}

/// How many paths at least a scan must newly find changed, nearly all of them missing from
/// disk, to look like a worktree read while it was being removed ([`half_removed`]).
const REMOVING: usize = 20;

/// Whether `after`, read since `before`, looks like a worktree caught while it was being
/// removed (`git worktree remove`, or `rm -rf` of it): many paths newly changed, nine in ten
/// of them missing from disk, as each tracked file deleted so far shows as a change. On
/// 6 Nov 2026 one such scan said 298 files were about to change, overlapping and waking
/// three sessions, half a second before the worktree was gone. Read it again before
/// believing it: by then the worktree is gone, or the deletion is real and still there.
#[must_use]
pub fn half_removed(before: &Snapshot, after: &Snapshot) -> bool {
    let new: Vec<&String> = after.dirty.difference(&before.dirty).collect();
    let missing = new
        .iter()
        .filter(|path| after.missing.contains(**path))
        .count();
    new.len() >= REMOVING && missing * 10 >= new.len() * 9
}

/// What happened between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The set of paths with uncommitted changes is different, or what the work is built on
    /// or has committed is ([`Snapshot::base`], [`Snapshot::committed`]): these are all the
    /// paths with uncommitted changes now.
    Working(BTreeSet<String>),
    /// HEAD moved, by commits, checkouts, pulls or resets: HEAD's reflog tells which.
    HeadMoved {
        /// The branch checked out now.
        branch: String,
        /// Where HEAD pointed before, unless there were no commits yet.
        from: Option<String>,
    },
    /// A remote-tracking branch caught up with the local branch: it was pushed.
    Pushed {
        /// The branch.
        branch: String,
        /// The commit it now points at.
        head: String,
        /// Where the remote-tracking branch pointed before, if it existed.
        old: Option<String>,
    },
}

/// What changed from `before` to `after`.
#[must_use]
pub fn diff(before: &Snapshot, after: &Snapshot) -> Vec<Change> {
    let mut changes = Vec::new();

    if (&after.dirty, &after.base, &after.committed)
        != (&before.dirty, &before.base, &before.committed)
    {
        changes.push(Change::Working(after.dirty.clone()));
    }

    if let Some(branch) = &after.branch
        && after.head.is_some()
        && before.head != after.head
    {
        changes.push(Change::HeadMoved {
            branch: branch.clone(),
            from: before.head.clone(),
        });
    }

    if let (Some(branch), Some(head)) = (&after.branch, &after.head) {
        let suffix = format!("/{branch}");
        for (remote, commit) in &after.remotes {
            if remote.ends_with(&suffix)
                && commit == head
                && before.remotes.get(remote) != Some(commit)
            {
                changes.push(Change::Pushed {
                    branch: branch.clone(),
                    head: head.clone(),
                    old: before.remotes.get(remote).cloned(),
                });
            }
        }
    }

    changes
}

/// The root of the working tree containing `path`.
///
/// # Errors
///
/// When `path` isn't in a Git working tree.
pub async fn root(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        bail!("{} doesn't exist", path.display());
    }
    match git(path, &["rev-parse", "--show-toplevel"]).await {
        Ok(root) => Ok(PathBuf::from(root)),
        // Git's own words for it say no more, so they're left out.
        Err(err) if format!("{err:#}").contains("not a git repository") => {
            bail!("{} isn't in a Git repository", path.display())
        }
        Err(err) => Err(err.context(format!("{} isn't in a Git repository", path.display()))),
    }
}

/// `path` relative to the repository at `root`, as Git names it: `path` is absolute or
/// relative to `base`, a directory inside the repository, and needn't exist yet.
///
/// # Errors
///
/// When `path` is outside the repository.
pub fn relative(root: &Path, base: &Path, path: &Path) -> Result<String> {
    use std::path::Component;
    let full = base.join(path);
    // Resolve what exists, such as /tmp being /private/tmp, and keep the rest as written.
    let mut existing = full.as_path();
    let mut rest = Vec::new();
    let resolved = loop {
        if let Ok(resolved) = existing.canonicalize() {
            break resolved;
        }
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            break PathBuf::new();
        };
        rest.push(name.to_owned());
        existing = parent;
    };
    let mut clean = PathBuf::new();
    for component in resolved
        .components()
        .chain(rest.iter().rev().map(|name| Component::Normal(name)))
    {
        match component {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let inside = clean
        .strip_prefix(root)
        .with_context(|| format!("{} isn't in {}", path.display(), root.display()))?;
    Ok(inside.to_string_lossy().into_owned())
}

/// The Git directories to watch for ref changes: the worktree's own, and the common one
/// that linked worktrees share with the main checkout.
///
/// # Errors
///
/// When `root` isn't a Git working tree.
pub async fn git_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let own = PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"]).await?);
    let common = common_dir(root).await?;
    Ok(if common == own {
        vec![own]
    } else {
        vec![own, common]
    })
}

/// The Git directory every worktree of the repository at `root` shares, its main
/// checkout's `.git`: two checkouts with the same one are one repository.
///
/// # Errors
///
/// When `root` isn't a Git working tree.
pub async fn common_dir(root: &Path) -> Result<PathBuf> {
    let common = root.join(git(root, &["rev-parse", "--git-common-dir"]).await?);
    Ok(common.canonicalize().unwrap_or(common))
}

/// The main checkout of the repository whose common Git directory is `common`, the
/// directory its `.git` is in: `None` for a bare repository.
#[must_use]
pub fn main_checkout(common: &Path) -> Option<PathBuf> {
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_owned))?
}

/// The linked worktrees of the repository whose common Git directory is `common`, read
/// from the `gitdir` file Git keeps for each, without running Git: those still there, and
/// checked out. `git worktree add` locks one until its files are out, and writes its index
/// last, so a locked one without an index is still being made: read then, it would look as
/// if it deleted every file it hadn't checked out yet.
#[must_use]
pub fn linked_worktrees(common: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(common.join("worktrees")) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(|entry| {
            let dir = entry.ok()?.path();
            if dir.join("locked").exists() && !dir.join("index").exists() {
                return None;
            }
            let gitdir = std::fs::read_to_string(dir.join("gitdir")).ok()?;
            // The worktree's `.git` file.
            let dot_git = PathBuf::from(gitdir.trim());
            dot_git
                .is_file()
                .then(|| dot_git.parent()?.canonicalize().ok())?
        })
        .collect();
    found.sort();
    found
}

/// The URL of `origin`, or of the first remote when there's no `origin`.
pub async fn remote_url(root: &Path) -> Option<String> {
    if let Ok(url) = git(root, &["remote", "get-url", "origin"]).await {
        return Some(url);
    }
    let remotes = git(root, &["remote"]).await.ok()?;
    let first = remotes.lines().next()?;
    git(root, &["remote", "get-url", first]).await.ok()
}

/// The remote's default branch, as `origin/HEAD` names it, or `main` or `master` when
/// those exist locally, or `main`.
pub async fn default_branch(root: &Path) -> String {
    if let Ok(head) = git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .await
        && let Some((_, branch)) = head.split_once('/')
    {
        return branch.to_owned();
    }
    for branch in ["main", "master"] {
        let local = format!("refs/heads/{branch}");
        if git(root, &["rev-parse", "--quiet", "--verify", &local])
            .await
            .is_ok()
        {
            return branch.to_owned();
        }
    }
    "main".to_owned()
}

/// How many times [`checked_out`] reads HEAD again when its branch changed meanwhile.
const CHECKOUT_READS: usize = 5;

/// The branch checked out in `root` and the commit HEAD is at, as they were at one moment.
/// They're read by separate commands, so a branch switched and committed on between them,
/// as `git switch -c fix && git commit` does, once paired the old branch with the new
/// commit: its commit was said to be on `main`, and a session it touched never waited on it
/// to land (`zircd_landings_test`, builds 2820 and 2883). The branch is read again after HEAD,
/// and HEAD again until the branch held still around it.
async fn checked_out(root: &Path) -> (Option<String>, Option<String>) {
    let branch_now = async || {
        git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await
            .ok()
    };
    let mut branch = branch_now().await;
    let mut head = None;
    for _ in 0..CHECKOUT_READS {
        head = git(root, &["rev-parse", "--quiet", "--verify", "HEAD"])
            .await
            .ok();
        let after = branch_now().await;
        if after == branch {
            break;
        }
        branch = after;
    }
    (branch, head)
}

/// What `root` looks like now.
///
/// # Errors
///
/// When a git command fails.
pub async fn snapshot(root: &Path) -> Result<Snapshot> {
    // The refs before HEAD: a push is a remote-tracking branch caught up with HEAD
    // ([`diff`]), and HEAD read before them, as a `git commit && git push` went by between,
    // paired the old HEAD with the new remote-tracking branch. Nothing was said, the next
    // scan found that branch as it was and said the commit alone, and the push was never
    // told (`zircd_overlaps_test`, builds 2245, 2340 and 3327). Read first, the
    // remote-tracking branch is at worst behind HEAD, and its push is told when it catches up.
    let refs = git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/remotes",
            "refs/heads",
        ],
    )
    .await?;
    let refs: Vec<(&str, &str)> = refs
        .lines()
        .filter_map(|line| line.split_once(' '))
        .collect();
    let remotes = refs
        .iter()
        .filter_map(|(name, commit)| Some((name.strip_prefix("refs/remotes/")?, commit)))
        .filter(|(name, _)| !name.ends_with("/HEAD"))
        .map(|(name, commit)| (name.to_owned(), (*commit).to_owned()))
        .collect();
    let branches = refs
        .iter()
        .filter_map(|(name, _)| name.strip_prefix("refs/heads/"))
        .map(str::to_owned)
        .collect();
    let (branch, head) = checked_out(root).await;
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await?;
    let files = files_from(root, branch.as_deref(), head.as_deref(), &status).await;
    let dirty = work(root, &remotes, files.changed, &untracked_paths(&status)).await;
    let missing = dirty
        .iter()
        .filter(|path| !root.join(path).exists())
        .cloned()
        .collect();
    let (base, committed) = built_on(root, head.as_deref(), &dirty).await;
    Ok(Snapshot {
        branch,
        head,
        remotes,
        branches,
        dirty,
        missing,
        base,
        committed,
    })
}

/// What the work at `root`, whose HEAD is `head`, is built on, and the paths its own
/// commits since change beyond the `dirty` ones ([`Snapshot::base`],
/// [`Snapshot::committed`]).
async fn built_on(
    root: &Path,
    head: Option<&str>,
    dirty: &BTreeSet<String>,
) -> (Option<String>, BTreeSet<String>) {
    let Some(head) = head else {
        return (None, BTreeSet::new());
    };
    let default = format!("origin/{}", default_branch(root).await);
    let Some(base) = merge_base(root, head, &default).await else {
        return (None, BTreeSet::new());
    };
    if base == head {
        return (Some(base), BTreeSet::new());
    }
    // Commits that landed under other ids, as a landing's rebase gives them, aren't this
    // worktree's own any more.
    if all_in(root, &default).await {
        return (Some(base), BTreeSet::new());
    }
    let committed = changed_between(root, &base, head)
        .await
        .map(|paths| paths.difference(dirty).cloned().collect())
        .unwrap_or_default();
    (Some(base), committed)
}

/// Of the `dirty` paths, those that differ from the remote's default branch too: the rest
/// are what that branch has already, as in a worktree caught halfway through updating to it,
/// and nobody's work. Without a remote default branch, all of them.
async fn work(
    root: &Path,
    remotes: &BTreeMap<String, String>,
    dirty: BTreeSet<String>,
    untracked: &BTreeSet<String>,
) -> BTreeSet<String> {
    let default = format!("origin/{}", default_branch(root).await);
    if dirty.is_empty() || !remotes.contains_key(&default) {
        return dirty;
    }
    // Tracked files, as they are in the working tree, against the default branch.
    let Ok(differ) = git(root, &["diff", "--name-only", "-z", &default, "--"]).await else {
        return dirty;
    };
    let differ: BTreeSet<&str> = differ.split('\0').filter(|path| !path.is_empty()).collect();
    let mut work = BTreeSet::new();
    for path in dirty {
        let same = if untracked.contains(&path) {
            // Untracked here, so the diff didn't see it: the same if the default branch has
            // a file there with the same content.
            let here = git(root, &["hash-object", "--", &path]).await.ok();
            let there = git(
                root,
                &[
                    "rev-parse",
                    "--quiet",
                    "--verify",
                    &format!("{default}:{path}"),
                ],
            )
            .await
            .ok();
            here.is_some() && here == there
        } else {
            !differ.contains(path.as_str())
        };
        if !same {
            work.insert(path);
        }
    }
    work
}

/// The untracked paths in `git status --porcelain=v1 -z` output.
fn untracked_paths(status: &str) -> BTreeSet<String> {
    status
        .split('\0')
        .filter_map(|entry| entry.strip_prefix("?? "))
        .filter(|path| !path.ends_with('/'))
        .map(ToOwned::to_owned)
        .collect()
}

/// How recent a commit must be to count as just made. A checkout of an old branch also
/// brings unpushed commits into HEAD's reach; they're older than this.
const RECENT_SECS: u64 = 10 * 60;
/// The most commits announced for one move of HEAD.
const MAX_NEW_COMMITS: &str = "50";

/// The commits just made since HEAD was at `from`, oldest first, as `(commit, subject)`:
/// reachable from HEAD but not from `from` or any remote-tracking branch, and made in the
/// last few minutes. This doesn't depend on HEAD's reflog saying `commit: …`, which tools
/// such as some Git front ends don't write; when the reflog does say HEAD moved by a checkout, pull,
/// reset, rebase or merge, nothing counts.
pub async fn new_commits(root: &Path, from: Option<&str>) -> Vec<(String, String)> {
    // The reflog's newest entry speaks for this move only if it moved HEAD to where it is:
    // a tool that writes no entry leaves an older one on top.
    let head = git(root, &["rev-parse", "HEAD"]).await.unwrap_or_default();
    let newest = git(root, &["reflog", "-1", "--format=%H%x09%gs"])
        .await
        .unwrap_or_default();
    if let Some((moved_to, how)) = newest.split_once('\t')
        && moved_to == head
        && [
            "checkout",
            "pull",
            "reset",
            "rebase",
            "merge",
            "cherry-pick",
        ]
        .iter()
        .any(|kind| how.starts_with(kind))
    {
        return Vec::new();
    }
    let not_from = from.map(|from| format!("^{from}"));
    let mut args = vec![
        "log",
        "--reverse",
        "--format=%H%x09%ct%x09%s",
        "--max-count",
        MAX_NEW_COMMITS,
        "HEAD",
    ];
    args.extend(not_from.as_deref());
    args.extend(["--not", "--remotes"]);
    let Ok(log) = git(root, &args).await else {
        return Vec::new();
    };
    recent_commits(&log, crate::clock::unix())
}

/// The commits in log lines of `%H\t%ct\t%s` made within [`RECENT_SECS`] of `now`.
fn recent_commits(log: &str, now: u64) -> Vec<(String, String)> {
    log.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let (commit, time, subject) = (fields.next()?, fields.next()?, fields.next()?);
            let time: u64 = time.parse().ok()?;
            (now.saturating_sub(time) <= RECENT_SECS)
                .then(|| (commit.to_owned(), subject.to_owned()))
        })
        .collect()
}

/// The subjects of the commits a push from `old` to `head` added, oldest first; for a new
/// branch, just `head`'s.
pub async fn pushed_subjects(root: &Path, old: Option<&str>, head: &str) -> Vec<String> {
    let range = old.map_or_else(
        || format!("{head}~1..{head}"),
        |old| format!("{old}..{head}"),
    );
    let log = git(
        root,
        &["log", "--reverse", "--format=%s", "--max-count=20", &range],
    )
    .await
    .or(Err(()));
    let log = match log {
        Ok(log) => log,
        // A root commit has no parent to start a range from.
        Err(()) => git(root, &["log", "-1", "--format=%s", head])
            .await
            .unwrap_or_default(),
    };
    log.lines().map(ToOwned::to_owned).collect()
}

/// The most paths a push lists: more than this, and it lists none.
const MAX_PUSHED_PATHS: usize = 100;

/// The paths a push from `old` to `head` changed, or none for a new branch, whose old
/// commit isn't known, or a very large push.
pub async fn pushed_paths(root: &Path, old: Option<&str>, head: &str) -> Vec<String> {
    let Some(old) = old else {
        return Vec::new();
    };
    let paths: Vec<String> = git(root, &["diff", "--name-only", "-z", old, head])
        .await
        .unwrap_or_default()
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if paths.len() > MAX_PUSHED_PATHS {
        return Vec::new();
    }
    paths
}

/// Whether every commit HEAD has that `upstream` doesn't is in `upstream` all the same, as
/// the same change under another sha (`git cherry`), as after a rebase or a landing: false
/// when HEAD has none of its own, or any that isn't.
pub async fn all_in(root: &Path, upstream: &str) -> bool {
    let cherry = git(root, &["cherry", upstream, "HEAD"])
        .await
        .unwrap_or_default();
    let mut lines = cherry.lines().peekable();
    lines.peek().is_some() && lines.all(|line| line.starts_with('-'))
}

/// Whether `ancestor` is in `commit`'s history: a push from one to the other didn't rewrite
/// anything.
pub async fn is_ancestor(root: &Path, ancestor: &str, commit: &str) -> bool {
    git(root, &["merge-base", "--is-ancestor", ancestor, commit])
        .await
        .is_ok()
}

/// The newest commit both `a` and `b` have, if they have one: what they each changed from.
pub async fn merge_base(root: &Path, a: &str, b: &str) -> Option<String> {
    git(root, &["merge-base", a, b]).await.ok()
}

/// The paths the work at `root` changes, committed on its branch or not, that the remote's
/// default branch doesn't have: what's this worktree's own, rather than what it has from that
/// branch and a branch cut earlier doesn't. Counted from the commit its files came from
/// ([`Files::base`]). `None` without a remote default branch.
pub async fn own_paths(root: &Path) -> Option<BTreeSet<String>> {
    let files = files(root).await.ok()?;
    let fork = fork(root, files.base.as_deref()?).await?;
    let mut paths = files.changed;
    let base = files.base?;
    if fork != base {
        paths.extend(changed_between(root, &fork, &base).await?.iter().cloned());
    }
    Some(paths)
}

/// Where `commit` left the remote's default branch: the newest commit of it that `commit`
/// has. `None` without a remote default branch.
pub async fn fork(root: &Path, commit: &str) -> Option<String> {
    let default = format!("origin/{}", default_branch(root).await);
    merge_base(root, commit, &default).await
}

/// What the files of a worktree are made from, and how they differ from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// The commit they were checked out from, unless there are no commits yet: HEAD, unless
    /// the branch moved on without them, as when a tool rebases a worktree's branch and
    /// leaves its files as they were. Then it's the earlier commit of the branch they're
    /// nearest, and the newer commits aren't counted as undone by them.
    pub base: Option<String>,
    /// The paths whose files differ from `base`'s, relative to the root, untracked ones
    /// included and ignored ones not. A change only in the index, the files as `base` has
    /// them, isn't one.
    pub changed: BTreeSet<String>,
}

/// The [`Files`] of the worktree at `root`.
///
/// # Errors
///
/// When git can't say its status.
pub async fn files(root: &Path) -> Result<Files> {
    let branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .ok();
    let head = head(root).await;
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await?;
    Ok(files_from(root, branch.as_deref(), head.as_deref(), &status).await)
}

/// How many of a branch's earlier commits, newest first, [`files_from`] considers its
/// files' base.
const BASE_CANDIDATES: &str = "50";

/// The [`Files`] of the worktree at `root`, on `branch` at `head`, from its `status`
/// (`git status --porcelain=v1 -z`).
async fn files_from(root: &Path, branch: Option<&str>, head: Option<&str>, status: &str) -> Files {
    let Some(head) = head else {
        return Files {
            base: None,
            changed: dirty_paths(status),
        };
    };
    let here = differ_from_head(root, head, status).await;
    let Some(branch) = branch else {
        return Files {
            base: Some(head.to_owned()),
            changed: here,
        };
    };
    let mut best = (head.to_owned(), here);
    if best.1.is_empty() {
        return Files {
            base: Some(best.0),
            changed: best.1,
        };
    }
    let reflog = format!("refs/heads/{branch}");
    let positions = git(
        root,
        &["log", "-g", "--format=%H", "-n", BASE_CANDIDATES, &reflog],
    )
    .await
    .unwrap_or_default();
    let mut seen = BTreeSet::from([head.to_owned()]);
    for position in positions.lines() {
        if !seen.insert(position.to_owned()) {
            continue;
        }
        let Some(moved) = changed_between(root, position, head).await else {
            continue;
        };
        // The files differ from `position` where they differ from HEAD and it doesn't, where
        // it differs from HEAD and they don't, and where both differ from HEAD, if not in
        // the same way.
        let here = &best.1;
        let only: BTreeSet<String> = here.symmetric_difference(&moved).cloned().collect();
        if only.len() >= best.1.len() {
            continue;
        }
        let both: BTreeSet<String> = here.intersection(&moved).cloned().collect();
        let mut changed = only;
        changed.extend(differ_from(root, position, &both).await);
        if changed.len() < best.1.len() {
            best = (position.to_owned(), changed);
        }
    }
    Files {
        base: Some(best.0),
        changed: best.1,
    }
}

/// The paths in `status` whose files differ from `head`'s. Most entries say so for sure; one
/// whose index differs from both, or that's untracked where the index deletes it, as when
/// the index is left behind as the files move on, is compared.
async fn differ_from_head(root: &Path, head: &str, status: &str) -> BTreeSet<String> {
    let mut codes: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let (code, path) = entry.split_at(entry.len().min(3));
        if !path.ends_with('/') {
            codes.entry(path).or_default().push(code);
        }
        if code.starts_with(['R', 'C']) {
            entries.next();
        }
    }
    let mut sure = BTreeSet::new();
    let mut unsure = BTreeSet::new();
    for (path, codes) in codes {
        let one_side = |code: &&str| {
            let mut sides = code.chars();
            *code == "?? " || sides.next() == Some(' ') || sides.next() == Some(' ')
        };
        if codes.len() == 1 && codes.iter().all(one_side) {
            sure.insert(path.to_owned());
        } else {
            unsure.insert(path.to_owned());
        }
    }
    sure.extend(differ_from(root, head, &unsure).await);
    sure
}

/// The paths changed between two commits, by the commits: [`changed_between`]'s answers.
type Kept = HashMap<(String, String), Arc<BTreeSet<String>>>;

/// The paths changed between commits or trees `from` and `to`. That never changes, so the
/// answers are kept.
async fn changed_between(root: &Path, from: &str, to: &str) -> Option<Arc<BTreeSet<String>>> {
    static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(Mutex::default);
    let key = (from.to_owned(), to.to_owned());
    if let Some(paths) = KEPT.lock().ok()?.get(&key) {
        return Some(paths.clone());
    }
    let names = git(
        root,
        &["diff", "--name-only", "-z", "--no-renames", from, to, "--"],
    )
    .await
    .ok()?;
    let paths: Arc<BTreeSet<String>> = Arc::new(
        names
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    );
    let mut kept = KEPT.lock().ok()?;
    if kept.len() >= 1024 {
        kept.clear();
    }
    kept.insert(key, paths.clone());
    Some(paths)
}

/// Of `paths`, those whose files at `root` differ from `commit`'s: in content, or by one
/// being there and not the other.
async fn differ_from(root: &Path, commit: &str, paths: &BTreeSet<String>) -> BTreeSet<String> {
    if paths.is_empty() {
        return BTreeSet::new();
    }
    let mut args = vec!["ls-tree", "-z", "--full-tree", commit, "--"];
    args.extend(paths.iter().map(String::as_str));
    let listed = git(root, &args).await.unwrap_or_default();
    // `<mode> <type> <object>\t<path>`
    let there: BTreeMap<&str, &str> = listed
        .split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            Some((path, meta.split(' ').nth(2)?))
        })
        .collect();
    let present: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|path| {
            root.join(path)
                .symlink_metadata()
                .is_ok_and(|it| !it.is_dir())
        })
        .collect();
    let mut here: BTreeMap<&str, String> = BTreeMap::new();
    if !present.is_empty() {
        let mut args = vec!["hash-object", "--"];
        args.extend(&present);
        let blobs = git(root, &args).await.unwrap_or_default();
        here.extend(
            present
                .iter()
                .copied()
                .zip(blobs.lines().map(|blob| blob.trim().to_owned())),
        );
    }
    paths
        .iter()
        .filter(|path| {
            there.get(path.as_str()).copied() != here.get(path.as_str()).map(String::as_str)
        })
        .cloned()
        .collect()
}

/// A tree of `base`'s with `paths` as the worktree at `root` has them: added, changed or
/// removed. Written with an index of its own, so the worktree's is left alone.
pub async fn own_tree(root: &Path, base: &str, paths: &[String]) -> Option<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let index = std::env::temp_dir().join(format!(
        "zircd-index-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let index_path = index.to_string_lossy().into_owned();
    let env = [("GIT_INDEX_FILE", index_path.as_str())];
    let (present, missing): (Vec<&str>, Vec<&str>) =
        paths.iter().map(String::as_str).partition(|path| {
            root.join(path)
                .symlink_metadata()
                .is_ok_and(|it| !it.is_dir())
        });
    let tree = async {
        run(root, &env, &["read-tree", base]).await.ok()?;
        for (how, paths) in [("--add", present), ("--force-remove", missing)] {
            if !paths.is_empty() {
                let mut args = vec!["update-index", how, "--"];
                args.extend(paths);
                run(root, &env, &args).await.ok()?;
            }
        }
        run(root, &env, &["write-tree"]).await.ok()
    }
    .await;
    let _ = std::fs::remove_file(&index);
    tree
}

/// One side of [`rebase`]: its changes are those from `base`, a commit of the default
/// branch, to `tree`.
#[derive(Debug, Clone, Copy)]
pub struct Side<'a> {
    /// The commit of the default branch the side's made from.
    pub base: &'a str,
    /// The tree, or commit, it makes of it.
    pub tree: &'a str,
}

/// What [`rebase`] predicts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rebased {
    /// The paths both sides change of their own.
    pub both: BTreeSet<String>,
    /// Those that conflict once one's rebased onto the other, with the lines of `ours`'s
    /// file they conflict in.
    pub same_lines: BTreeMap<String, Vec<[u32; 2]>>,
    /// Those that merge.
    pub apart: Vec<String>,
}

/// Which of `paths` two sides both change, and which of those conflict, as rebasing one onto
/// the other would say: the side made from the older commit of the default branch is first
/// rebased onto the other's, so neither's changes include what the default branch did in
/// between. A path that conflicts with the default branch itself there is left out: it's not
/// the other side's. `None` when their bases aren't one before the other, or git can't.
pub async fn rebase(
    root: &Path,
    ours: Side<'_>,
    theirs: Side<'_>,
    paths: &[String],
) -> Option<Rebased> {
    let (mut ours_tree, mut theirs_tree) = (ours.tree.to_owned(), theirs.tree.to_owned());
    let mut base = ours.base;
    let mut skip = BTreeSet::new();
    if ours.base != theirs.base {
        if is_ancestor(root, ours.base, theirs.base).await {
            let (tree, conflicts) = merge(root, ours.base, theirs.base, ours.tree).await?;
            (ours_tree, skip, base) = (tree, conflicts, theirs.base);
        } else if is_ancestor(root, theirs.base, ours.base).await {
            let (tree, conflicts) = merge(root, theirs.base, ours.base, theirs.tree).await?;
            (theirs_tree, skip) = (tree, conflicts);
        } else {
            return None;
        }
    }
    let changed = |tree: String| async move {
        let mut args = vec![
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            base,
            tree.as_str(),
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        git(root, &args).await.ok().map(|names| {
            names
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(ToOwned::to_owned)
                .collect::<BTreeSet<String>>()
        })
    };
    let ours_changed = changed(ours_tree.clone()).await?;
    let theirs_changed = changed(theirs_tree.clone()).await?;
    let both: BTreeSet<String> = ours_changed
        .intersection(&theirs_changed)
        .filter(|path| !skip.contains(*path))
        .cloned()
        .collect();
    let mut rebased = Rebased {
        both,
        ..Rebased::default()
    };
    if rebased.both.is_empty() {
        return Some(rebased);
    }
    let (merged, conflicts) = merge(root, base, &ours_tree, &theirs_tree).await?;
    for path in &rebased.both {
        if conflicts.contains(path) {
            let text = git(root, &["cat-file", "-p", &format!("{merged}:{path}")])
                .await
                .unwrap_or_default();
            rebased
                .same_lines
                .insert(path.clone(), conflict_lines(&text));
        } else {
            rebased.apart.push(path.clone());
        }
    }
    Some(rebased)
}

/// `ours` and `theirs`, commits or trees both made from `base`, merged as `git merge-tree`
/// does, writing nothing but objects: the merged tree, and the paths that conflict.
async fn merge(
    root: &Path,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Option<(String, BTreeSet<String>)> {
    let base = format!("--merge-base={base}");
    let output = run_allowing(
        root,
        &[],
        &[
            "-c",
            "merge.conflictStyle=merge",
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
            &base,
            ours,
            theirs,
        ],
        &[0, 1],
    )
    .await
    .ok()?;
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    let tree = fields.next()?.to_owned();
    Some((tree, fields.map(ToOwned::to_owned).collect()))
}

/// The lines of the `ours` side of each conflict in `text`, a file with conflict markers,
/// first and last, counted as in `ours`'s file. A conflict where `ours` has no lines is
/// between the lines either side of it. A conflict without markers, as in a binary file or
/// a file one side deleted, is said to be at its top.
fn conflict_lines(text: &str) -> Vec<[u32; 2]> {
    let mut lines = Vec::new();
    let mut line: u32 = 0;
    let mut first = 0;
    let mut in_ours = false;
    let mut in_theirs = false;
    for text in text.lines() {
        if !in_ours && !in_theirs && text.starts_with("<<<<<<< ") {
            in_ours = true;
            first = line + 1;
        } else if in_ours && (text == "=======" || text.starts_with("||||||| ")) {
            in_ours = false;
            in_theirs = true;
            lines.push(if line >= first {
                [first, line]
            } else {
                [line.max(1), line + 1]
            });
        } else if in_theirs && text.starts_with(">>>>>>> ") {
            in_theirs = false;
        } else if !in_theirs {
            line += 1;
        }
    }
    if lines.is_empty() {
        lines.push([1, 1]);
    }
    lines
}

/// The commit checked out at `root` now, unless there are no commits yet.
pub async fn head(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
        .ok()
}

/// The paths changed between `from` and `to`, at most as many as a push lists.
pub async fn changed_paths(root: &Path, from: &str, to: &str) -> Vec<String> {
    let range = format!("{from}..{to}");
    git(root, &["diff", "--name-only", "--no-renames", &range])
        .await
        .map(|names| {
            names
                .lines()
                .take(MAX_PUSHED_PATHS)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether this repository has `commit`, as it does when another worktree of it made it.
pub async fn has_commit(root: &Path, commit: &str) -> bool {
    let object = format!("{commit}^{{commit}}");
    git(root, &["cat-file", "-e", &object]).await.is_ok()
}

/// The hunks of `paths` changed from `base`: to `to`, or to the worktree at `root` with
/// its uncommitted changes when `to` is `None`. New files Git doesn't track aren't in it.
pub async fn hunks(root: &Path, base: &str, to: Option<&str>, paths: &[String]) -> Hunks {
    let mut args = vec![
        "diff",
        "--unified=0",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "--no-prefix",
        base,
    ];
    args.extend(to);
    args.push("--");
    args.extend(paths.iter().map(String::as_str));
    match git(root, &args).await {
        Ok(diff) => zirc_core::hunks::parse(&diff),
        Err(err) => {
            tracing::debug!("no hunks in {}: {err:#}", root.display());
            Hunks::new()
        }
    }
}

/// The paths in `git status --porcelain=v1 -z` output. A rename's entry is followed by the
/// path it came from, which is skipped. A nested repository, such as a worktree some tool
/// keeps inside the checkout (`.claude/worktrees/…`), shows as a directory ending in `/`:
/// it's its own working tree, not this one's changes, so it's left out.
fn dirty_paths(status: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let (code, path) = entry.split_at(entry.len().min(3));
        if !path.ends_with('/') {
            paths.insert(path.to_owned());
        }
        if code.starts_with(['R', 'C']) {
            entries.next();
        }
    }
    paths
}

/// What a push that rewrote the branch from `old` to `head` brought that's new: the
/// subjects of its commits that aren't the same change as one `old` had, as a rebase or an
/// amend that only rebased makes them, nor on the `default` branch, here or on a remote, as
/// a rebase onto it brings in, oldest first; and the paths those commits changed. Nothing
/// for a rewrite that only rebased, or when `old` isn't here to compare with.
pub async fn rewritten(
    root: &Path,
    old: &str,
    head: &str,
    default: &str,
) -> (Vec<String>, Vec<String>) {
    if !has_commit(root, old).await {
        return (Vec::new(), Vec::new());
    }
    let range = format!("{old}...{head}");
    // The default branch as this repository has it, and as each remote does.
    let on_default = git(
        root,
        &[
            "for-each-ref",
            "--format=^%(refname)",
            &format!("refs/heads/{default}"),
            &format!("refs/remotes/*/{default}"),
        ],
    )
    .await
    .unwrap_or_default();
    let mut args = vec![
        "log",
        "--cherry-pick",
        "--right-only",
        "--no-merges",
        "--reverse",
        "--max-count=20",
        "--format=%H%x00%s",
        &range,
    ];
    args.extend(on_default.lines());
    let log = git(root, &args).await.unwrap_or_default();
    let mut subjects = Vec::new();
    let mut paths = BTreeSet::new();
    for line in log.lines() {
        let Some((sha, subject)) = line.split_once('\0') else {
            continue;
        };
        subjects.push(subject.to_owned());
        let changed = git(
            root,
            &[
                "diff-tree",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                sha,
            ],
        )
        .await
        .unwrap_or_default();
        paths.extend(
            changed
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    let paths = if paths.len() > MAX_PUSHED_PATHS {
        Vec::new()
    } else {
        paths.into_iter().collect()
    };
    (subjects, paths)
}

/// How long one `git` command may take before it's given up on and killed. zircd's are
/// read-only and take well under a second; minutes means one is stuck, as on a lock or a
/// file system that stopped answering, and a scan waiting on it would never end.
const WITHIN: std::time::Duration = std::time::Duration::from_secs(120);

/// What `command` printed, if it ends `within` that long: past it, it's killed.
async fn output_within(
    mut command: Command,
    within: std::time::Duration,
) -> Result<std::process::Output> {
    // Dropped when the time runs out, which kills it.
    let running = command.kill_on_drop(true).output();
    let Ok(output) = tokio::time::timeout(within, running).await else {
        bail!("no answer within {}s: killed", within.as_secs());
    };
    Ok(output?)
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    run(dir, &[], args).await
}

/// `git` with `env` set too.
async fn run(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Result<String> {
    run_allowing(dir, env, args, &[0]).await
}

/// [`run`], taking any of the exit `codes` as success, as `git merge-tree` exits 1 for a
/// conflict.
async fn run_allowing(
    dir: &Path,
    env: &[(&str, &str)],
    args: &[&str],
    codes: &[i32],
) -> Result<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .envs(env.iter().copied())
        .current_dir(dir);
    let output = output_within(command, WITHIN)
        .await
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output
        .status
        .code()
        .is_some_and(|code| codes.contains(&code))
    {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut text = String::from_utf8(output.stdout).context("git's output isn't UTF-8")?;
    // Keep NULs (status -z) but not the trailing newline of one-line answers.
    let trimmed = text.trim_end_matches('\n').len();
    text.truncate(trimmed);
    Ok(text)
}

#[cfg(test)]
mod relative_tests {
    use super::*;

    #[test]
    fn names_paths_relative_to_the_root_whether_they_exist_or_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("crates/zircd")).unwrap();
        let base = dir.path().join("crates");
        let named = |path: &str| relative(&root, &base, Path::new(path)).unwrap();
        assert_eq!(named("zircd"), "crates/zircd");
        assert_eq!(named("zircd/src/new.rs"), "crates/zircd/src/new.rs");
        assert_eq!(named("../README.md"), "README.md");
        assert_eq!(named("."), "crates");
        assert_eq!(named(root.join("web").to_str().unwrap()), "web");
        assert!(relative(&root, &base, Path::new("/etc/hosts")).is_err());
    }
}

#[cfg(test)]
mod own_tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn own_paths_leave_out_what_the_default_branch_has() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        run(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("a.rs"), "a\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "a"]);
        // No remote default branch: nothing to tell its work from.
        assert_eq!(own_paths(&root).await, None);
        // The default branch moved on with b.rs; the worktree's branch has it and its own.
        run(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&root, &["checkout", "--quiet", "-b", "mine"]);
        std::fs::write(root.join("b.rs"), "b\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "b"]);
        run(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        std::fs::write(root.join("c.rs"), "c\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "c"]);
        std::fs::write(root.join("a.rs"), "a, changed\n").unwrap();
        let own = own_paths(&root).await.unwrap();
        assert_eq!(own, ["a.rs".to_owned(), "c.rs".to_owned()].into());
    }

    #[tokio::test]
    async fn a_snapshot_says_what_the_work_is_built_on_and_has_committed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        run(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("a.rs"), "a\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "a"]);
        // No remote default branch: nothing it's built on.
        let alone = snapshot(&root).await.unwrap();
        assert_eq!((alone.base, alone.committed), (None, BTreeSet::new()));
        // On the default branch's head: built on it, with nothing of its own.
        run(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let fork = snapshot(&root).await.unwrap().head;
        run(&root, &["checkout", "--quiet", "-b", "mine"]);
        let fresh = snapshot(&root).await.unwrap();
        assert_eq!((&fresh.base, fresh.committed.len()), (&fork, 0));
        // Its own commit, and an uncommitted change: the commit's paths beyond that one.
        std::fs::write(root.join("b.rs"), "b\n").unwrap();
        std::fs::write(root.join("c.rs"), "c\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "b and c"]);
        std::fs::write(root.join("c.rs"), "c, changed\n").unwrap();
        let before = snapshot(&root).await.unwrap();
        assert_eq!(before.base, fork);
        assert_eq!(before.dirty, ["c.rs".to_owned()].into());
        assert_eq!(before.committed, ["b.rs".to_owned()].into());
        // The default branch moving on leaves what it's built on where it was.
        run(&root, &["checkout", "--quiet", "--", "c.rs"]);
        run(&root, &["checkout", "--quiet", "--detach", "origin/main"]);
        std::fs::write(root.join("d.rs"), "d\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "d"]);
        run(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&root, &["checkout", "--quiet", "mine"]);
        let behind = snapshot(&root).await.unwrap();
        assert_eq!(behind.base, fork);
        assert_eq!(
            behind.committed,
            ["b.rs".to_owned(), "c.rs".to_owned()].into()
        );
        // Rebased onto it: built on the new head, which is a change to say.
        run(&root, &["rebase", "--quiet", "origin/main"]);
        let rebased = snapshot(&root).await.unwrap();
        assert_ne!(rebased.base, fork);
        assert_eq!(rebased.committed, behind.committed);
        assert!(matches!(
            diff(&behind, &rebased).as_slice(),
            [Change::Working(_), Change::HeadMoved { .. }]
        ));
        // Landed, under another id as a landing's rebase gives it: nothing of its own left.
        run(&root, &["checkout", "--quiet", "--detach", "origin/main"]);
        run(&root, &["cherry-pick", "mine"]);
        run(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&root, &["checkout", "--quiet", "mine"]);
        assert!(snapshot(&root).await.unwrap().committed.is_empty());
    }

    #[tokio::test]
    async fn own_paths_leave_out_what_a_checkout_left_behind_as_its_branch_moved_on() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        run(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("a.rs"), "a\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "a"]);
        // The default branch changes a.rs and adds d.rs; the branch moves onto it, but its
        // files stay as they were, so the landing shows as undone. e.rs is the work's own.
        std::fs::write(root.join("a.rs"), "a, landed\n").unwrap();
        std::fs::write(root.join("d.rs"), "d\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "landed"]);
        run(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&root, &["reset", "--quiet", "--hard", "HEAD~1"]);
        run(&root, &["reset", "--quiet", "--soft", "origin/main"]);
        std::fs::write(root.join("e.rs"), "e\n").unwrap();
        run(&root, &["add", "e.rs"]);
        assert_eq!(own_paths(&root).await.unwrap(), ["e.rs".to_owned()].into());
        // Changing a.rs itself is its own work again.
        std::fs::write(root.join("a.rs"), "a, mine\n").unwrap();
        assert_eq!(
            own_paths(&root).await.unwrap(),
            ["a.rs".to_owned(), "e.rs".to_owned()].into()
        );
    }

    /// A repository on `main` with a.rs, then a landing changing it and adding d.rs, which
    /// the remote's default branch has: the commits before and after it.
    fn landed() -> (tempfile::TempDir, PathBuf, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        run(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("a.rs"), "a\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "a"]);
        std::fs::write(root.join("a.rs"), "a, landed\n").unwrap();
        std::fs::write(root.join("d.rs"), "d\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "--quiet", "-m", "landed"]);
        run(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let commit = |rev: &str| {
            let out = std::process::Command::new("git")
                .args(["rev-parse", rev])
                .current_dir(&root)
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        let (before, after) = (commit("HEAD~1"), commit("HEAD"));
        (dir, root, before, after)
    }

    #[tokio::test]
    async fn files_left_behind_by_their_branch_are_made_from_where_it_was() {
        let (_dir, root, before, _) = landed();
        run(&root, &["reset", "--quiet", "--hard", "HEAD~1"]);
        run(&root, &["reset", "--quiet", "--soft", "origin/main"]);
        std::fs::write(root.join("e.rs"), "e\n").unwrap();
        let files = files(&root).await.unwrap();
        assert_eq!(files.base, Some(before));
        assert_eq!(files.changed, ["e.rs".to_owned()].into());
    }

    #[tokio::test]
    async fn files_moved_on_with_their_index_left_behind_are_made_from_head() {
        let (_dir, root, _, after) = landed();
        // The index as it was before the landing, the files as they are after it.
        run(&root, &["read-tree", "HEAD~1"]);
        let files = files(&root).await.unwrap();
        assert_eq!(files.base, Some(after));
        assert_eq!(files.changed, BTreeSet::new());
    }

    #[tokio::test]
    async fn undoing_a_landing_after_updating_onto_it_is_work() {
        let (_dir, root, _, after) = landed();
        std::fs::write(root.join("a.rs"), "a\n").unwrap();
        let files = files(&root).await.unwrap();
        assert_eq!(files.base, Some(after));
        assert_eq!(files.changed, ["a.rs".to_owned()].into());
    }

    #[tokio::test]
    async fn an_own_tree_has_the_files_at_its_paths_alone() {
        let (_dir, root, before, _) = landed();
        std::fs::write(root.join("a.rs"), "a, mine\n").unwrap();
        std::fs::remove_file(root.join("d.rs")).unwrap();
        std::fs::write(root.join("e.rs"), "e\n").unwrap();
        std::fs::write(root.join("f.rs"), "not asked for\n").unwrap();
        let paths = ["a.rs", "d.rs", "e.rs", "gone.rs"].map(str::to_owned);
        let tree = own_tree(&root, &before, &paths).await.unwrap();
        let listed = git(&root, &["ls-tree", "--name-only", &tree])
            .await
            .unwrap();
        assert_eq!(listed, "a.rs\ne.rs");
        let a = git(&root, &["cat-file", "-p", &format!("{tree}:a.rs")]).await;
        assert_eq!(a.unwrap(), "a, mine");
    }

    #[tokio::test]
    async fn a_rebase_counts_neither_sides_share_of_the_default_branch() {
        let (_dir, root, before, after) = landed();
        let tree = |base: &str, a: &str| {
            let root = root.clone();
            let (base, a) = (base.to_owned(), a.to_owned());
            async move {
                std::fs::write(root.join("a.rs"), a).unwrap();
                own_tree(&root, &base, &["a.rs".to_owned()]).await.unwrap()
            }
        };
        let paths = ["a.rs".to_owned()];
        // Ours, from before the landing, changes a.rs on the line the landing did; theirs,
        // from after it, changes the landing's line again. Rebased onto the landing, ours
        // conflicts with the landing, not with theirs.
        let ours = tree(&before, "a, ours\n").await;
        let theirs = tree(&after, "a, theirs\n").await;
        let side = |base, tree| Side { base, tree };
        let rebased = rebase(&root, side(&before, &ours), side(&after, &theirs), &paths)
            .await
            .unwrap();
        assert_eq!(rebased, Rebased::default());
        // From the same base, they meet.
        let ours = tree(&after, "a, ours\n").await;
        let rebased = rebase(&root, side(&after, &ours), side(&after, &theirs), &paths)
            .await
            .unwrap();
        assert_eq!(rebased.both, ["a.rs".to_owned()].into());
        assert_eq!(rebased.same_lines["a.rs"], [[1, 1]]);
        // Ours leaving a.rs as the landing made it changes nothing of its own there.
        let ours = tree(&after, "a, landed\n").await;
        let rebased = rebase(&root, side(&after, &ours), side(&after, &theirs), &paths)
            .await
            .unwrap();
        assert_eq!(rebased.both, BTreeSet::new());
    }

    #[test]
    fn conflict_lines_count_ours_lines() {
        let text = "1\n<<<<<<< ours\n2\n3\n=======\nb\n>>>>>>> theirs\n4\n<<<<<<< ours\n=======\nc\n>>>>>>> theirs\n5\n";
        assert_eq!(conflict_lines(text), [[2, 3], [4, 5]]);
        assert_eq!(conflict_lines("binary"), [[1, 1]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(branch: &str, head: &str, dirty: &[&str]) -> Snapshot {
        Snapshot {
            branch: Some(branch.into()),
            head: Some(head.into()),
            remotes: BTreeMap::new(),
            branches: BTreeSet::new(),
            dirty: dirty.iter().map(|&path| path.to_owned()).collect(),
            ..Snapshot::default()
        }
    }

    #[test]
    fn a_worktree_read_while_it_was_removed_is_read_again() {
        let before = snapshot("mine", "a", &["src/lib.rs"]);
        let files: Vec<String> = (0..300).map(|n| format!("src/f{n}.rs")).collect();
        let named: Vec<&str> = files.iter().map(String::as_str).collect();
        // Halfway through a removal: 300 tracked files gone from disk, shown as changed.
        let removing = Snapshot {
            missing: files.iter().cloned().collect(),
            ..snapshot("mine", "a", &named)
        };
        assert!(half_removed(&before, &removing));
        // Read again with nothing new, as a real deletion still there would be: believed.
        assert!(!half_removed(&removing, &removing));
        // Many files changed and still on disk: real work, believed at once.
        assert!(!half_removed(&before, &snapshot("mine", "a", &named)));
        // A few deleted: a session's own, believed at once.
        let few = Snapshot {
            missing: ["src/f1.rs".to_owned(), "src/f2.rs".to_owned()].into(),
            ..snapshot("mine", "a", &["src/f1.rs", "src/f2.rs"])
        };
        assert!(!half_removed(&before, &few));
    }

    /// Runs git in `dir`, as a test does, and what it prints.
    fn run(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// Commits `path` with `text` as `subject`, and its id.
    fn commit(dir: &Path, path: &str, text: &str, subject: &str) -> String {
        std::fs::write(dir.join(path), text).unwrap();
        run(dir, &["add", path]);
        run(dir, &["commit", "-q", "-m", subject]);
        run(dir, &["rev-parse", "HEAD"])
    }

    #[tokio::test]
    async fn knows_when_its_own_work_is_upstream_under_other_shas() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run(root, &["init", "-q", "-b", "main"]);
        commit(root, "README.md", "zirc\n", "start");
        run(root, &["checkout", "-q", "-b", "login"]);
        let mine = commit(root, "login.rs", "fn login() {}\n", "log in");
        // Nothing of its own: nothing to be upstream.
        assert!(!all_in(root, "HEAD").await);
        // Main has another change; the login commit lands on it under another sha.
        run(root, &["checkout", "-q", "main"]);
        commit(root, "ci.yml", "steps: []\n", "ci");
        assert!(!all_in_from(root, "main", "login").await);
        run(root, &["cherry-pick", &mine]);
        assert!(all_in_from(root, "main", "login").await);
        // A further commit of its own isn't upstream.
        run(root, &["checkout", "-q", "login"]);
        commit(root, "token.rs", "fn token() {}\n", "a token");
        assert!(!all_in(root, "main").await);
    }

    /// [`all_in`] for `branch` checked out.
    async fn all_in_from(root: &Path, upstream: &str, branch: &str) -> bool {
        run(root, &["checkout", "-q", branch]);
        let answer = all_in(root, upstream).await;
        run(root, &["checkout", "-q", "main"]);
        answer
    }

    #[tokio::test]
    async fn a_rewrite_says_only_what_it_brought_that_is_new() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run(root, &["init", "-q", "-b", "main"]);
        commit(root, "README.md", "zirc\n", "start");
        run(root, &["checkout", "-q", "-b", "login"]);
        commit(root, "login.rs", "fn login() {}\n", "log in");
        let old = commit(root, "page.rs", "fn page() {}\n", "the page");
        // Master moves on; the branch is rebased onto it, and gains a commit.
        run(root, &["checkout", "-q", "main"]);
        commit(root, "ci.yml", "steps: []\n", "ci");
        run(root, &["checkout", "-q", "login"]);
        run(root, &["rebase", "-q", "main"]);
        let rebased = run(root, &["rev-parse", "HEAD"]);
        assert_eq!(
            rewritten(root, &old, &rebased, "main").await,
            (vec![], vec![])
        );
        let head = commit(root, "token.rs", "fn token() {}\n", "a token");
        assert_eq!(
            rewritten(root, &old, &head, "main").await,
            (vec!["a token".to_owned()], vec!["token.rs".to_owned()])
        );
        // An unknown old commit: nothing to compare with.
        assert_eq!(
            rewritten(
                root,
                "0123456789abcdef0123456789abcdef01234567",
                &head,
                "main"
            )
            .await,
            (vec![], vec![])
        );
    }

    #[test]
    fn reports_the_whole_working_set_when_it_changes() {
        let before = snapshot("main", "a", &["src/old.rs"]);
        let after = snapshot("main", "a", &["src/old.rs", "src/new.rs"]);
        assert_eq!(
            diff(&before, &after),
            [Change::Working(after.dirty.clone())]
        );
        // Shrinking counts too, down to nothing after a commit.
        let clean = snapshot("main", "a", &[]);
        assert_eq!(diff(&before, &clean), [Change::Working(BTreeSet::new())]);
        assert!(diff(&before, &before).is_empty());
    }

    #[test]
    fn reports_head_moves_across_branch_switches_too() {
        let before = snapshot("main", "a", &[]);
        for after in [snapshot("main", "b", &[]), snapshot("feature", "b", &[])] {
            assert_eq!(
                diff(&before, &after),
                [Change::HeadMoved {
                    branch: after.branch.clone().unwrap(),
                    from: Some("a".into())
                }]
            );
        }
        // A new branch where HEAD already was moves nothing.
        assert!(diff(&before, &snapshot("feature", "a", &[])).is_empty());
    }

    #[test]
    fn counts_only_commits_just_made() {
        let now = 1_000_000;
        let log = format!(
            "c1\t{}\tOld work from a checkout\nc2\t{}\tAdd login\nc3\t{}\tFix typo\n",
            now - RECENT_SECS - 1,
            now - 60,
            now
        );
        assert_eq!(
            recent_commits(&log, now),
            [
                ("c2".to_owned(), "Add login".to_owned()),
                ("c3".to_owned(), "Fix typo".to_owned())
            ]
        );
        assert!(recent_commits("garbage\n", now).is_empty());
    }

    #[test]
    fn reports_pushes_when_the_remote_branch_catches_up() {
        let mut before = snapshot("main", "b", &[]);
        before.remotes.insert("origin/main".into(), "a".into());
        let mut after = before.clone();
        after.remotes.insert("origin/main".into(), "b".into());
        assert_eq!(
            diff(&before, &after),
            [Change::Pushed {
                branch: "main".into(),
                head: "b".into(),
                old: Some("a".into())
            }]
        );
        // A fetch of someone else's work isn't our push.
        let mut fetched = before.clone();
        fetched.remotes.insert("origin/main".into(), "c".into());
        assert!(diff(&before, &fetched).is_empty());
    }

    #[test]
    fn reads_porcelain_status() {
        let status = " M src/a.rs\0?? new file.rs\0R  src/b.rs\0src/old_b.rs\0\
                      ?? .claude/worktrees/one/\0";
        let expected: BTreeSet<String> = ["new file.rs", "src/a.rs", "src/b.rs"]
            .map(String::from)
            .into();
        assert_eq!(dirty_paths(status), expected);
    }

    #[tokio::test]
    async fn a_command_that_never_ends_is_given_up_on() {
        let mut stuck = Command::new("sleep");
        stuck.arg("30");
        let start = std::time::Instant::now();
        let err = output_within(stuck, std::time::Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("killed"), "{err}");
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        // One that ends in time is read as before.
        let mut quick = Command::new("echo");
        quick.arg("hi");
        let output = output_within(quick, std::time::Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(output.stdout, b"hi\n");
    }
}
