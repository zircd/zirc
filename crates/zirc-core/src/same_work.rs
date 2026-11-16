//! Whether two tasks, as sessions said them in their `taking:` posts, look like the same work.
//! Two people asking for one thing in their own words got it built twice: the second session
//! never learnt that the first had taken it on (the agent evals' C5, 16 Nov 2026).
//!
//! The measure is word overlap, over the words that say what the work is about: a post's
//! words, lowercased, with function words and the verbs and nouns any task says ("add", "fix",
//! "support", "file", …) taken out, and plurals and `-ing` / `-ed` endings cut off. Two tasks
//! are alike when they share at least two such words, and those make up more than half of the
//! shorter task's ([`alike`]), or when both say the same words and nothing else.
//!
//! Why these numbers: one shared word is noise ("fix the orders page layout" and "add a CSV
//! export of orders" share only "order"); an overlap measured against the longer task, as
//! Jaccard's is, misses a rewording that adds a few words of its own ("let staff download
//! orders as a spreadsheet file (csv)" shares two of "add a CSV export of orders"' three, but
//! only two of the seven words in both). Measuring against the shorter one catches that, and
//! two shared words keep a single common term from matching on its own. More than half, not
//! half: "settings page dark mode" and "speed up the settings page queries" share two of
//! four, a place in the app rather than the work. It has no synonyms,
//! so "spreadsheet" and "csv" count only where both say one; and it can't tell an import from
//! an export of the same thing, which it calls alike: a question to the other session costs a
//! message, building the same thing twice a whole task.

use std::collections::BTreeSet;

/// Function words.
const FUNCTION_WORDS: &str = "\
     a about after again all also an and any are as at be been before being but by can \
     could do does done each etc every for from has have here how if in into is it its \
     just me more most my no not now of on onto only or our out over per should so some \
     still than that the their them then there these they this those to under up us via \
     vs was we were what when where which while who why will with would you your";

/// What any task says.
const ANY_TASK_SAYS: &str = "\
     able add added adding allow build change correct correctly create feature file fix \
     fixed fixing get handle implement improve issue let make need new proper properly \
     refactor remove set support sure taking task thing try update use want way work \
     write wrong wrongly";

/// Whether `word` says nothing of what a task is about: a function word, or a verb or noun
/// any task might use. Matched against a word as written and as [`stem`] cuts it.
fn says_nothing(word: &str) -> bool {
    FUNCTION_WORDS
        .split_whitespace()
        .chain(ANY_TASK_SAYS.split_whitespace())
        .any(|it| it == word)
}

/// The fewest words two tasks must share to be alike.
const SHARED_AT_LEAST: usize = 2;

/// The words of `task`, a `taking:` post or what it says, that say what the work is about,
/// each cut to its stem. "taking" is one that says nothing.
#[must_use]
pub fn words(task: &str) -> BTreeSet<String> {
    task.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| word.chars().count() >= 2 && !word.chars().all(|c| c.is_ascii_digit()))
        .filter(|word| !says_nothing(word))
        .map(|word| stem(&word))
        .filter(|word| !says_nothing(word))
        .collect()
}

/// `word`, lowercase, with a plural's or a verb's ending cut off, so a rewording that says
/// "orders" or "rounding" meets one that says "order" or "round". Never shorter than four
/// letters for a verb's ending, nor three for a plural's, so "string", "speed" and "gas" stay
/// whole and "boxes" is "box".
fn stem(word: &str) -> String {
    const SHORTEST: usize = 4;
    let cut = |end: &str, put: &str| {
        word.strip_suffix(end)
            .filter(|rest| rest.chars().count() + put.chars().count() >= SHORTEST - 1)
            .map(|rest| format!("{rest}{put}"))
    };
    let plain = if word.ends_with("ss") || word.ends_with("us") || word.ends_with("is") {
        None
    } else if word.ends_with("ies") {
        cut("ies", "y")
    } else if word.ends_with("xes") || word.ends_with("sses") {
        cut("es", "")
    } else {
        cut("s", "")
    };
    let plain = plain.unwrap_or_else(|| word.to_owned());
    let word = plain.as_str();
    word.strip_suffix("ing")
        .or_else(|| word.strip_suffix("ed"))
        .filter(|rest| rest.chars().count() >= SHORTEST)
        .map_or_else(|| word.to_owned(), str::to_owned)
}

/// Whether tasks `a` and `b`, `taking:` posts or what they say, look like the same work
/// (the module's doc says how, and why).
#[must_use]
pub fn alike(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    let shorter = a.len().min(b.len());
    let shared = a.intersection(&b).count();
    if shorter == 0 {
        return false;
    }
    (shared >= SHARED_AT_LEAST && shared * 2 > shorter) || a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same work, as two people might ask for it.
    const ALIKE: &[(&str, &str)] = &[
        (
            "taking: add a CSV export of orders",
            "taking: let staff download orders as a spreadsheet file (csv)",
        ),
        (
            "taking: fix tax rounding on invoices",
            "taking: invoice totals round tax wrongly: fix the rounding",
        ),
        (
            "taking: dark mode for the settings page",
            "taking: settings page: support a dark theme",
        ),
        (
            "taking: migrate CI to the new runners",
            "taking: move the CI jobs onto the new runner pool",
        ),
        (
            "taking: retry flaky uploads in the media worker",
            "taking: media worker uploads fail sometimes, retry them",
        ),
        ("taking: Orders CSV export", "taking: orders csv exporting"),
        ("taking: changelog", "taking: update the CHANGELOG"),
    ];

    /// Different work in the same repository, some of it sharing a word.
    const DIFFERENT: &[(&str, &str)] = &[
        ("taking: add CSV export", "taking: fix tax rounding"),
        (
            "taking: add a CSV export of orders",
            "taking: fix the orders page layout",
        ),
        (
            "taking: fix flaky login test",
            "taking: add tests for the CSV export",
        ),
        ("taking: update the README", "taking: update dependencies"),
        (
            "taking: rename the user model",
            "taking: add a user avatar to the profile page",
        ),
        (
            "taking: settings page dark mode",
            "taking: speed up the settings page queries",
        ),
        ("taking: docs", "taking: fix the docs build on Windows"),
        ("taking: fix it", "taking: fix it"),
    ];

    #[test]
    fn rewordings_of_the_same_work_are_alike_both_ways() {
        for (a, b) in ALIKE {
            assert!(
                alike(a, b),
                "{a:?} and {b:?}: {:?} {:?}",
                words(a),
                words(b)
            );
            assert!(alike(b, a), "{b:?} and {a:?}");
        }
    }

    #[test]
    fn different_work_is_not_both_ways() {
        for (a, b) in DIFFERENT {
            assert!(
                !alike(a, b),
                "{a:?} and {b:?}: {:?} {:?}",
                words(a),
                words(b)
            );
            assert!(!alike(b, a), "{b:?} and {a:?}");
        }
    }

    #[test]
    fn words_leave_out_what_any_task_says_and_cut_endings() {
        let words = |task: &str| words(task).into_iter().collect::<Vec<_>>();
        assert_eq!(
            words("Taking: Add the CSV exports of orders, and fix 2 queries"),
            ["csv", "export", "order", "query"]
        );
        assert_eq!(words("rounding exported boxes"), ["box", "export", "round"]);
        assert_eq!(
            words("string speed status class"),
            ["class", "speed", "status", "string"]
        );
        assert!(words("taking: fix it").is_empty());
    }
}
