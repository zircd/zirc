//! Which lines two changes to the same files both touch: a cheap prediction of whether
//! they'll merge. Each side is a diff against the same base, their merge base, so line
//! numbers in the base line up; changes that overlap or adjoin there are what Git can't
//! merge on its own.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Lines of a file's base version that a change replaces: from `start` (counting from 1),
/// `len` of them. A pure insertion has no lines and goes after line `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    /// The first line replaced, or the one an insertion follows (0 for the top).
    pub start: u32,
    /// How many lines are replaced.
    pub len: u32,
    /// A hash of the lines it removes and adds, telling two changes to the same lines
    /// apart; 0 when only the lines are known, as from a push event.
    pub patch: u64,
}

impl Hunk {
    /// The hunk replacing `len` lines from `start`, its text unknown.
    #[must_use]
    pub fn at(start: u32, len: u32) -> Self {
        Self {
            start,
            len,
            patch: 0,
        }
    }

    /// The base lines it touches, first and last. An insertion touches the lines on either
    /// side of it.
    fn span(self) -> (u32, u32) {
        if self.len == 0 {
            (self.start, self.start + 1)
        } else {
            (self.start, self.start + self.len - 1)
        }
    }
}

/// `theirs` without the hunks `landed` has too, the same lines changed the same way, both
/// counted in the same base: what a push changes that the default branch didn't already.
/// A hunk changing lines `landed` changes, but to other text, is the pusher's own.
#[must_use]
pub fn without(theirs: &Hunks, landed: &Hunks) -> Hunks {
    theirs
        .iter()
        .map(|(path, hunks)| {
            let gone = landed.get(path).map_or(&[][..], Vec::as_slice);
            let kept: Vec<Hunk> = hunks
                .iter()
                .copied()
                .filter(|hunk| !gone.contains(hunk))
                .collect();
            (path.clone(), kept)
        })
        .filter(|(_, hunks)| !hunks.is_empty())
        .collect()
}

/// Each file's hunks, by path relative to the repository's root.
pub type Hunks = BTreeMap<String, Vec<Hunk>>;

/// The hunks in the output of `git diff --unified=0 --no-prefix`, by the path each file
/// had or has. Anything else in the output is skipped.
#[must_use]
pub fn parse(diff: &str) -> Hunks {
    let mut hunks = Hunks::new();
    let mut old: Option<&str> = None;
    let mut current: Option<String> = None;
    // The hunk being read: its file, itself, the hash of its lines so far, and how many of
    // its removed and added lines are still to come.
    let mut open: Option<(String, Hunk, DefaultHasher, u32)> = None;
    for line in diff.lines() {
        if let Some((_, _, hasher, left)) = &mut open {
            // A `--- ` or `+++ ` here is a removed or added line, not a file's header.
            if *left > 0 || line.starts_with('\\') {
                line.hash(hasher);
                *left = left.saturating_sub(u32::from(!line.starts_with('\\')));
                continue;
            }
        }
        if let Some((path, mut hunk, hasher, _)) = open.take() {
            hunk.patch = hasher.finish();
            hunks.entry(path).or_default().push(hunk);
        }
        if let Some(path) = line.strip_prefix("--- ") {
            old = (path != "/dev/null").then_some(path);
        } else if let Some(path) = line.strip_prefix("+++ ") {
            current = if path == "/dev/null" { old } else { Some(path) }.map(str::to_owned);
        } else if let (Some(header), Some(path)) = (line.strip_prefix("@@ -"), &current) {
            let mut sides = header.split(' ');
            if let Some(hunk) = sides.next().and_then(parse_range)
                && let Some(added) = sides
                    .next()
                    .and_then(|side| side.strip_prefix('+'))
                    .and_then(parse_range)
            {
                let lines = hunk.len + added.len;
                open = Some((path.clone(), hunk, DefaultHasher::new(), lines));
            }
        }
    }
    if let Some((path, mut hunk, hasher, _)) = open {
        hunk.patch = hasher.finish();
        hunks.entry(path).or_default().push(hunk);
    }
    hunks
}

/// `12,3` or `12`, a side of a hunk header.
fn parse_range(range: &str) -> Option<Hunk> {
    let (start, len) = range.split_once(',').unwrap_or((range, "1"));
    Some(Hunk::at(start.parse().ok()?, len.parse().ok()?))
}

/// The base lines, first and last, where `ours` and `theirs` both change a file or change
/// lines next to each other's, merged into spans.
#[must_use]
pub fn clashing(ours: &[Hunk], theirs: &[Hunk]) -> Vec<[u32; 2]> {
    let mut spans: Vec<[u32; 2]> = Vec::new();
    for a in ours.iter().map(|hunk| hunk.span()) {
        for b in theirs.iter().map(|hunk| hunk.span()) {
            // Overlapping, or with no unchanged line between them.
            if a.0 <= b.1 + 1 && b.0 <= a.1 + 1 {
                spans.push([a.0.min(b.0), a.1.max(b.1)]);
            }
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<[u32; 2]> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span[0] <= last[1] + 1 => last[1] = last[1].max(span[1]),
            _ => merged.push(span),
        }
    }
    merged
}

/// What two sides changing the same files amounts to, path by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prediction {
    /// Files both change on the same or neighbouring lines, with those lines of the base:
    /// likely to conflict.
    pub same_lines: BTreeMap<String, Vec<[u32; 2]>>,
    /// Files both change, but in different parts: likely to merge.
    pub apart: Vec<String>,
}

/// The prediction for `paths`, from each side's hunks against their merge base. A path
/// missing from either side, such as a new file Git doesn't track yet, is in neither list:
/// there's nothing to compare.
#[must_use]
pub fn predict<'a>(
    paths: impl IntoIterator<Item = &'a String>,
    ours: &Hunks,
    theirs: &Hunks,
) -> Prediction {
    let mut prediction = Prediction::default();
    for path in paths {
        let (Some(ours), Some(theirs)) = (ours.get(path), theirs.get(path)) else {
            continue;
        };
        let lines = clashing(ours, theirs);
        if lines.is_empty() {
            prediction.apart.push(path.clone());
        } else {
            prediction.same_lines.insert(path.clone(), lines);
        }
    }
    prediction
}

/// "40-58, 90": spans of lines, for people.
#[must_use]
pub fn describe(lines: &[[u32; 2]]) -> String {
    lines
        .iter()
        .map(|[first, last]| {
            if first == last {
                first.to_string()
            } else {
                format!("{first}-{last}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `path` is a lockfile, which a tool writes: two branches changing the same lines
/// of one don't need to agree on anything, the second just writes it again.
#[must_use]
pub fn is_lockfile(path: &str) -> bool {
    let path = std::path::Path::new(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    path.extension()
        .is_some_and(|extension| extension == "lock")
        || [
            "package-lock.json",
            "pnpm-lock.yaml",
            "go.sum",
            "maven_install.json",
        ]
        .contains(&name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn leaves_out_what_the_default_branch_already_changed() {
        let landed = parse("--- a.rs\n+++ a.rs\n@@ -10,2 +10 @@\n-x\n-y\n+z\n");
        let theirs = parse(
            "--- a.rs\n+++ a.rs\n@@ -10,2 +10 @@\n-x\n-y\n+z\n@@ -40 +39 @@\n-a\n+b\n\
             --- b.rs\n+++ b.rs\n@@ -5,0 +6 @@\n+--- not a header\n",
        );
        let kept = without(&theirs, &landed);
        assert_eq!(kept.keys().collect::<Vec<_>>(), ["a.rs", "b.rs"]);
        assert_eq!(ranges(&kept["a.rs"]), [(40, 1)]);
        assert_eq!(ranges(&kept["b.rs"]), [(5, 0)]);
        assert_eq!(without(&theirs, &Hunks::new()), theirs);
    }

    #[test]
    fn keeps_the_pushers_own_edit_to_lines_the_default_branch_changed() {
        // Master turned line 3 into `master();`; the push, merged with master, has its own
        // `mine();` there. Same lines, other text: the pusher's, and a conflict to warn of.
        let landed = parse("--- a.rs\n+++ a.rs\n@@ -3 +3 @@\n-old();\n+master();\n");
        let theirs = parse("--- a.rs\n+++ a.rs\n@@ -3 +3 @@\n-old();\n+mine();\n");
        assert_eq!(without(&theirs, &landed), theirs);
        assert_eq!(without(&landed, &landed), Hunks::new());
    }

    use super::*;

    const DIFF: &str = "\
diff --git src/a.rs src/a.rs
index 1111111..2222222 100644
--- src/a.rs
+++ src/a.rs
@@ -3 +3 @@ fn a() {
-    old();
+    new();
@@ -10,0 +11,2 @@ fn b() {
+    added();
+    more();
diff --git gone.rs gone.rs
deleted file mode 100644
--- gone.rs
+++ /dev/null
@@ -1,4 +0,0 @@
-x
-y
-z
-w
";

    fn hunk(start: u32, len: u32) -> Hunk {
        Hunk::at(start, len)
    }

    fn ranges(hunks: &[Hunk]) -> Vec<(u32, u32)> {
        hunks.iter().map(|hunk| (hunk.start, hunk.len)).collect()
    }

    #[test]
    fn reads_git_diff_hunks() {
        let hunks = parse(DIFF);
        assert_eq!(ranges(&hunks["src/a.rs"]), [(3, 1), (10, 0)]);
        assert_eq!(ranges(&hunks["gone.rs"]), [(1, 4)]);
    }

    #[test]
    fn finds_changes_to_the_same_or_neighbouring_lines() {
        // Lines 3-5 against line 5: the same line.
        assert_eq!(clashing(&[hunk(3, 3)], &[hunk(5, 1)]), [[3, 5]]);
        // Line 3 against line 4: no unchanged line between, which Git won't merge.
        assert_eq!(clashing(&[hunk(3, 1)], &[hunk(4, 1)]), [[3, 4]]);
        // Line 3 against line 5: line 4 stays between them.
        assert!(clashing(&[hunk(3, 1)], &[hunk(5, 1)]).is_empty());
        // An insertion after line 10 against a change to line 11.
        assert_eq!(clashing(&[hunk(10, 0)], &[hunk(11, 1)]), [[10, 11]]);
        assert!(clashing(&[hunk(10, 0)], &[hunk(20, 2)]).is_empty());
    }

    #[test]
    fn says_which_files_clash_and_which_merge() {
        let ours = parse(DIFF);
        let theirs: Hunks = [
            ("src/a.rs".to_owned(), vec![hunk(40, 2)]),
            ("gone.rs".to_owned(), vec![hunk(2, 1)]),
        ]
        .into();
        let paths = ["src/a.rs", "gone.rs", "new.rs"].map(String::from);
        let prediction = predict(&paths, &ours, &theirs);
        assert_eq!(prediction.apart, ["src/a.rs"]);
        assert_eq!(
            prediction.same_lines,
            [("gone.rs".to_owned(), vec![[1, 4]])].into()
        );
        assert_eq!(describe(&[[40, 58], [90, 90]]), "40-58, 90");
    }
}
