//! Whether a branch takes out what someone else landed: the change a conflict's resolution
//! most often loses, keeping its own side and dropping theirs, while the tests still pass (the
//! agent evals' S4, 16 Nov 2026, in every cell). A line by line comparison would say so of
//! every merge, since keeping both sides edits their line too; what tells the two apart is
//! whether the words their change brought survive in what replaced it.

use std::collections::BTreeSet;

/// One hunk of a diff: in `path`, these lines taken out and these put in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The file, from the repository's root.
    pub path: String,
    /// The lines it takes out.
    pub removed: Vec<String>,
    /// The lines it puts in.
    pub added: Vec<String>,
}

/// The hunks of a unified diff, as `git diff -U0` and `git show -U0` write them.
#[must_use]
pub fn parse(diff: &str) -> Vec<Change> {
    let mut changes: Vec<Change> = Vec::new();
    let mut path = String::new();
    for line in diff.lines() {
        if line.starts_with("--- ") || line.starts_with("diff ") {
            continue;
        }
        if let Some(to) = line.strip_prefix("+++ ") {
            to.strip_prefix("b/").unwrap_or(to).clone_into(&mut path);
        } else if line.starts_with("@@") {
            changes.push(Change {
                path: path.clone(),
                removed: Vec::new(),
                added: Vec::new(),
            });
        } else if let Some(change) = changes.last_mut() {
            if let Some(gone) = line.strip_prefix('-') {
                change.removed.push(gone.to_owned());
            } else if let Some(new) = line.strip_prefix('+') {
                change.added.push(new.to_owned());
            }
        }
    }
    changes
}

/// The words of `lines` that say something: identifiers of three letters or more.
fn words<'a>(lines: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    lines
        .into_iter()
        .flat_map(|line| line.split(|c: char| !(c.is_alphanumeric() || c == '_')))
        .filter(|word| word.chars().count() >= 3 && !word.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_lowercase)
        .collect()
}

/// What of `theirs`, a change someone landed, `ours` takes out, if anything: a line their
/// change put in that ours removes, where nothing ours puts in its place keeps a word their
/// change brought. `None` when ours keeps their change, or doesn't touch it.
#[must_use]
pub fn undone<'a>(theirs: &'a [Change], ours: &[Change]) -> Option<(&'a str, &'a str)> {
    for their in theirs {
        let brought: BTreeSet<String> = words(&their.added)
            .difference(&words(&their.removed))
            .cloned()
            .collect();
        if brought.is_empty() {
            continue;
        }
        for our in ours.iter().filter(|our| our.path == their.path) {
            let Some(line) = their.added.iter().find(|line| {
                !line.trim().is_empty() && our.removed.iter().any(|gone| gone.trim() == line.trim())
            }) else {
                continue;
            };
            if words(&our.added).is_disjoint(&brought) {
                return Some((&their.path, line.trim()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAROL: &str = "\
diff --git a/shop/report.py b/shop/report.py
--- a/shop/report.py
+++ b/shop/report.py
@@ -9 +9 @@ def summary(orders):
-    return f\"{len(orders)} orders, {cents // 100}.{cents % 100:02d} in all\"
+    return f\"orders: {len(orders)}, total: {cents // 100}.{cents % 100:02d}\"
";

    fn ours(line: &str) -> Vec<Change> {
        parse(&format!(
            "--- a/shop/report.py\n+++ b/shop/report.py\n@@ -9 +9 @@\n-    return f\"orders: {{len(orders)}}, total: {{cents // 100}}.{{cents % 100:02d}}\"\n+{line}\n"
        ))
    }

    #[test]
    fn says_when_a_resolution_keeps_only_its_own_side() {
        let theirs = parse(CAROL);
        assert_eq!(theirs.len(), 1);
        let own_side = ours("    return f\"{len(orders)} orders, {format_cents(cents)} in all\"");
        let (path, line) = undone(&theirs, &own_side).expect("their words are gone");
        assert_eq!(path, "shop/report.py");
        assert!(
            line.starts_with("return f\"orders: {len(orders)}, total:"),
            "{line}"
        );
    }

    #[test]
    fn keeps_quiet_when_both_sides_are_kept() {
        let theirs = parse(CAROL);
        let both = ours("    return f\"orders: {len(orders)}, total: {format_cents(cents)}\"");
        assert_eq!(undone(&theirs, &both), None);
    }

    #[test]
    fn keeps_quiet_about_lines_ours_doesnt_touch() {
        let theirs = parse(CAROL);
        let elsewhere =
            parse("--- a/shop/report.py\n+++ b/shop/report.py\n@@ -2 +2 @@\n-x = 1\n+x = 2\n");
        assert_eq!(undone(&theirs, &elsewhere), None);
    }
}
