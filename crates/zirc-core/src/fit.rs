//! How another session's intent meets one's own work, file by file: what it has changed
//! under the paths it names, what one has changed there oneself, and, of the files both
//! change, whether the lines are the same. Told only that another was "about to change
//! web", sessions judged for themselves that it wouldn't touch their file, and were often
//! wrong; zircd knows both sides' files, and on one machine their lines, so it says.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::hunks;

/// How an intent meets the work of the session hearing it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Fit {
    /// The files the intending session has changed under the intent's paths, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub theirs: Vec<String>,
    /// The files the hearing session has changed under them, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mine: Vec<String>,
    /// Of the files both change, how.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shared: BTreeMap<String, Shared>,
    /// The hearing session's files the intent names themselves, not by a directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named: Vec<String>,
}

/// How two sessions both change one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Shared {
    /// On the same or neighbouring lines, these of their merge base: likely to conflict.
    SameLines(Vec<[u32; 2]>),
    /// In different parts: likely to merge.
    Apart,
    /// Which lines isn't known, as with another machine's.
    Unknown,
}

/// What an intent means for the session hearing it, the worst of its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    /// It has changed nothing under its paths yet, and names none of one's files itself.
    Ahead,
    /// It changes files there, none of them one's own.
    DifferentFiles,
    /// It changes some of one's files, in other parts of them, or only lockfiles.
    Elsewhere,
    /// It names one of one's files itself, and hasn't changed it yet.
    Named,
    /// Both change a file, the lines unknown.
    SameFiles,
    /// Both change the same lines of a file.
    SameLines,
}

impl Verdict {
    /// Whether it's worth asking the other session now: both have changed a file, on the
    /// same lines or on lines not known. The rest merge on their own or aren't written
    /// yet, and zircd says so if that changes. A plan that names a file isn't one: in five
    /// days' conflicts between sessions (20 to 24 Nov 2026) each side had been told of the
    /// other by its changes, and no plan had kept two apart.
    #[must_use]
    pub fn asks(self) -> bool {
        self >= Verdict::SameFiles
    }
}

impl Fit {
    /// What it means for the hearing session.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        let shared = self
            .shared
            .iter()
            .map(|(path, shared)| match shared {
                _ if hunks::is_lockfile(path) => Verdict::Elsewhere,
                Shared::SameLines(_) => Verdict::SameLines,
                Shared::Unknown => Verdict::SameFiles,
                Shared::Apart => Verdict::Elsewhere,
            })
            .max();
        let named = self
            .named
            .iter()
            .any(|path| !self.shared.contains_key(path) && !hunks::is_lockfile(path))
            .then_some(Verdict::Named);
        let changed = (!self.theirs.is_empty()).then_some(Verdict::DifferentFiles);
        [shared, named, changed]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(Verdict::Ahead)
    }

    /// It as the end of the intent's line: the verdict, then each side's files, as
    /// `different files: they change web/BUILD.bazel; you change web/e2e/a.ts`.
    #[must_use]
    pub fn text(&self) -> String {
        let verdict = match self.verdict() {
            Verdict::SameLines => {
                let lines: Vec<String> = self
                    .shared
                    .iter()
                    .filter_map(|(path, shared)| match shared {
                        Shared::SameLines(lines) if !hunks::is_lockfile(path) => {
                            Some(format!("{path} ({})", hunks::describe(lines)))
                        }
                        _ => None,
                    })
                    .collect();
                format!("same lines in {}", some_of(&lines))
            }
            Verdict::SameFiles => {
                let unknown: Vec<String> = self
                    .shared
                    .iter()
                    .filter(|(path, shared)| {
                        **shared == Shared::Unknown && !hunks::is_lockfile(path)
                    })
                    .map(|(path, _)| path.clone())
                    .collect();
                format!("both change {}, lines unknown", some_of(&unknown))
            }
            Verdict::Named => {
                let named: Vec<String> = self
                    .named
                    .iter()
                    .filter(|path| !self.shared.contains_key(*path))
                    .cloned()
                    .collect();
                format!("names your {}, not changed by them yet", some_of(&named))
            }
            Verdict::Elsewhere => {
                let shared: Vec<String> = self.shared.keys().cloned().collect();
                format!("elsewhere in {}", some_of(&shared))
            }
            Verdict::DifferentFiles => "different files".to_owned(),
            Verdict::Ahead => "nothing changed there by them yet".to_owned(),
        };
        let sides: Vec<String> = [("they change", &self.theirs), ("you change", &self.mine)]
            .into_iter()
            .filter(|(_, paths)| !paths.is_empty())
            .map(|(who, paths)| format!("{who} {}", some_of(paths)))
            .collect();
        if sides.is_empty() {
            verdict
        } else {
            format!("{verdict}: {}", sides.join("; "))
        }
    }
}

/// A few of `paths`, and how many more.
fn some_of(paths: &[String]) -> String {
    const SHOWN: usize = 4;
    match paths.len() {
        n if n <= SHOWN => paths.join(", "),
        n => format!("{} and {} more", paths[..SHOWN].join(", "), n - SHOWN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|&path| path.to_owned()).collect()
    }

    #[test]
    fn files_of_its_own_under_a_directory_are_different_files() {
        let fit = Fit {
            theirs: paths(&["web/BUILD.bazel"]),
            mine: paths(&["web/e2e/a.ts"]),
            ..Fit::default()
        };
        assert_eq!(fit.verdict(), Verdict::DifferentFiles);
        assert!(!fit.verdict().asks());
        assert_eq!(
            fit.text(),
            "different files: they change web/BUILD.bazel; you change web/e2e/a.ts"
        );
    }

    #[test]
    fn the_same_lines_say_which_and_ask() {
        let fit = Fit {
            theirs: paths(&["a.rs", "b.rs"]),
            mine: paths(&["a.rs", "b.rs"]),
            shared: [
                ("a.rs".to_owned(), Shared::SameLines(vec![[40, 58]])),
                ("b.rs".to_owned(), Shared::Apart),
            ]
            .into(),
            ..Fit::default()
        };
        assert_eq!(fit.verdict(), Verdict::SameLines);
        assert!(fit.verdict().asks());
        assert_eq!(
            fit.text(),
            "same lines in a.rs (40-58): they change a.rs, b.rs; you change a.rs, b.rs"
        );
    }

    #[test]
    fn different_parts_and_lockfiles_merge_on_their_own() {
        let fit = Fit {
            theirs: paths(&["Cargo.lock", "a.rs"]),
            mine: paths(&["Cargo.lock", "a.rs"]),
            shared: [
                ("Cargo.lock".to_owned(), Shared::SameLines(vec![[1, 2]])),
                ("a.rs".to_owned(), Shared::Apart),
            ]
            .into(),
            ..Fit::default()
        };
        assert_eq!(fit.verdict(), Verdict::Elsewhere);
        assert!(!fit.verdict().asks());
        assert!(fit.text().starts_with("elsewhere in Cargo.lock, a.rs: "));
    }

    #[test]
    fn another_machines_file_in_both_is_worth_asking_about() {
        let fit = Fit {
            theirs: paths(&["a.rs"]),
            mine: paths(&["a.rs"]),
            shared: [("a.rs".to_owned(), Shared::Unknown)].into(),
            ..Fit::default()
        };
        assert_eq!(fit.verdict(), Verdict::SameFiles);
        assert!(fit.verdict().asks());
        assert!(fit.text().starts_with("both change a.rs, lines unknown: "));
    }

    #[test]
    fn a_file_of_mine_named_before_they_start_is_only_a_plan() {
        let fit = Fit {
            mine: paths(&["src/a.rs"]),
            named: paths(&["src/a.rs"]),
            ..Fit::default()
        };
        assert_eq!(fit.verdict(), Verdict::Named);
        assert!(!fit.verdict().asks());
        assert_eq!(
            fit.text(),
            "names your src/a.rs, not changed by them yet: you change src/a.rs"
        );
    }

    #[test]
    fn a_directory_named_before_they_start_is_only_ahead() {
        let fit = Fit {
            mine: paths(&["src/a.rs"]),
            ..Fit::default()
        };
        assert_eq!(fit.verdict(), Verdict::Ahead);
        assert!(!fit.verdict().asks());
        assert_eq!(
            fit.text(),
            "nothing changed there by them yet: you change src/a.rs"
        );
    }
}
