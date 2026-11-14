//! Git repositories for tests of what zirc does with them.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// A clone of a project whose remote is `remote`: a repository on `main` with one commit,
/// a `README.md`.
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
    git(dir.path(), &["commit", "--quiet", "--message", "Start"]);
    dir
}

/// Runs `git args` in `dir`, as a test author, whatever the machine's Git config says.
///
/// # Panics
///
/// When Git fails.
pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@zirc.dev"])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("running git");
    assert!(status.success(), "git {args:?}");
}
