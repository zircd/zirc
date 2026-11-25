//! Git repositories for tests of what zirc does with them.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// When every clone's first commit says it was made: one moment, so each has the same
/// commit, as clones of one repository do. Made at whatever time it was, two shared it only
/// when made within a second, and whether zircd could compare two machines' lines, which
/// takes the commit one's are counted from, was a matter of timing.
const STARTED: &str = "2026-01-01T00:00:00Z";

/// A clone of a project whose remote is `remote`: a repository on `main` with one commit,
/// a `README.md`, the same commit in every clone.
///
/// # Panics
///
/// When Git can't make it.
#[must_use]
pub fn clone(remote: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    git(dir.path(), &["init", "--quiet", "--initial-branch=main"]);
    git(dir.path(), &["remote", "add", "origin", remote]);
    std::fs::write(dir.path().join("README.md"), "api\n").expect("writing README.md");
    git(dir.path(), &["add", "README.md"]);
    commit(dir.path(), "Start");
    dir
}

/// Commits what's staged in `dir` as `message`, dated so that clones committing the same
/// have the same commit ([`STARTED`]).
///
/// # Panics
///
/// When Git fails.
pub fn commit(dir: &Path, message: &str) {
    run(
        dir,
        &["commit", "--quiet", "--message", message],
        Some(STARTED),
    );
}

/// Runs `git args` in `dir`, as a test author, whatever the machine's Git config says.
///
/// # Panics
///
/// When Git fails.
pub fn git(dir: &Path, args: &[&str]) {
    run(dir, args, None);
}

/// `git args` in `dir`, a commit it makes dated `at` when that's given.
fn run(dir: &Path, args: &[&str], at: Option<&str>) {
    let mut git = Command::new("git");
    git.args(["-c", "user.name=Test", "-c", "user.email=test@zirc.dev"])
        .args(args)
        .current_dir(dir);
    if let Some(at) = at {
        git.env("GIT_AUTHOR_DATE", at).env("GIT_COMMITTER_DATE", at);
    }
    let status = git.status().expect("running git");
    assert!(status.success(), "git {args:?}");
}
