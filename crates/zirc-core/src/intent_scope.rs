//! Whether an intent's path is broader than the change it announces. An intent's paths are
//! its session's for as long as it lasts: another session changing anything under one, or
//! saying it's about to, wakes it, and it wakes them. A whole crate named for a change to a
//! few of its files woke a session for another's change to a file it never touched (9 Nov
//! 2026), so `zirc intend` says so when a path holds many files.

use std::path::Path;

/// How many tracked files a directory may hold before an intent naming it is too broad.
pub const BROAD_FILES: usize = 40;

/// How many files git tracks under `path`, a directory in the repository containing `cwd`;
/// `None` for a file, a path that isn't there, or outside a repository.
#[must_use]
pub fn tracked_files(cwd: &Path, path: &Path) -> Option<usize> {
    if !cwd.join(path).is_dir() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["ls-files", "-z", "--"])
        .arg(path)
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    Some(
        out.stdout
            .split(|&byte| byte == 0)
            .filter(|name| !name.is_empty())
            .count(),
    )
}

/// What to say about `path`, holding `files` tracked files, if that's too many for an
/// intent.
#[must_use]
pub fn too_broad(path: &str, files: usize) -> Option<String> {
    (files > BROAD_FILES).then(|| {
        format!(
            "note: {path} holds {files} files: while this intent lasts, any session changing \
             anything under it wakes you, and you wake them. Name the files or the narrowest \
             directory you'll change, and say it again as your plan narrows.\n"
        )
    })
}

/// [`too_broad`] for each of `paths`, given relative to `cwd`, in order.
#[must_use]
pub fn warnings(cwd: &Path, paths: &[impl AsRef<Path>]) -> String {
    paths
        .iter()
        .filter_map(|path| {
            let path = path.as_ref();
            too_broad(&path.display().to_string(), tracked_files(cwd, path)?)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_of_many_files_is_too_broad_and_a_few_are_not() {
        assert!(too_broad("src", BROAD_FILES + 1).is_some());
        assert!(too_broad("src/auth", BROAD_FILES).is_none());
    }

    #[test]
    fn warns_of_directories_holding_many_tracked_files_only() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        for (dir, count) in [("big", BROAD_FILES + 1), ("small", 3)] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            for n in 0..count {
                std::fs::write(root.join(dir).join(format!("{n}.rs")), "").unwrap();
            }
        }
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        let said = warnings(root, &["big", "small", "big/0.rs", "gone"]);
        assert_eq!(said.lines().count(), 1, "{said}");
        assert!(
            said.starts_with(&format!("note: big holds {} files", BROAD_FILES + 1)),
            "{said}"
        );
    }
}
