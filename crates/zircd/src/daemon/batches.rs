//! The server's history batches, whoever asked for them. History is never news: a line in a
//! `chathistory` batch is the read's or the replay's that asked for it, or nobody's, and never
//! told to watchers as if just said.
//!
//! On 10 Nov 2026 a session's every new watch woke to the same three CI lines, hours after
//! each had been dealt with: reads of a channel's history (`zirc who` and `zirc sessions`
//! read every channel's, from many sessions at once) that waited past their time left their
//! batches to nobody, and each line in them went on as one heard live. One heard long enough
//! ago to have left [`super::Heard`] was news again, and named the session.

use std::collections::BTreeSet;

use zirc_irc::Line;

/// The batch types the server sends history in: `CHATHISTORY`'s answers, and the
/// conversations `CHATHISTORY TARGETS` lists.
const HISTORY: [&str; 2] = ["chathistory", "draft/chathistory-targets"];

/// The history batches open now, by the server's name for each.
#[derive(Debug, Default)]
pub(super) struct Batches(BTreeSet<String>);

impl Batches {
    /// Notes `line`'s part in the history batches. Whether it's history: a history batch
    /// starting or ending, or a line in one.
    pub(super) fn history(&mut self, line: &Line) -> bool {
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        if line.command == "BATCH" {
            if let Some(reference) = param(0).strip_prefix('+') {
                if HISTORY.contains(&param(1)) {
                    self.0.insert(reference.to_owned());
                    return true;
                }
                return false;
            }
            if let Some(reference) = param(0).strip_prefix('-') {
                return self.0.remove(reference);
            }
        }
        line.tag("batch")
            .is_some_and(|batch| self.0.contains(batch))
    }

    /// The connection dropped: no batch it opened will end.
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(reference: &str, kind: &[&str]) -> Line {
        let mut params = vec![reference];
        params.extend(kind);
        Line::new("BATCH", params)
    }

    fn privmsg(batch: Option<&str>) -> Line {
        let mut line = Line::new(
            "PRIVMSG",
            ["#api", "cc-1: dry-safety conflicts with master"],
        );
        if let Some(batch) = batch {
            line.tags = vec![("batch".to_owned(), Some(batch.to_owned()))];
        }
        line
    }

    #[test]
    fn a_line_in_a_history_batch_is_history_until_it_ends() {
        let mut batches = Batches::default();
        assert!(batches.history(&batch("+h1", &["chathistory", "#api"])));
        assert!(batches.history(&privmsg(Some("h1"))));
        assert!(batches.history(&batch("-h1", &[])));
        assert!(
            !batches.history(&privmsg(Some("h1"))),
            "a name the server may give again"
        );
        assert!(!batches.history(&privmsg(None)), "a line heard live");
    }

    #[test]
    fn other_batches_are_live() {
        let mut batches = Batches::default();
        assert!(!batches.history(&batch("+m1", &["draft/multiline", "#api"])));
        assert!(!batches.history(&privmsg(Some("m1"))));
        assert!(!batches.history(&batch("-m1", &[])));
        assert!(batches.history(&batch("+t1", &["draft/chathistory-targets"])));
    }

    #[test]
    fn a_dropped_connection_ends_every_batch() {
        let mut batches = Batches::default();
        batches.history(&batch("+h1", &["chathistory", "#api"]));
        batches.clear();
        assert!(!batches.history(&privmsg(Some("h1"))));
    }
}
