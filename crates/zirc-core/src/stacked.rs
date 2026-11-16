//! What a branch is stacked on, from git alone: the unlanded branches on origin its head is
//! built on. `zirc stack` keeps them as stacks the session said; zircd says, with a push, the
//! one it's built on, so whoever works on that one hears that someone builds on it.

use std::path::Path;
use std::process::Command;

/// The branches on origin (refs/remotes/origin/*, as last fetched) that `head` at `dir` is
/// stacked on, `(branch, head)`: those whose head `head` holds and `origin/<default>` doesn't,
/// with a commit it doesn't have as a patch either (`git cherry`), so a branch that landed,
/// rebased onto it, isn't one. Not `own`, `head`'s own branch, nor the default branch. Only a
/// base held whole: one that took more commits after `head` was built on it looks to git just
/// as `head` built beside it. A copy of `ops/land.sh`'s `stacked_on`, which CI runs without
/// zirc: change both.
#[must_use]
pub fn stacked_on(
    dir: &Path,
    default: &str,
    own: Option<&str>,
    head: &str,
) -> Vec<(String, String)> {
    let main = format!("refs/remotes/origin/{default}");
    let Some(refs) = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/remotes/origin/",
        ],
    ) else {
        return Vec::new();
    };
    let holds = |a: &str, b: &str| {
        Command::new("git")
            .current_dir(dir)
            .args(["merge-base", "--is-ancestor", a, b])
            .output()
            .is_ok_and(|out| out.status.success())
    };
    let mut found = Vec::new();
    for line in refs.lines() {
        let Some((name, tip)) = line.trim().split_once(' ') else {
            continue;
        };
        let branch = name.trim_start_matches("refs/remotes/origin/");
        if branch == default || branch == "HEAD" || Some(branch) == own || tip == head {
            continue;
        }
        if !holds(tip, head) || holds(tip, &main) {
            continue;
        }
        let unlanded = git(dir, &["cherry", &main, tip])
            .is_some_and(|out| out.lines().any(|line| line.trim().starts_with('+')));
        if unlanded {
            found.push((branch.to_owned(), tip.to_owned()));
        }
    }
    found
}

/// What git says at `dir`, when it succeeds.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A clone of a bare origin with master's first commit, in `root`.
    fn repo(root: &Path) -> std::path::PathBuf {
        let (origin, work) = (root.join("origin.git"), root.join("work"));
        sh(
            root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "master",
                origin.to_str().unwrap(),
            ],
        );
        sh(
            root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        sh(&work, &["config", "user.email", "t@example.com"]);
        sh(&work, &["config", "user.name", "t"]);
        sh(&work, &["symbolic-ref", "HEAD", "refs/heads/master"]);
        commit(&work, "m");
        sh(&work, &["push", "-q", "origin", "master"]);
        work
    }

    /// A commit adding `file`, its subject `file`.
    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        sh(dir, &["add", file]);
        sh(dir, &["commit", "-qm", file]);
    }

    /// `branch` from `from` with a commit of its own, pushed.
    fn pushed(dir: &Path, branch: &str, from: &str) {
        sh(dir, &["checkout", "-q", "-b", branch, from]);
        commit(dir, branch);
        sh(dir, &["push", "-q", "origin", branch]);
    }

    /// `branch` lands: its commits put onto master as new ones, as a landing's rebase does,
    /// onto another landing's: onto master alone, in the same second, a cherry-pick would
    /// make the very same commit.
    fn lands(dir: &Path, branch: &str) {
        sh(dir, &["checkout", "-q", "master"]);
        commit(dir, &format!("before-{branch}"));
        sh(dir, &["cherry-pick", branch]);
        sh(dir, &["push", "-q", "origin", "master"]);
        sh(dir, &["fetch", "-q", "origin"]);
    }

    #[test]
    fn finds_what_a_branch_is_stacked_on_from_git_alone() {
        let root = tempfile::tempdir().unwrap();
        let work = repo(root.path());
        pushed(&work, "a", "master");
        pushed(&work, "b", "a");
        pushed(&work, "l", "master");
        pushed(&work, "d", "l");
        lands(&work, "l");
        let on = |branch: &str| {
            let head = sh(&work, &["rev-parse", branch]);
            stacked_on(&work, "master", Some(branch), &head)
                .into_iter()
                .map(|(branch, _)| branch)
                .collect::<Vec<_>>()
        };
        assert_eq!(on("b"), ["a"], "b is built on a");
        assert!(
            on("a").is_empty(),
            "a is built on nothing, b being built on it"
        );
        assert!(on("d").is_empty(), "d is built on l, which landed");
    }
}
