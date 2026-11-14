//! IRCv3 `draft/multiline`: a message of several lines, or longer than an IRC line allows,
//! sent and received as one. <https://ircv3.net/specs/extensions/multiline>
//!
//! A multiline message is a `BATCH` of `PRIVMSG`s to one target. Each line of the text is a
//! `PRIVMSG`; a line too long for one is cut into pieces, and every piece after the first
//! carries `draft/multiline-concat`, so the receiver joins it to the one before with nothing
//! between. Receivers without the capability get the pieces as separate messages.

use std::collections::HashMap;

use crate::Line;

/// The capability.
pub const CAP: &str = "draft/multiline";
/// The tag that joins a line to the one before it without a line break.
const CONCAT: &str = "draft/multiline-concat";
/// The longest piece of text a `PRIVMSG` in a batch carries, leaving room on its line for the
/// prefix, command, target and tags.
const PIECE_BYTES: usize = 400;

/// What the server takes in one multiline message, as its `CAP LS` value says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes of text, counted without line breaks.
    pub max_bytes: usize,
    /// The most `PRIVMSG`s in the batch.
    pub max_lines: usize,
}

impl Limits {
    /// The limits in a `draft/multiline` value, such as `max-bytes=4096,max-lines=100`.
    /// `max-bytes` is required; without `max-lines`, lines are limited only by bytes.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let mut max_bytes = None;
        let mut max_lines = usize::MAX;
        for pair in value.split(',') {
            match pair.split_once('=') {
                Some(("max-bytes", n)) => max_bytes = n.parse().ok(),
                Some(("max-lines", n)) => max_lines = n.parse().ok()?,
                _ => {}
            }
        }
        Some(Self {
            max_bytes: max_bytes?,
            max_lines,
        })
    }
}

/// The lines that send `text` to `target` as one multiline message, in a batch named
/// `reference`, or `None` when it's more than `limits` allow.
#[must_use]
pub fn batch(reference: &str, target: &str, text: &str, limits: Limits) -> Option<Vec<Line>> {
    let mut lines = vec![Line::new("BATCH", [&format!("+{reference}"), CAP, target])];
    let mut bytes = 0;
    for line in text.split('\n') {
        let mut rest = line;
        let mut first = true;
        // An empty line still counts: it's a blank line in the message.
        while first || !rest.is_empty() {
            let cut = floor_char_boundary(rest, PIECE_BYTES);
            let (piece, after) = rest.split_at(cut);
            let mut privmsg = Line::new("PRIVMSG", [target, piece]);
            privmsg
                .tags
                .push(("batch".to_owned(), Some(reference.to_owned())));
            if !first {
                privmsg.tags.push((CONCAT.to_owned(), None));
            }
            lines.push(privmsg);
            bytes += piece.len();
            rest = after;
            first = false;
        }
    }
    let pieces = lines.len() - 1;
    if bytes > limits.max_bytes || pieces > limits.max_lines {
        return None;
    }
    lines.push(Line::new("BATCH", [format!("-{reference}")]));
    Some(lines)
}

/// The largest index no greater than `max` that's on a character boundary of `text`.
fn floor_char_boundary(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }
    (0..=max)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0)
}

/// Joins received multiline messages back into single `PRIVMSG`s. Feed it every line the
/// server sends: it holds a multiline batch's lines until the batch ends, then gives back
/// one `PRIVMSG` with the whole text, its source, and the tags of the batch's opening line;
/// every other line passes straight through.
#[derive(Debug, Default)]
pub struct Batches {
    open: HashMap<String, Pending>,
}

#[derive(Debug)]
struct Pending {
    opening: Line,
    target: String,
    source: Option<String>,
    text: String,
    empty: bool,
}

impl Batches {
    /// What the server's `line` amounts to: itself, nothing yet (it's part of a multiline
    /// batch still open), or the whole message once its batch ends.
    pub fn feed(&mut self, line: Line) -> Option<Line> {
        let param = |i: usize| line.params.get(i).map(String::as_str);
        if line.command == "BATCH" {
            match (param(0), param(1), param(2)) {
                (Some(start), Some(CAP), Some(target)) if start.starts_with('+') => {
                    let pending = Pending {
                        opening: line.clone(),
                        target: target.to_owned(),
                        source: None,
                        text: String::new(),
                        empty: true,
                    };
                    self.open.insert(start[1..].to_owned(), pending);
                    return None;
                }
                (Some(end), _, _) if end.starts_with('-') => {
                    if let Some(pending) = self.open.remove(&end[1..]) {
                        return Some(pending.into_line());
                    }
                }
                _ => {}
            }
            return Some(line);
        }
        let Some(pending) = line.tag("batch").and_then(|batch| self.open.get_mut(batch)) else {
            return Some(line);
        };
        if line.command == "PRIVMSG" {
            let piece = param(1).unwrap_or_default();
            if !pending.empty && !has_tag(&line, CONCAT) {
                pending.text.push('\n');
            }
            pending.text.push_str(piece);
            pending.empty = false;
            pending.source = pending.source.take().or_else(|| line.source.clone());
        }
        None
    }
}

impl Pending {
    fn into_line(self) -> Line {
        let mut line = Line::new("PRIVMSG", [self.target, self.text]);
        line.source = self.source.or(self.opening.source);
        line.tags = self.opening.tags;
        line
    }
}

/// Whether `line` has `key`, valued or not.
fn has_tag(line: &Line, key: &str) -> bool {
    line.tags.iter().any(|(k, _)| k == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        max_bytes: 4096,
        max_lines: 100,
    };

    fn line(text: &str) -> Line {
        text.parse().unwrap()
    }

    #[test]
    fn reads_the_servers_limits() {
        assert_eq!(Limits::parse("max-bytes=4096,max-lines=100"), Some(LIMITS));
        assert_eq!(
            Limits::parse("max-bytes=40").map(|limits| limits.max_lines),
            Some(usize::MAX)
        );
        assert_eq!(Limits::parse("max-lines=10"), None);
    }

    #[test]
    fn cuts_long_lines_into_pieces_that_join_again() {
        let long = "é".repeat(300); // 600 bytes, two-byte characters
        let text = format!("first\n{long}\n\nlast");
        let lines = batch("ml1", "#api", &text, LIMITS).unwrap();
        let shown: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(shown[0], "BATCH +ml1 draft/multiline #api");
        assert_eq!(shown[1], "@batch=ml1 PRIVMSG #api first");
        assert!(shown[3].starts_with("@batch=ml1;draft/multiline-concat PRIVMSG #api "));
        assert_eq!(shown.last().unwrap(), "BATCH -ml1");
        for line in &lines[1..lines.len() - 1] {
            assert!(line.params[1].len() <= PIECE_BYTES);
        }

        // What a receiver makes of them, with a server's prefix on each.
        let mut batches = Batches::default();
        let mut out = Vec::new();
        for line in lines {
            let mut line = line;
            line.source = Some("alpha!a@host".to_owned());
            out.extend(batches.feed(line));
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].command, "PRIVMSG");
        assert_eq!(out[0].params, ["#api", text.as_str()]);
        assert_eq!(out[0].source.as_deref(), Some("alpha!a@host"));
    }

    #[test]
    fn refuses_more_than_the_server_takes() {
        let small = Limits {
            max_bytes: 10,
            max_lines: 100,
        };
        assert!(batch("ml1", "#api", "eleven bytes", small).is_none());
        let few = Limits {
            max_bytes: 4096,
            max_lines: 2,
        };
        assert!(batch("ml1", "#api", "a\nb\nc", few).is_none());
    }

    #[test]
    fn passes_other_lines_and_batches_through() {
        let mut batches = Batches::default();
        let plain = line(":alpha!a@h PRIVMSG #api :hi");
        assert_eq!(batches.feed(plain.clone()), Some(plain));
        // A chathistory batch isn't multiline: its lines are the caller's.
        let history = line("BATCH +h1 chathistory #api");
        assert_eq!(batches.feed(history.clone()), Some(history));
        let inside = line("@batch=h1 :alpha!a@h PRIVMSG #api :old");
        assert_eq!(batches.feed(inside.clone()), Some(inside));
        // A multiline batch inside it comes out whole, keeping the outer batch's tag.
        assert_eq!(
            batches.feed(line("@batch=h1 BATCH +m1 draft/multiline #api")),
            None
        );
        assert_eq!(
            batches.feed(line("@batch=m1 :alpha!a@h PRIVMSG #api :one")),
            None
        );
        assert_eq!(
            batches.feed(line("@batch=m1 :alpha!a@h PRIVMSG #api :two")),
            None
        );
        let whole = batches.feed(line("BATCH -m1")).unwrap();
        assert_eq!(whole.params, ["#api", "one\ntwo"]);
        assert_eq!(whole.tag("batch"), Some("h1"));
    }
}
