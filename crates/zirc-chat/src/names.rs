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
    word(text, name, |_, _| true)
}

/// Whether `text` names the person `nick`, as [`mentions`] says, but not as part of a path
/// or an address: `/Users/mattias/c` and `mattias@zirc.dev` aren't said to them, and
/// pushed to their phone as if they were (four disk warnings and two commit subjects, 16-19
/// Nov 2026). `@mattias` is.
#[must_use]
pub fn names_person(text: &str, nick: &str) -> bool {
    word(text, nick, |before, after| {
        before != Some('/') && !matches!(after, Some('/' | '@'))
    })
}

/// Whether `name` is in `text` as a word of its own, without case, where `apart` also
/// holds for the characters either side of it.
fn word(text: &str, name: &str, apart: impl Fn(Option<char>, Option<char>) -> bool) -> bool {
    if name.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let name = name.to_lowercase();
    text.match_indices(&name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(nick_char) && !after.is_some_and(nick_char) && apart(before, after)
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

/// The longest channel name zirc makes. Ergo's default limit is 64 bytes.
pub const MAX_CHANNEL_BYTES: usize = 64;

/// `#` and `name`, cut to fit a channel's name ([`MAX_CHANNEL_BYTES`]).
#[must_use]
pub fn channel(name: &str) -> String {
    let mut channel = format!("#{name}");
    if channel.len() > MAX_CHANNEL_BYTES {
        let mut end = MAX_CHANNEL_BYTES;
        while !channel.is_char_boundary(end) {
            end -= 1;
        }
        channel.truncate(end);
    }
    channel
}

/// What zirc keys what's said about the repository named `repo` by, `#api` for `api`: how
/// zircd, hints, shared states and the server's activity store have named a repository since
/// it had a channel of that name. It's no channel now.
#[must_use]
pub fn repo_scope(repo: &str) -> String {
    channel(&channel_part(repo))
}

/// What a line said in `channel` about `repo` ([`zirc_proto::Message::repo`]) counts as said
/// in: in a room every repository shares ([`zirc_proto::AGENTS_CHANNEL`],
/// [`zirc_proto::gotchas::CHANNEL`]), its repository's scope ([`repo_scope`]); else, as in a
/// topic, its channel.
#[must_use]
pub fn scope_of(channel: &str, repo: Option<&str>) -> String {
    let shared = [zirc_proto::AGENTS_CHANNEL, zirc_proto::gotchas::CHANNEL]
        .iter()
        .any(|room| room.eq_ignore_ascii_case(channel));
    match repo {
        Some(repo) if shared => repo_scope(repo),
        _ => channel.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_in_a_shared_room_counts_as_its_repositorys() {
        assert_eq!(repo_scope("API"), "#api");
        assert_eq!(scope_of("#agents", Some("api")), "#api");
        assert_eq!(scope_of("#Gotchas", Some("api")), "#api");
        assert_eq!(scope_of("#agents", None), "#agents");
        assert_eq!(scope_of("#auth", Some("api")), "#auth");
        assert_eq!(repo_scope(&"x".repeat(100)).len(), MAX_CHANNEL_BYTES);
    }

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

    #[test]
    fn names_a_person_but_not_in_a_path_or_an_address() {
        assert!(names_person("@mattias, over to you", "mattias"));
        assert!(names_person("mattias: May I run it?", "mattias"));
        assert!(!names_person(
            "18.2 GB /Users/mattias/c/monorepo-worktrees",
            "mattias"
        ));
        assert!(!names_person("mattias/cc-1 landed it", "mattias"));
        assert!(names_person(
            "from .mailmap, mattias@zirc.dev for Mattias",
            "mattias"
        ));
        assert!(!names_person("committed as mattias@zirc.dev", "mattias"));
    }
}
