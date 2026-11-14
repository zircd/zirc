//! What nicks zirc makes and accepts, and when text mentions one: rules every client
//! shares, so a machine's daemon, the terminal client and the desktop app agree.

/// The longest nick zirc makes. Ergo's default limit is 32 bytes.
pub const MAX_NICK_BYTES: usize = 30;

/// A nick made from `name`, such as a user name or `<user>-<host>`: lowercase letters,
/// digits and `-` only, starting with a letter, at most [`MAX_NICK_BYTES`].
#[must_use]
pub fn nick_from(name: &str) -> String {
    let mut nick: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(MAX_NICK_BYTES)
        .collect();
    if !nick.starts_with(|c: char| c.is_ascii_alphabetic()) {
        nick.insert(0, 'z');
        nick.truncate(MAX_NICK_BYTES);
    }
    nick
}

/// Whether `nick` is one zirc accepts as given: letters, digits, `-` and `_`, starting
/// with a letter, at most [`MAX_NICK_BYTES`].
#[must_use]
pub fn is_nick(nick: &str) -> bool {
    nick.len() <= MAX_NICK_BYTES
        && nick.starts_with(|c: char| c.is_ascii_alphabetic())
        && nick
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `text` mentions `name`, a nick or a session's name, as a word of its own and
/// without case: `mattias-mbp` in "ask mattias-mbp?", but not `cc-1` in `cc-12`. What IRC
/// allows in nicks counts as part of a word.
#[must_use]
pub fn mentions(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let name = name.to_lowercase();
    text.match_indices(&name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(nick_char) && !after.is_some_and(nick_char)
    })
}

/// Whether `c` can be part of a nick, as IRC allows: what [`mentions`] counts as part of a
/// word, and what an `@` mention being typed is made of.
#[must_use]
pub fn nick_char(c: char) -> bool {
    c.is_alphanumeric() || "-_[]\\`^{}|".contains(c)
}

/// `name` as part of a channel's name, as branches, topics and threads are named:
/// lowercase, with only what channel names and URLs agree on, letters, digits, `.`, `-`,
/// `_` and `/`, and anything else a `-`. Without `/` at either end.
#[must_use]
pub fn channel_part(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '.' | '-' | '_' | '/' => c,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('/')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_nicks_irc_accepts() {
        assert_eq!(nick_from("mattias-MBP.local"), "mattias-mbp-local");
        assert_eq!(nick_from("1st"), "z1st");
        assert_eq!(nick_from(&"a".repeat(40)).len(), MAX_NICK_BYTES);
        assert!(is_nick(&nick_from("Åsa Öberg")));
    }

    #[test]
    fn accepts_only_nicks_it_could_make_or_take() {
        assert!(is_nick("mattias_2"));
        assert!(!is_nick("2mattias"));
        assert!(!is_nick("mattias mbp"));
        assert!(!is_nick(&"a".repeat(MAX_NICK_BYTES + 1)));
    }

    #[test]
    fn finds_mentions_as_words() {
        assert!(mentions("ask Mattias-MBP?", "mattias-mbp"));
        assert!(mentions("mattias: hi", "mattias"));
        assert!(mentions("hey Mattias, look", "mattias"));
        assert!(!mentions("mattias-mbp committed", "mattias"));
        assert!(!mentions("bmattias", "mattias"));
        assert!(!mentions("cc-12 is done", "cc-1"));
        assert!(!mentions("mattias_ is away", "mattias"));
        assert!(!mentions("anything", ""));
    }
}
