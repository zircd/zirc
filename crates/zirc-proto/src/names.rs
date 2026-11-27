//! The names people and machines have on an organisation's IRC server, said once for
//! zirc.dev, the tenant controller, the gateway and zirc (`docs/accounts.md`). A person's
//! handle names their account and nick; a machine's account is its own name, such as
//! `m3-mac`, which says nothing of whose it is: its host does ([`machine_host`]), which zircd
//! relies on (zirc-core's `agents::may_query`).

/// The longest nick or account name, Ergo's `NICKLEN`.
pub const NICK_MAX: usize = 32;

/// The shortest handle.
pub const HANDLE_MIN: usize = 2;

/// The longest handle: room for `-<machine>` after it within [`NICK_MAX`].
pub const HANDLE_MAX: usize = 24;

/// Handles nobody may take, on any server: `zirc` founds teams' channels (and keeps `zirc-*`
/// machines free).
pub const RESERVED_HANDLES: [&str; 1] = ["zirc"];

/// zirc's own system account's nick: what tells owners a new device signed in. Every app
/// calls it `zirc`.
pub const SYSTEM: &str = "zirc-system";

/// Nicks of zirc's own integrations (the alert bridge's, CI's, the tenant's recorder's and pusher's), which
/// no team's integration may post as: the tenant sidecar refuses to post as them, so no
/// integration can pass for another.
pub const RESERVED_INTEGRATION_NICKS: [&str; 7] = [
    "zirc-alerts",
    "zirc-ci",
    "zirc-bridge",
    SYSTEM,
    "zirc-zirc",
    crate::activity::RECORDER,
    crate::push::PUSHER,
];

// A handle and `-<machine>` fit a nick.
const _: () = assert!(HANDLE_MAX + 1 < NICK_MAX);

/// Whether `target` names a channel rather than a nick: what IRC starts one with.
#[must_use]
pub fn is_channel(target: &str) -> bool {
    target.starts_with(['#', '&'])
}

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

/// Whether `name` is shaped as a handle is ([`handle_pattern`]).
fn handle_shaped(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && (HANDLE_MIN..=HANDLE_MAX).contains(&name.len())
}

/// What's wrong with `name` as a role's name ([`crate::roles`]), if anything. It's the
/// role's nick, among people's on the server, so it's shaped as a handle is
/// ([`handle_pattern`]) and is none of zirc's own names. That it's no member's handle, nor a
/// handle a role's name, the server's sidecar and zirc.dev check, as they know both.
#[must_use]
pub fn role_problem(name: &str) -> Option<String> {
    nick_problem("role", name)
}

/// What's wrong with `name` as a coworker's name ([`crate::coworkers`]), if anything: a
/// role's rules, as it's a nick as a role's is.
#[must_use]
pub fn coworker_problem(name: &str) -> Option<String> {
    nick_problem("coworker", name)
}

/// What's wrong with `name` as a job's name ([`crate::coworkers`]), if anything: a role's
/// rules, as a role is named after its job today.
#[must_use]
pub fn job_problem(name: &str) -> Option<String> {
    nick_problem("job", name)
}

/// The longest name a coworker is shown by ([`shown_name_problem`]), in characters.
pub const SHOWN_NAME_MAX: usize = 32;

/// What [`shown_name_problem`] says of no name at all, as zirc.dev's hiring form says it too.
pub const SHOWN_NAME_NONE: &str = "Name a coworker.";

/// What [`shown_name_problem`] says of a name over [`SHOWN_NAME_MAX`] characters, as zirc.dev's
/// hiring form says it too.
#[must_use]
pub fn shown_name_too_long() -> String {
    format!("A coworker's name is {SHOWN_NAME_MAX} characters at most.")
}

/// What's wrong with `shown` as the name a coworker is shown by, `John`, if anything: one
/// line of at most [`SHOWN_NAME_MAX`] characters that a nick can be made from
/// ([`nick_base`]).
#[must_use]
pub fn shown_name_problem(shown: &str) -> Option<String> {
    let shown = shown.trim();
    if shown.is_empty() {
        return Some(SHOWN_NAME_NONE.to_owned());
    }
    if shown.chars().count() > SHOWN_NAME_MAX {
        return Some(shown_name_too_long());
    }
    if shown.chars().any(char::is_control) {
        return Some("A coworker's name is one line.".to_owned());
    }
    nick_base(shown).is_none().then(|| {
        format!(
            "A coworker's name needs {HANDLE_MIN} letters from a to z or more: its handle in \
             chat is made from them."
        )
    })
}

/// The letters Latin-1's and Latin Extended-A's letters fold to, from U+00C0, one each: a
/// space for one that folds to none, or to two ([`fold`]).
const FOLDED: &str = "aaaaaa ceeeeiiiidnooooo ouuuuy  aaaaaa ceeeeiiiidnooooo ouuuuy y\
                      aaaaaaccccccccddddeeeeeeeeeegggggggghhhhiiiiiiiiii  jjkkklllllll\
                      lllnnnnnnnnnoooooo  rrrrrrssssssssttttttuuuuuuuuuuuuwwyyyzzzzzzs";

/// What `c` is written with in a nick, in lowercase a to z: `Zoë` and `Ærø` as `zoe` and
/// `aero`. None for a character that's no Latin letter.
fn fold(c: char) -> Option<&'static str> {
    const ASCII: &str = "abcdefghijklmnopqrstuvwxyz";
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        let at = usize::from(lower as u8 - b'a');
        return Some(&ASCII[at..=at]);
    }
    match c {
        'Æ' | 'æ' => Some("ae"),
        'Œ' | 'œ' => Some("oe"),
        'Þ' | 'þ' => Some("th"),
        'ß' => Some("ss"),
        'Ĳ' | 'ĳ' => Some("ij"),
        _ => {
            let at = usize::try_from(u32::from(c).checked_sub(0xC0)?).ok()?;
            FOLDED.get(at..=at).filter(|folded| *folded != " ")
        }
    }
}

/// The nick a coworker shown as `shown` is given when no other has it: lowercase, accents
/// folded (`Zoë` is `zoe`), spaces and hyphens as underscores (`Mary Jane` is `mary_jane`),
/// anything else left out (`O'Brien` is `obrien`), starting with a letter and at most
/// [`HANDLE_MAX`] long (the user's choice, 27 Nov 2026). None when that leaves fewer than
/// [`HANDLE_MIN`] characters.
#[must_use]
pub fn nick_base(shown: &str) -> Option<String> {
    let mut nick = String::new();
    for c in shown.trim().chars() {
        if c.is_whitespace() || c == '-' || c == '_' {
            if !nick.is_empty() && !nick.ends_with('_') {
                nick.push('_');
            }
        } else if c.is_ascii_digit() {
            if !nick.is_empty() {
                nick.push(c);
            }
        } else if let Some(folded) = fold(c) {
            nick.push_str(folded);
        }
    }
    nick.truncate(HANDLE_MAX);
    let nick = nick.trim_end_matches('_');
    (nick.len() >= HANDLE_MIN).then(|| nick.to_owned())
}

/// The nick a coworker shown as `shown` is hired with, where `taken` says which nicks are
/// someone's already in the office (a member's, a machine's, another coworker's): its
/// [`nick_base`], or that with the lowest number from 2 that's free after it (`john2`),
/// shortened to fit. zirc's own names count as taken.
///
/// # Errors
///
/// Why `shown` can't be a coworker's name ([`shown_name_problem`]).
pub fn nick_for(shown: &str, taken: impl Fn(&str) -> bool) -> Result<String, String> {
    if let Some(why) = shown_name_problem(shown) {
        return Err(why);
    }
    let base = nick_base(shown).unwrap_or_default();
    let free = |nick: &str| !taken(nick) && nick_problem("coworker", nick).is_none();
    if free(&base) {
        return Ok(base);
    }
    (2..=u32::from(u16::MAX))
        .map(|number| {
            let number = number.to_string();
            let stem = &base[..base.len().min(HANDLE_MAX - number.len())];
            format!("{}{number}", stem.trim_end_matches('_'))
        })
        .find(|nick| free(nick))
        .ok_or_else(|| format!("{base} is taken in this office: choose another name."))
}

/// What's wrong with `name` as the name of a `what`, which is or may be a nick on the server.
fn nick_problem(what: &str, name: &str) -> Option<String> {
    if !handle_shaped(name) {
        return Some(format!(
            "Name a {what} with {HANDLE_MIN} to {HANDLE_MAX} lowercase letters, digits and \
             underscores, starting with a letter."
        ));
    }
    (RESERVED_HANDLES.contains(&name) || RESERVED_INTEGRATION_NICKS.contains(&name))
        .then(|| format!("{name} is zirc's own: choose another name."))
}

/// What's wrong with `name` as a handle, as people are told, if anything.
#[must_use]
pub fn handle_problem(name: &str) -> Option<String> {
    if !handle_shaped(name) {
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
/// a letter, at most [`NICK_MAX`] letters, digits, `_` and `-`. A person's and a machine's
/// both are.
#[must_use]
pub fn is_account_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= NICK_MAX
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
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

/// What the host of a role's nick starts with, before the role's name: [`role_host`].
pub const ROLE_HOST_PREFIX: &str = "role/";

/// The host the server gives a role's nick ([`crate::roles`]), `role/fixer`, set by its
/// operator as a machine's is: what tells apps a member is a role, not a person.
#[must_use]
pub fn role_host(role: &str) -> String {
    format!("{ROLE_HOST_PREFIX}{}", role.to_lowercase())
}

/// The role whose nick has `host`, if it's a [`role_host`].
#[must_use]
pub fn role_from_host(host: &str) -> Option<&str> {
    host.strip_prefix(ROLE_HOST_PREFIX)
        .filter(|role| role_problem(role).is_none())
}

/// What the server says an account is, by the host its operator gave it: the one place a
/// host is read. An account with neither is a person's own or an integration's,
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
        format!("const SHOWN_NAME_MAX = {SHOWN_NAME_MAX};"),
        // The browser checks a name before the server does, and runs no wasm (lib/files.ts):
        // what it says of one is these.
        format!("const SHOWN_NAME_NONE = {SHOWN_NAME_NONE:?};"),
        format!("const SHOWN_NAME_TOO_LONG = {:?};", shown_name_too_long()),
        // A role's name is shaped as a handle is (role_problem).
        format!("const ROLE_PATTERN = {:?};", handle_pattern()),
        format!(
            "const RESERVED_HANDLES: readonly string[] = [{}];",
            reserved.join(", ")
        ),
        format!(
            "const RESERVED_INTEGRATION_NICKS: readonly string[] = [{}];",
            nicks.join(", ")
        ),
        format!(
            "const INTEGRATION_NICK_PATTERN = {:?};",
            integration_nick_pattern()
        ),
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
        assert_eq!(role_from_host(&role_host("fixer")), Some("fixer"));
        assert_eq!(role_from_host("role/Not A Role"), None);
        assert_eq!(owner_from_host(&role_host("fixer")), None);
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
            handle_problem("zirc").as_deref(),
            Some("zirc is zirc's own: choose another handle.")
        );
        assert_eq!(handle_pattern(), "^[a-z][a-z0-9_]{1,23}$");
    }

    #[test]
    fn an_account_name_is_a_nick_the_server_takes() {
        assert!(is_account_name("ada-mbp") && is_account_name("ada"));
        assert!(!is_account_name("Ada") && !is_account_name(&"a".repeat(NICK_MAX + 1)));
        // No handle can be a machine's: handles have no hyphen.
        assert!(!is_handle("ada-mbp"));
    }

    #[test]
    fn a_coworkers_nick_is_made_from_the_name_it_is_shown_by() {
        assert_eq!(FOLDED.len(), 0x180 - 0xC0);
        for (shown, nick) in [
            ("John", "john"),
            ("Mary Jane", "mary_jane"),
            ("Jean-Luc", "jean_luc"),
            ("  Zoë  ", "zoe"),
            ("Ærø Łukasz", "aero_lukasz"),
            ("Weiß", "weiss"),
            ("O'Brien", "obrien"),
            ("2Pac", "pac"),
            ("R2 D2", "r2_d2"),
            ("Ada 🤖", "ada"),
            ("Alexandra Montgomery-Smith", "alexandra_montgomery_smi"),
        ] {
            assert_eq!(nick_base(shown).as_deref(), Some(nick), "{shown}");
            assert_eq!(nick_for(shown, |_| false).as_deref(), Ok(nick), "{shown}");
            assert!(handle_shaped(nick), "{nick}");
        }
        // No letters a to z to make one from, or too few: refused, saying why.
        for shown in ["李明", "🤖", "Q", "42", ""] {
            assert!(nick_base(shown).is_none(), "{shown}");
            assert!(shown_name_problem(shown).is_some(), "{shown}");
            assert!(nick_for(shown, |_| false).is_err(), "{shown}");
        }
        assert!(shown_name_problem(&"a".repeat(SHOWN_NAME_MAX + 1)).is_some());
        assert!(shown_name_problem("Jo\nhn").is_some());
    }

    #[test]
    fn a_nick_someone_has_gets_the_lowest_free_number() {
        let taken = ["john", "john2"];
        assert_eq!(
            nick_for("John", |nick| taken.contains(&nick)).as_deref(),
            Ok("john3")
        );
        // zirc's own names are taken everywhere.
        assert_eq!(nick_for("Zirc", |_| false).as_deref(), Ok("zirc2"));
        // A long one is shortened to fit its number.
        let long = "a".repeat(HANDLE_MAX);
        let nick = nick_for(&long, |nick| nick == long).unwrap();
        assert_eq!(nick, format!("{}2", "a".repeat(HANDLE_MAX - 1)));
    }
}
