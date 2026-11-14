//! The line format: [`Line`] parses and writes IRC lines, IRCv3 message tags included.

use std::fmt;
use std::str::FromStr;

/// One IRC line, without its CRLF.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    /// IRCv3 message tags, in the order they came, with their values unescaped.
    pub tags: Vec<(String, Option<String>)>,
    /// Who sent it, as `nick!user@host` or a server name.
    pub source: Option<String>,
    /// The command or three-digit numeric.
    pub command: String,
    /// The parameters, the trailing one included.
    pub params: Vec<String>,
}

impl Line {
    /// A line with a command and parameters and nothing else.
    #[must_use]
    pub fn new<P: Into<String>>(command: &str, params: impl IntoIterator<Item = P>) -> Self {
        Self {
            command: command.to_owned(),
            params: params.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// The value of the tag `key`, if it's there and has one.
    #[must_use]
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_deref())
    }
}

/// Why a line couldn't be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The line has no command.
    #[error("line has no command")]
    NoCommand,
}

impl FromStr for Line {
    type Err = ParseError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let mut rest = line.trim_end_matches(['\r', '\n']);
        let mut parsed = Line::default();

        if let Some(tagged) = rest.strip_prefix('@') {
            let (tags, after) = tagged.split_once(' ').unwrap_or((tagged, ""));
            parsed.tags = tags
                .split(';')
                .filter(|tag| !tag.is_empty())
                .map(|tag| match tag.split_once('=') {
                    Some((key, value)) => (key.to_owned(), Some(unescape_tag(value))),
                    None => (tag.to_owned(), None),
                })
                .collect();
            rest = after.trim_start_matches(' ');
        }

        if let Some(sourced) = rest.strip_prefix(':') {
            let (source, after) = sourced.split_once(' ').unwrap_or((sourced, ""));
            parsed.source = Some(source.to_owned());
            rest = after.trim_start_matches(' ');
        }

        let (command, mut rest) = rest.split_once(' ').unwrap_or((rest, ""));
        if command.is_empty() {
            return Err(ParseError::NoCommand);
        }
        parsed.command = command.to_ascii_uppercase();

        loop {
            rest = rest.trim_start_matches(' ');
            if rest.is_empty() {
                break;
            }
            if let Some(trailing) = rest.strip_prefix(':') {
                parsed.params.push(trailing.to_owned());
                break;
            }
            let (param, after) = rest.split_once(' ').unwrap_or((rest, ""));
            parsed.params.push(param.to_owned());
            rest = after;
        }

        Ok(parsed)
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.tags.is_empty() {
            f.write_str("@")?;
            for (i, (key, value)) in self.tags.iter().enumerate() {
                if i > 0 {
                    f.write_str(";")?;
                }
                f.write_str(key)?;
                if let Some(value) = value {
                    write!(f, "={}", escape_tag(value))?;
                }
            }
            f.write_str(" ")?;
        }
        if let Some(source) = &self.source {
            write!(f, ":{source} ")?;
        }
        f.write_str(&self.command)?;
        let last = self.params.len().saturating_sub(1);
        for (i, param) in self.params.iter().enumerate() {
            let needs_colon = param.is_empty() || param.starts_with(':') || param.contains(' ');
            if i == last && needs_colon {
                write!(f, " :{param}")?;
            } else {
                write!(f, " {param}")?;
            }
        }
        Ok(())
    }
}

/// Tag value escapes, as IRCv3 message-tags defines them: (escaped, raw).
const TAG_ESCAPES: [(char, char); 5] = [
    (':', ';'),
    ('s', ' '),
    ('\\', '\\'),
    ('r', '\r'),
    ('n', '\n'),
];

fn escape_tag(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match TAG_ESCAPES.iter().find(|(_, raw)| *raw == c) {
            Some((code, _)) => {
                escaped.push('\\');
                escaped.push(*code);
            }
            None => escaped.push(c),
        }
    }
    escaped
}

fn unescape_tag(value: &str) -> String {
    let mut raw = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            raw.push(c);
            continue;
        }
        // An unknown escape drops the backslash; one at the very end is dropped too.
        if let Some(next) = chars.next() {
            raw.push(
                TAG_ESCAPES
                    .iter()
                    .find(|(code, _)| *code == next)
                    .map_or(next, |(_, raw)| *raw),
            );
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_line() {
        let line: Line =
            "@time=2026-10-30T12:00:00.000Z;+zirc/x=a\\sb :nick!u@h PRIVMSG #api :hello there\r\n"
                .parse()
                .unwrap();
        assert_eq!(line.tag("time"), Some("2026-10-30T12:00:00.000Z"));
        assert_eq!(line.tag("+zirc/x"), Some("a b"));
        assert_eq!(line.source.as_deref(), Some("nick!u@h"));
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.params, ["#api", "hello there"]);
    }

    #[test]
    fn parses_bare_commands_and_numerics() {
        assert_eq!("PING".parse::<Line>().unwrap(), Line::new("PING", [""; 0]));
        let welcome: Line = ":irc.zirc.dev 001 me :Welcome".parse().unwrap();
        assert_eq!(welcome.command, "001");
        assert_eq!(welcome.params, ["me", "Welcome"]);
    }

    #[test]
    fn rejects_lines_without_a_command() {
        assert_eq!("".parse::<Line>(), Err(ParseError::NoCommand));
        assert_eq!(":only.a.source".parse::<Line>(), Err(ParseError::NoCommand));
    }

    #[test]
    fn writes_what_it_parses() {
        for text in [
            "PRIVMSG #api :hello there",
            "JOIN #api",
            "@+typing=active;msgid=x\\:y TAGMSG #api",
            ":n!u@h PRIVMSG #api ::starts with a colon",
            "CAP REQ :",
        ] {
            assert_eq!(text.parse::<Line>().unwrap().to_string(), text);
        }
    }

    #[test]
    fn escapes_tag_values_both_ways() {
        let raw = "semi; space \\ cr\r lf\n";
        assert_eq!(unescape_tag(&escape_tag(raw)), raw);
        assert_eq!(unescape_tag("dangling\\"), "dangling");
        assert_eq!(unescape_tag("unknown\\x"), "unknownx");
    }
}
