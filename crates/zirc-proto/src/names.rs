//! The names people, machines and guests have on an organisation's IRC server, said once for
//! zirc.dev, the tenant controller, the gateway and zirc (`docs/accounts.md`). A person's
//! handle names their account and nick; a machine's account is its own name, such as
//! `m3-mac`, which says nothing of whose it is: its host does ([`machine_host`]), which zircd
//! relies on (zirc-core's `agents::may_query`); a guest's is [`crate::GUEST_PREFIX`] and an id.

/// The longest nick or account name, Ergo's `NICKLEN`.
pub const NICK_MAX: usize = 32;

/// The shortest handle.
pub const HANDLE_MIN: usize = 2;

/// The longest handle: room for `-<machine>` after it within [`NICK_MAX`].
pub const HANDLE_MAX: usize = 24;

/// Handles nobody may take, on any server: `zirc` founds teams' channels (and keeps `zirc-*`
/// machines free), `guest` and `guests` keep [`crate::GUEST_PREFIX`] for visitors.
pub const RESERVED_HANDLES: [&str; 3] = ["zirc", "guest", "guests"];

/// zirc's own system account's nick: what tells owners a guest is waiting or a new device
/// signed in, and notices a public channel. Every app calls it `zirc`.
pub const SYSTEM: &str = "zirc-system";

/// Names a guest may not show as, whatever the case: Zirc's own and the roles a team's people
/// hold, so no visitor passes for them beside the team. zirc.dev refuses them, and names
/// starting `zirc-` or [`crate::GUEST_PREFIX`] besides, when a guest names themselves.
pub const RESERVED_GUEST_NAMES: [&str; 9] = [
    "zirc",
    "guest",
    "guests",
    "system",
    "admin",
    "owner",
    "staff",
    "support",
    "moderator",
];

/// Nicks of zirc's own integrations (the alert bridge's, CI's, the tenant's recorder's), which
/// no team's incoming hook may post as: the web app refuses them when a hook is made, and the
/// tenant sidecar refuses to post as them, so neither can be the one to pass for the other.
pub const RESERVED_INTEGRATION_NICKS: [&str; 7] = [
    "zirc-alerts",
    "zirc-ci",
    "zirc-bridge",
    SYSTEM,
    "zirc-zirc",
    crate::activity::RECORDER,
    crate::workflow::HOST,
];

/// The fewest characters a guest's id has after [`crate::GUEST_PREFIX`].
pub const GUEST_ID_MIN: usize = 4;

/// The most.
pub const GUEST_ID_MAX: usize = 16;

/// How many zirc.dev makes, as digits: `guest-21042`. Guests made before were 8 hex digits,
/// which [`is_guest_account`] still takes.
pub const GUEST_ID_MADE: usize = 5;

// A handle and `-<machine>` fit a nick, and zirc.dev makes guest ids anyone takes.
const _: () = assert!(HANDLE_MAX + 1 < NICK_MAX);
const _: () = assert!(GUEST_ID_MIN <= GUEST_ID_MADE && GUEST_ID_MADE <= GUEST_ID_MAX);

/// A handle, as a regular expression for languages that check one that way: lower case,
/// starting with a letter, [`HANDLE_MIN`] to [`HANDLE_MAX`] letters, digits and `_`.
#[must_use]
pub fn handle_pattern() -> String {
    format!("^[a-z][a-z0-9_]{{{},{}}}$", HANDLE_MIN - 1, HANDLE_MAX - 1)
}

/// Whether `name` is a handle someone may have: [`handle_pattern`], and not one of
/// [`RESERVED_HANDLES`].
#[must_use]
pub fn is_handle(name: &str) -> bool {
    handle_problem(name).is_none()
}

/// What every app says of a change of nickname, the person's handle, `from` one `to`
/// another: zirc.dev renames the account, the person's other apps and machines follow by
/// themselves, and the old nick is free for anyone to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NicknameWords {
    /// The question's title.
    pub title: String,
    /// What a change does, said before it's made.
    pub warning: String,
    /// The button that changes it.
    pub change: String,
    /// The button that keeps the old one.
    pub keep: String,
    /// What to say once it's changed.
    pub changed: String,
}

/// The words for changing the nickname `from` one handle `to` another.
#[must_use]
pub fn nickname_words(from: &str, to: &str) -> NicknameWords {
    NicknameWords {
        title: "Change your nickname".to_owned(),
        warning: format!(
            "After this, @{from} is free for anyone to take. Your old messages stay under \
             @{from}, and your other apps and computers follow by themselves."
        ),
        change: format!("Change to @{to}"),
        keep: format!("Keep @{from}"),
        changed: format!(
            "Changed to @{to}. The old nick is free for anyone to take, and your other apps \
             and computers follow by themselves."
        ),
    }
}

/// What's wrong with `name` as a handle, as people are told, if anything.
#[must_use]
pub fn handle_problem(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let shaped = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && (HANDLE_MIN..=HANDLE_MAX).contains(&name.len());
    if !shaped {
        return Some(format!(
            "Use {HANDLE_MIN} to {HANDLE_MAX} lowercase letters, digits and underscores, \
             starting with a letter."
        ));
    }
    RESERVED_HANDLES
        .contains(&name)
        .then(|| format!("{name} is zirc's own: choose another handle."))
}

/// Whether `name` is an account name the server takes as a nick: lower case, starting with
/// a letter, at most [`NICK_MAX`] letters, digits, `_` and `-`. A person's, a machine's and
/// a guest's all are.
#[must_use]
pub fn is_account_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= NICK_MAX
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Whether `name` is a guest's account as zirc.dev names them: [`crate::GUEST_PREFIX`] and
/// [`GUEST_ID_MIN`] to [`GUEST_ID_MAX`] lowercase letters and digits.
#[must_use]
pub fn is_guest_account(name: &str) -> bool {
    name.strip_prefix(crate::GUEST_PREFIX).is_some_and(|id| {
        (GUEST_ID_MIN..=GUEST_ID_MAX).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// What a machine's host starts with, before its owner's handle: [`machine_host`].
pub const MACHINE_HOST_PREFIX: &str = "machine/";

/// What the host of a person's own client starts with, before their handle: [`client_host`].
pub const CLIENT_HOST_PREFIX: &str = "client/";

/// The host the server gives a machine's account, `machine/mattias` for one of `mattias`'s,
/// in place of its address: what tells everyone whose a machine is, whatever its name. Only
/// the server's operator sets it (Ergo's `HostServ`), so no machine can say it's another
/// person's; at most it turns its own off, and is then nobody's.
#[must_use]
pub fn machine_host(owner: &str) -> String {
    format!("{MACHINE_HOST_PREFIX}{}", owner.to_lowercase())
}

/// The host the server gives an account a person signs in to themself under another name
/// than their handle, `client/mattias` for `mattias-irc`, their own IRC client: the person
/// at a keyboard, as their handle's account is, where a machine is their agents. Set as a
/// machine's is, by the server's operator alone.
#[must_use]
pub fn client_host(owner: &str) -> String {
    format!("{CLIENT_HOST_PREFIX}{}", owner.to_lowercase())
}

/// What the server says an account is, by the host its operator gave it: the one place a
/// host is read. An account with neither is a person's own, a guest's or an integration's,
/// as its name says, which only zirc.dev registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hosted<'a> {
    /// One of this person's machines, whose agents share its account: [`machine_host`].
    Machine(&'a str),
    /// This person's own client under another name: [`client_host`].
    Client(&'a str),
}

impl<'a> Hosted<'a> {
    /// What `host` says, if it's a [`machine_host`] or a [`client_host`] naming a handle.
    #[must_use]
    pub fn of(host: &'a str) -> Option<Self> {
        let handle = |owner: &'a str| Some(owner).filter(|owner| handle_problem(owner).is_none());
        if let Some(owner) = host.strip_prefix(MACHINE_HOST_PREFIX) {
            handle(owner).map(Self::Machine)
        } else {
            let owner = host.strip_prefix(CLIENT_HOST_PREFIX)?;
            handle(owner).map(Self::Client)
        }
    }

    /// What the sender of a line with `source` (`nick!user@host`) is, by its host.
    #[must_use]
    pub fn of_source(source: &'a str) -> Option<Self> {
        let (_, host) = source.rsplit_once('@')?;
        Self::of(host)
    }
}

/// Whose machine an account with `host` is, if its host is a [`machine_host`].
#[must_use]
pub fn owner_from_host(host: &str) -> Option<&str> {
    match Hosted::of(host)? {
        Hosted::Machine(owner) => Some(owner),
        Hosted::Client(_) => None,
    }
}

/// The most characters an integration's name has after `zirc-`.
pub const INTEGRATION_NAME_MAX: usize = 25;

/// Whether `nick` is an integration's, as zirc.dev registers them: `zirc-` and 1 to
/// [`INTEGRATION_NAME_MAX`] lowercase letters, digits and hyphens, not starting with a
/// hyphen. No person or machine can hold one: `zirc` is a reserved handle. The controller's
/// sidecar calls this (and refuses [`RESERVED_INTEGRATION_NICKS`]); the web app gets it as
/// [`integration_nick_pattern`] in its generated TypeScript.
#[must_use]
pub fn is_integration_nick(nick: &str) -> bool {
    nick.strip_prefix("zirc-").is_some_and(|name| {
        (1..=INTEGRATION_NAME_MAX).contains(&name.len())
            && !name.starts_with('-')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

/// [`is_integration_nick`] as a regular expression, for the web app's generated file.
#[must_use]
pub fn integration_nick_pattern() -> String {
    format!("^zirc-[a-z0-9][a-z0-9-]{{0,{}}}$", INTEGRATION_NAME_MAX - 1)
}

/// The names' rules as TypeScript constants, for the web app's generated file.
#[must_use]
pub fn typescript() -> Vec<String> {
    let reserved: Vec<String> = RESERVED_HANDLES.iter().map(|h| format!("{h:?}")).collect();
    let guest_names: Vec<String> = RESERVED_GUEST_NAMES
        .iter()
        .map(|n| format!("{n:?}"))
        .collect();
    let nicks: Vec<String> = RESERVED_INTEGRATION_NICKS
        .iter()
        .map(|n| format!("{n:?}"))
        .collect();
    vec![
        format!("const NAME = {:?};", crate::NAME),
        format!("const NICK_MAX = {NICK_MAX};"),
        format!("const HANDLE_MIN = {HANDLE_MIN};"),
        format!("const HANDLE_MAX = {HANDLE_MAX};"),
        format!("const HANDLE_PATTERN = {:?};", handle_pattern()),
        format!(
            "const RESERVED_HANDLES: readonly string[] = [{}];",
            reserved.join(", ")
        ),
        format!(
            "const RESERVED_GUEST_NAMES: readonly string[] = [{}];",
            guest_names.join(", ")
        ),
        format!(
            "const RESERVED_INTEGRATION_NICKS: readonly string[] = [{}];",
            nicks.join(", ")
        ),
        format!(
            "const INTEGRATION_NICK_PATTERN = {:?};",
            integration_nick_pattern()
        ),
        format!("const GUEST_PREFIX = {:?};", crate::GUEST_PREFIX),
        format!("const GUEST_LABEL = {:?};", crate::GUEST_LABEL),
        format!(
            "const GUEST_LABEL_ID_CHARS = {};",
            crate::GUEST_LABEL_ID_CHARS
        ),
        format!("const GUEST_ID_MIN = {GUEST_ID_MIN};"),
        format!("const GUEST_ID_MAX = {GUEST_ID_MAX};"),
        format!("const GUEST_ID_MADE = {GUEST_ID_MADE};"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machines_host_names_its_owner_and_nothing_else_does() {
        assert_eq!(machine_host("Mattias"), "machine/mattias");
        assert_eq!(owner_from_host(&machine_host("mattias")), Some("mattias"));
        for host in [
            "mattias",
            "machine/",
            "machine/Mattias",
            "machine/a-b",
            "user/mattias",
        ] {
            assert_eq!(owner_from_host(host), None, "{host}");
        }
    }

    #[test]
    fn a_host_says_a_machine_from_a_persons_own_client() {
        assert_eq!(client_host("Mattias"), "client/mattias");
        assert_eq!(
            Hosted::of("machine/mattias"),
            Some(Hosted::Machine("mattias"))
        );
        assert_eq!(
            Hosted::of("client/mattias"),
            Some(Hosted::Client("mattias"))
        );
        assert_eq!(
            Hosted::of_source("mattias-irc!u@client/mattias"),
            Some(Hosted::Client("mattias"))
        );
        // A person's client is no machine, to those that ask whose machine it is.
        assert_eq!(owner_from_host("client/mattias"), None);
        for host in ["client/", "client/a-b", "person/mattias", "a1b2.cloak"] {
            assert_eq!(Hosted::of(host), None, "{host}");
        }
        assert_eq!(Hosted::of_source("mattias-irc"), None);
    }

    #[test]
    fn the_pattern_says_what_the_function_says() {
        let pattern = integration_nick_pattern();
        assert_eq!(pattern, "^zirc-[a-z0-9][a-z0-9-]{0,24}$");
        // The pattern's shape: a first character, then up to the maximum less one.
        for nick in ["zirc-a", "zirc-ci-2", "zirc-aaaaaaaaaaaaaaaaaaaaaaaaa"] {
            assert!(is_integration_nick(nick), "{nick}");
        }
        assert!(!is_integration_nick("zirc-aaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn an_integration_is_zirc_and_a_short_plain_name() {
        for ok in [
            "zirc-github",
            "zirc-ci",
            "zirc-a",
            "zirc-1-2",
            "zirc-abcdefghijklmnopqrstuvwxy",
        ] {
            assert!(is_integration_nick(ok), "{ok}");
        }
        for bad in [
            "zirc",
            "zirc-",
            "zirc--x",
            "zirc-GitHub",
            "zirc-a_b",
            "zirc-abcdefghijklmnopqrstuvwxyz",
            "ada",
            "ada-mbp",
        ] {
            assert!(!is_integration_nick(bad), "{bad}");
        }
    }

    #[test]
    fn a_handle_is_short_plain_and_no_ones_reserved() {
        for good in ["ada", "ab", "mattias_g", "a2345678901234567890abcd"] {
            assert!(is_handle(good), "{good}");
        }
        for bad in [
            "a",
            "Ada",
            "1ada",
            "ada-mbp",
            "a23456789012345678901234x",
            "",
        ] {
            assert!(!is_handle(bad), "{bad}");
        }
        assert_eq!(
            handle_problem("guest").as_deref(),
            Some("guest is zirc's own: choose another handle.")
        );
        assert_eq!(handle_pattern(), "^[a-z][a-z0-9_]{1,23}$");
    }

    #[test]
    fn accounts_and_guests_are_told_by_their_names() {
        assert!(is_account_name("ada-mbp") && is_account_name("guest-1a2b3c4d"));
        assert!(!is_account_name("Ada") && !is_account_name(&"a".repeat(NICK_MAX + 1)));
        assert!(is_guest_account("guest-1a2b3c4d") && is_guest_account("guest-7k2q"));
        assert!(!is_guest_account("guest-7k2") && !is_guest_account("guest-ABCD"));
        assert!(!is_guest_account(&format!(
            "guest-{}",
            "a".repeat(GUEST_ID_MAX + 1)
        )));
        // No handle can be a guest's or a machine's: they have no hyphen.
        assert!(!is_handle("guest-1a2b"));
    }
}
