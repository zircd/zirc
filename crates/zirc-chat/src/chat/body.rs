//! What a PRIVMSG body means: a zirc message, an action, a CTCP request, or words. Text
//! comes out without IRC's formatting codes and other control characters, so no client has
//! to clean it up before showing it.

use zirc_proto::Message;

/// A PRIVMSG body, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Words.
    Text(String),
    /// An action, as `/me` sends.
    Action(String),
    /// A zirc message, boxed: it's much larger than the rest.
    Zirc(Box<Message>),
    /// A zirc message that can't be read, from a newer zirc or broken: the human text it
    /// carries after its payload, never the payload.
    Unreadable(String),
    /// A CTCP request or reply, such as `VERSION`, which isn't for reading.
    Ctcp,
}

const CTCP: char = '\x01';

/// Reads a PRIVMSG body.
#[must_use]
pub fn read(body: &str) -> Body {
    match zirc_proto::decode(body) {
        Ok(Some(message)) => return Body::Zirc(Box::new(message)),
        Ok(None) => {}
        Err(_) => {
            let human = body
                .split_once(" :")
                .map_or("", |(_, human)| human)
                .trim_end_matches(CTCP);
            return Body::Unreadable(strip_formatting(human));
        }
    }
    if let Some(action) = body.strip_prefix("\x01ACTION ") {
        return Body::Action(strip_formatting(action.trim_end_matches(CTCP)));
    }
    if body.starts_with(CTCP) {
        return Body::Ctcp;
    }
    Body::Text(strip_formatting(body))
}

/// `text` without IRC's bold, colour and similar codes, and without other control
/// characters that would upset a terminal or a text view. Line breaks stay: a multiline
/// message has them.
#[must_use]
pub fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Colour: up to two digits, then optionally a comma and up to two more.
            '\x03' => {
                let coloured = skip(&mut chars, 2, char::is_ascii_digit);
                let mut ahead = chars.clone();
                if coloured
                    && ahead.next() == Some(',')
                    && ahead.peek().is_some_and(char::is_ascii_digit)
                {
                    chars.next();
                    skip(&mut chars, 2, char::is_ascii_digit);
                }
            }
            // Hex colour: six hex digits, then optionally a comma and six more.
            '\x04' => {
                let coloured = skip(&mut chars, 6, char::is_ascii_hexdigit);
                let mut ahead = chars.clone();
                if coloured
                    && ahead.next() == Some(',')
                    && ahead.peek().is_some_and(char::is_ascii_hexdigit)
                {
                    chars.next();
                    skip(&mut chars, 6, char::is_ascii_hexdigit);
                }
            }
            '\t' => out.push(' '),
            '\n' => out.push('\n'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The IRC formatting codes [`keep_formatting`] keeps: bold, colour, hex colour,
/// monospace, reverse, italic, strike, underline and reset.
const FORMATTING: &[char] = &[
    '\x02', '\x03', '\x04', '\x0f', '\x11', '\x16', '\x1d', '\x1e', '\x1f',
];

/// `text` with IRC's formatting codes kept, for [`super::rich`] to draw, and every other
/// control character dropped as [`strip_formatting`] drops them.
#[must_use]
pub fn keep_formatting(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            '\n' => Some('\n'),
            c if FORMATTING.contains(&c) => Some(c),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// `text` with its formatting codes, as an entry keeps it beside its plain text, when it
/// has any: `None` when it's the same as [`strip_formatting`] makes it.
#[must_use]
pub fn formatted(text: &str) -> Option<String> {
    text.contains(FORMATTING).then(|| keep_formatting(text))
}

/// Skips up to `most` characters that are `wanted`, saying whether there were any.
fn skip(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    most: usize,
    wanted: fn(&char) -> bool,
) -> bool {
    (0..most)
        .take_while(|_| chars.next_if(wanted).is_some())
        .count()
        > 0
}

/// `text` cut into pieces of at most `max` bytes, at spaces where it can, for messages
/// longer than an IRC line takes.
#[must_use]
pub fn split_for_line(text: &str, max: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.len() > max {
        let mut cut = max;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let at_space = rest.as_bytes()[cut] == b' ';
        if let Some(space) = rest[..cut].rfind(' ').filter(|&at| at > 0 && !at_space) {
            cut = space;
        }
        pieces.push(rest[..cut].to_owned());
        rest = rest[cut..].strip_prefix(' ').unwrap_or(&rest[cut..]);
    }
    if !rest.is_empty() || pieces.is_empty() {
        pieces.push(rest.to_owned());
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_proto::Event;

    #[test]
    fn reads_every_kind_of_body() {
        let commit = Message::new(
            "cc-1",
            Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: "abc".into(),
                subject: "Fix".into(),
                repo: None,
            },
        );
        let encoded = zirc_proto::encode(&commit).unwrap();
        assert_eq!(read(&encoded), Body::Zirc(Box::new(commit)));
        assert_eq!(read("\x02hello\x02"), Body::Text("hello".into()));
        assert_eq!(read("\x01ACTION waves\x01"), Body::Action("waves".into()));
        assert_eq!(read("\x01VERSION\x01"), Body::Ctcp);
    }

    #[test]
    fn shows_an_unreadable_messages_words_not_its_payload() {
        assert_eq!(
            read("\x01ACTION ZIRC bm90IGpzb24= :committed on main: Fix\x01"),
            Body::Unreadable("committed on main: Fix".into())
        );
    }

    #[test]
    fn strips_formatting() {
        assert_eq!(strip_formatting("\x0304,12red\x03 plain"), "red plain");
        assert_eq!(strip_formatting("\x04ff0000red\x0f"), "red");
        assert_eq!(strip_formatting("a\tb\x07\r\nc"), "a b\nc");
    }

    #[test]
    fn splits_long_text_at_spaces() {
        assert_eq!(split_for_line("aaa bbb ccc", 7), ["aaa bbb", "ccc"]);
        assert_eq!(split_for_line("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(split_for_line("åäö", 3), ["å", "ä", "ö"]);
        assert_eq!(split_for_line("", 3), [""]);
    }
}
