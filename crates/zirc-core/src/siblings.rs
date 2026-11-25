//! What a session is told of its user's other sessions on the same machine, as it says what
//! it's about to change (`zirc intend`): what each has taken on, where, and its files.
//!
//! Someone starting a third session tells it what the other two are on; left untold, it
//! finds out when they overlap, and treats their work as a stranger's. In the agent evals
//! (17 Nov 2026) the arm with no zirc whose sessions were briefed this way by hand did as
//! well as zirc's on what a briefing covers, and zirc knew all of it and said none.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::ipc::ChannelPresence;

/// How many sessions are listed, and how many paths of each.
const SESSIONS: usize = 6;
const PATHS: usize = 5;

/// One of the user's other sessions, as the briefing says it.
#[derive(Default)]
struct Sibling {
    name: Option<String>,
    task: Option<String>,
    branch: Option<String>,
    paths: Vec<String>,
}

/// The briefing for `session` on `machine` from `channels`, the machine's presence: a
/// heading and a line for each other session there that has taken a task, said what it's
/// about to change or has changes. Empty when there's none.
#[must_use]
pub fn brief(channels: &[ChannelPresence], machine: &str, session: &str) -> String {
    let mut siblings: BTreeMap<&str, Sibling> = BTreeMap::new();
    let theirs = |nick: &str, other: &str| nick == machine && other != session && !other.is_empty();
    for channel in channels {
        for claim in &channel.claims {
            if theirs(&claim.nick, &claim.session) {
                let sibling = siblings.entry(&claim.session).or_default();
                sibling.name = sibling.name.take().or_else(|| claim.session_name.clone());
                sibling.task = Some(claim.task.clone());
            }
        }
        for intent in &channel.intents {
            if theirs(&intent.nick, &intent.session) {
                let sibling = siblings.entry(&intent.session).or_default();
                sibling.name = sibling.name.take().or_else(|| intent.session_name.clone());
                sibling.task.get_or_insert_with(|| intent.summary.clone());
                add(&mut sibling.paths, &intent.paths);
            }
        }
        for member in &channel.members {
            let Some(other) = member.session.as_deref() else {
                continue;
            };
            if theirs(&member.nick, other) {
                let sibling = siblings.entry(other).or_default();
                sibling.name = sibling.name.take().or_else(|| member.session_name.clone());
                sibling.branch = sibling.branch.take().or_else(|| member.branch.clone());
                add(&mut sibling.paths, &member.paths);
            }
        }
    }
    if siblings.is_empty() {
        return String::new();
    }
    // Told only to build around their work, sessions whose task held part of another's
    // built it a second time in files of their own, and said so at the end (the agent
    // evals' PB1 probe, 17 Nov 2026).
    let mut text = String::from(
        "your user's other sessions on this machine. If one is already making part of what you \
         were asked for, don't make it a second time, in other files either: do the rest of \
         your task, wait for theirs (`zirc wait landed <its session>`), then build on it. \
         Otherwise work around its files\n",
    );
    for (session, sibling) in siblings.iter().take(SESSIONS) {
        let _ = write!(text, "  {session}");
        if let Some(name) = &sibling.name {
            let _ = write!(text, " ({name})");
        }
        if let Some(task) = &sibling.task {
            let _ = write!(text, ": {task}");
        }
        if let Some(branch) = &sibling.branch {
            let _ = write!(text, "; on {branch}");
        }
        if !sibling.paths.is_empty() {
            let _ = write!(
                text,
                "; changing {}",
                zirc_proto::some_of(&sibling.paths, PATHS, ", ")
            );
        }
        text.push('\n');
    }
    if siblings.len() > SESSIONS {
        let _ = writeln!(
            text,
            "  and {} more: `zirc who` lists them",
            siblings.len() - SESSIONS
        );
    }
    text
}

/// `more` added to `paths`, each once.
fn add(paths: &mut Vec<String>, more: &[String]) {
    for path in more {
        if !paths.contains(path) {
            paths.push(path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(json: serde_json::Value) -> Vec<ChannelPresence> {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn says_what_each_other_session_of_the_machine_is_on() {
        let channels = presence(serde_json::json!([{
            "channel": "#shop",
            "members": [
                {"nick": "ada-mbp", "branch": "coupons", "session": "cc-2", "paths": ["shop/pricing.py"]},
                {"nick": "ada-mbp", "branch": "gift", "session": "cc-1", "paths": ["shop/orders.py"]},
                {"nick": "bob-mbp", "branch": "tax", "session": "cc-9", "paths": ["shop/tax.py"]},
            ],
            "claims": [{"nick": "ada-mbp", "session": "cc-2", "session_name": "coupons", "task": "Coupons at checkout", "since": 1}],
            "intents": [{"nick": "ada-mbp", "session": "cc-2", "summary": "coupons", "paths": ["shop/pricing.py", "tests/test_pricing.py"]}],
        }]));
        let said = brief(&channels, "ada-mbp", "cc-1");
        assert!(
            said.starts_with("your user's other sessions on this machine"),
            "{said}"
        );
        assert!(
            said.contains("  cc-2 (coupons): Coupons at checkout; on coupons; changing shop/pricing.py, tests/test_pricing.py\n"),
            "{said}"
        );
        // Not its own, nor another machine's.
        assert!(!said.contains("cc-1") && !said.contains("cc-9"), "{said}");
    }

    #[test]
    fn says_nothing_with_no_other_session() {
        let channels = presence(serde_json::json!([{
            "channel": "#shop",
            "members": [{"nick": "ada-mbp", "branch": "gift", "session": "cc-1", "paths": ["shop/orders.py"]}],
        }]));
        assert_eq!(brief(&channels, "ada-mbp", "cc-1"), "");
    }
}
