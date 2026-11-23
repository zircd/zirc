//! Messages by their ids (`msgid`), as far as a viewer may see them: what zirc.dev's Insights
//! asks for to show the lines behind a number, from the person's browser through the content
//! API (`GET /api/v1/history?ids=…`). The activity store keeps only ids; the words come from
//! Ergo's history here, by the same rule as every other read ([`Viewer::may_see`]).
//!
//! An id that comes back with nothing is gone (retention, a redaction, a trim) or one the
//! viewer may not read: the answer doesn't say which, so it tells nobody that a conversation
//! they can't see exists.

use std::collections::HashSet;
use std::path::Path;

use super::{Conversation, Cursor, Message, Query, Row, Viewer, read};

/// The most ids one request may ask for.
pub const MOST: usize = 100;

/// The messages of `ids` that `viewer` may see, oldest first, each once; at most [`MOST`] ids
/// are looked at. zircd's events and notices come back too: a number's lines are often theirs.
///
/// # Errors
///
/// When the history can't be read.
pub fn by_ids(history: &Path, viewer: &Viewer, ids: &[String]) -> rusqlite::Result<Vec<Message>> {
    if !history.exists() {
        return Ok(Vec::new());
    }
    let connection = crate::trim::open(history)?;
    // A message in a channel is one row of the sequence; a direct message is one row for each
    // of its two accounts, read here as one conversation.
    let mut channels = connection.prepare_cached(
        "SELECT s.nanotime, h.id, h.msgid, h.data, s.target, NULL FROM sequence s \
         JOIN history h ON h.id = s.history_id WHERE s.target LIKE '#%' AND h.msgid = ?1",
    )?;
    let mut direct = connection.prepare_cached(
        "SELECT c.nanotime, h.id, h.msgid, h.data, c.target, c.correspondent \
         FROM conversations c JOIN history h ON h.id = c.history_id WHERE h.msgid = ?1",
    )?;
    let everything = Query {
        events: true,
        ..Query::default()
    };
    let (mut found, mut seen) = (Vec::new(), HashSet::new());
    let wanted: HashSet<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| !id.is_empty())
        .take(MOST)
        .collect();
    for id in wanted {
        for statement in [&mut channels, &mut direct] {
            let mut rows = statement.query([id])?;
            while let Some(row) = rows.next()? {
                let (target, other): (String, Option<String>) = (row.get(4)?, row.get(5)?);
                let row = Row {
                    at: Cursor {
                        nanotime: row.get(0)?,
                        id: row.get(1)?,
                    },
                    msgid: row.get(2)?,
                    data: row.get(3)?,
                    conversation: match other {
                        None => Conversation::Channel(target),
                        Some(other) => Conversation::direct(&target, &other),
                    },
                };
                if viewer.may_see(&row.conversation)
                    && seen.insert(row.at.id)
                    && let Some(message) = read(&row, &everything)
                {
                    found.push((row.at, message));
                }
            }
        }
    }
    found.sort_by_key(|(at, _)| *at);
    Ok(found.into_iter().map(|(_, message)| message).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trim::testing::{add, add_direct, history};
    use zirc_proto::desired::Visibility;

    const SECOND: i64 = 1_000_000_000;

    /// alice and bob in #team, carol alone in #secret, and alice and bob's direct messages.
    fn sample() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        add(&path, "m1", "#team", 10 * SECOND, "alice", "morning");
        add(&path, "m2", "#team", 20 * SECOND, "bob", "morning, alice");
        add(&path, "m3", "#secret", 30 * SECOND, "carol", "the plan");
        add_direct(&path, "d1", 40 * SECOND, "alice", "bob", "lunch?");
        let zirc = zirc_proto::Message::new(
            "cc-1",
            zirc_proto::Event::Working {
                paths: vec!["a.rs".into()],
                continued: false,
                worktree: None,
                branch: None,
                part: None,
                base: None,
                committed: Vec::new(),
            },
        );
        add(
            &path,
            "m4",
            "#team",
            50 * SECOND,
            "alice-mbp",
            &zirc_proto::encode(&zirc).unwrap(),
        );
        (dir, path)
    }

    fn viewer(account: &str, channels: &[&str], policy: Visibility) -> Viewer {
        Viewer {
            account: account.into(),
            machines: [format!("{account}-mbp")].into(),
            channels: channels.iter().map(|c| (*c).to_owned()).collect(),
            policy,
        }
    }

    fn ids(path: &Path, viewer: &Viewer, asked: &[&str]) -> Vec<String> {
        let asked: Vec<String> = asked.iter().map(|id| (*id).to_owned()).collect();
        by_ids(path, viewer, &asked)
            .unwrap()
            .into_iter()
            .map(|m| m.msgid)
            .collect()
    }

    #[test]
    fn gives_what_the_viewer_may_read_oldest_first_events_too() {
        let (_dir, path) = sample();
        let bob = viewer("bob", &["#team"], Visibility::Participant);
        assert_eq!(
            ids(&path, &bob, &["m4", "m1", "d1", "m2"]),
            ["m1", "m2", "d1", "m4"]
        );
    }

    #[test]
    fn a_conversation_the_viewer_may_not_read_never_comes_back() {
        let (_dir, path) = sample();
        // carol isn't in alice and bob's direct messages, and under participant not in #team.
        let carol = viewer("carol", &["#secret"], Visibility::Participant);
        assert_eq!(ids(&path, &carol, &["m1", "d1", "m3"]), ["m3"]);
        // With every channel open to everyone, #team is hers to read; the direct message still isn't.
        let open = viewer("carol", &["#secret"], Visibility::Channels);
        assert_eq!(ids(&path, &open, &["m1", "d1", "m3"]), ["m1", "m3"]);
    }

    #[test]
    fn a_direct_message_comes_back_once_and_ids_it_doesnt_have_not_at_all() {
        let (_dir, path) = sample();
        let alice = viewer("alice", &["#team"], Visibility::Everything);
        assert_eq!(ids(&path, &alice, &["d1", "d1", "gone", ""]), ["d1"]);
    }
}
