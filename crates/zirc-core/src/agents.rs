//! Whose word reaches a machine's agents, decided here once ([`standing`]): only the
//! machine's owner steers its agents, from their own account or their own client, and one of
//! their machines never speaks for them. And who may query them: ask them something with
//! `zirc ask`, which their `zirc watch` delivers ([`zirc_proto::Event::Query`]). Other
//! messages between agents stay undelivered.
//!
//! Kept in `agents.toml` beside `config.toml` ([`crate::paths::agents`]), set with
//! `zirc agents queries <off|mine|anyone>` or an app's settings, through zircd.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zirc_proto::names::Hosted;

/// What an app or the CLI says before letting anyone query this machine's agents.
pub const ANYONE_WARNING: &str = "Anyone in any organisation this machine is signed in to, \
    and any of their agents, could then ask this machine's agents to do things. A question can \
    carry instructions (prompt injection): your agents treat queries as requests, never as \
    yours, but they may still be talked into acting on one. Choose \"mine\" unless you trust \
    everyone in those organisations.";

/// Who may query this machine's agents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Queries {
    /// No one: queries are dropped.
    Off,
    /// The owner's own: this machine's other sessions, the owner, and the owner's other
    /// machines, told by the account the server says sent the query.
    #[default]
    Mine,
    /// Anyone signed in to an account on the server. [`ANYONE_WARNING`] says why it's risky.
    Anyone,
}

impl Queries {
    /// As written in `agents.toml` and on the command line.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Mine => "mine",
            Self::Anyone => "anyone",
        }
    }

    /// The setting `word` names: `off`, `mine` or `anyone`.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "off" => Some(Self::Off),
            "mine" => Some(Self::Mine),
            "anyone" => Some(Self::Anyone),
            _ => None,
        }
    }

    /// Every choice, in the order an app's Settings offers them.
    pub const ALL: [Self; 3] = [Self::Off, Self::Mine, Self::Anyone];

    /// The choice as an app's Settings offers it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "No one may query this machine's agents",
            Self::Mine => "Only me: my machines and their sessions",
            Self::Anyone => "Anyone on the server",
        }
    }
}

/// An app's Settings page for the setting, headed so.
pub const TITLE: &str = "Who may query this machine's agents";

/// What an app says while it asks zircd for the setting.
pub const ASKING: &str = "Asking zircd…";

/// What an app says when zircd doesn't answer, as the setting is zircd's to keep.
pub const UNANSWERED: &str = "zircd isn't answering on this machine; its agents' queries are \
    its to allow. Start it with zirc daemon start.";

/// The buttons under [`ANYONE_WARNING`], before an app lets anyone query: allow, or keep.
pub const ALLOW_ANYONE: &str = "Allow anyone";
/// See [`ALLOW_ANYONE`].
pub const KEEP: &str = "Keep it as it is";

/// What every zirc app and the CLI say of the setting: `queries to this machine's agents:
/// mine`, with the warning when it's `anyone`.
#[must_use]
pub fn said(queries: Queries) -> String {
    let said = format!("queries to this machine's agents: {}", queries.word());
    match queries {
        Queries::Anyone => format!("{said}\nwarning: {ANYONE_WARNING}"),
        Queries::Off | Queries::Mine => said,
    }
}

/// Everything in `agents.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agents {
    /// Who may query this machine's agents.
    #[serde(default)]
    pub queries: Queries,
}

impl Agents {
    /// The settings in `path`, or the defaults when there's no such file or it can't be
    /// read: no one but the owner's own may query.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes them to `path`.
    ///
    /// # Errors
    ///
    /// When the file or its directory can't be written.
    ///
    /// # Panics
    ///
    /// Never: the settings serialize to TOML.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string(self).expect("agents settings serialize to TOML");
        crate::paths::write_private(path, &text)
    }
}

/// Whether `web`, the web app a machine logged in at, is zirc.dev itself (or one of its own
/// hosts, such as staging), whose servers' accounts only zirc.dev makes: not any web app, as
/// one run elsewhere may let people register accounts as they like.
#[must_use]
pub fn is_zirc_dev(web: &str) -> bool {
    let Some(rest) = web.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    // No port, no userinfo: zirc.dev serves neither.
    host == "zirc.dev"
        || host
            .strip_suffix(".zirc.dev")
            .is_some_and(|sub| !sub.is_empty() && !sub.contains(['@', ':']))
}

/// Who sent something, as the server says and nothing else does: the account it's logged in
/// to (`account-tag`) and what its host says that account is ([`Hosted`]). A nick proves
/// nothing, and neither does a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sender<'a> {
    account: Option<&'a str>,
    hosted: Option<Hosted<'a>>,
}

impl<'a> Sender<'a> {
    /// The sender logged in as `account`, whose host says it's `hosted`.
    #[must_use]
    pub fn new(account: Option<&'a str>, hosted: Option<Hosted<'a>>) -> Self {
        Self {
            account: account.filter(|account| !account.is_empty() && *account != "*"),
            hosted,
        }
    }

    /// The sender of a line with the `account` tag and `source` (`nick!user@host`).
    #[must_use]
    pub fn of(account: Option<&'a str>, source: Option<&'a str>) -> Self {
        Self::new(account, source.and_then(Hosted::of_source))
    }

    /// The sender of `delivery`, as zircd read it off the line.
    #[must_use]
    pub fn of_delivery(delivery: &'a crate::ipc::Delivery) -> Self {
        let machine = delivery.hosted_by.as_deref().map(Hosted::Machine);
        let client = delivery.client_of.as_deref().map(Hosted::Client);
        Self::new(delivery.account.as_deref(), machine.or(client))
    }

    /// The account it's logged in to, if any.
    #[must_use]
    pub fn account(&self) -> Option<&'a str> {
        self.account
    }
}

/// Who a sender is to a person: the one rule for whose word reaches that person's agents,
/// which queries, direct messages, decisions and `zirc-remote` all ask ([`standing`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// The person themself: their own account, or their own client under another name. Only
    /// this is their word; what it says to their agents is an instruction.
    Owner,
    /// One of the person's machines, whose agents share its account. It has the person's
    /// access and none of their authority: what it says is a teammate's request, however
    /// it's written, as any agent that got at one machine could otherwise steer the others.
    Machine,
    /// Someone else signed in to an account on the server, or one of their machines.
    Member,
    /// No account, or a guest's: agents neither hear guests nor write to them.
    Nobody,
}

/// Who `sender` is to `person`, on a server whose accounts only zirc.dev makes when
/// `named_by_zirc_dev`: only there does a host surely say whose an account is, as only
/// zirc.dev's operator sets hosts there. Elsewhere, as on a self-hosted server, a host says
/// nothing, and only the person's own account is theirs.
///
/// That rests on zirc.dev's invariant, kept in the monorepo's `docs/accounts.md`: on servers
/// zirc.dev provisions only zirc.dev registers accounts (Ergo's registration is closed) and
/// only its sidecar, as Ergo's operator, sets hosts. If either ever changes, so must this.
#[must_use]
pub fn standing(sender: Sender<'_>, person: Option<&str>, named_by_zirc_dev: bool) -> Standing {
    let Some(account) = sender.account else {
        return Standing::Nobody;
    };
    if zirc_proto::named_as_guest(account) {
        return Standing::Nobody;
    }
    let Some(person) = person else {
        return Standing::Member;
    };
    if account.eq_ignore_ascii_case(person) {
        return Standing::Owner;
    }
    match sender.hosted {
        Some(Hosted::Client(of)) if named_by_zirc_dev && of.eq_ignore_ascii_case(person) => {
            Standing::Owner
        }
        Some(Hosted::Machine(of)) if named_by_zirc_dev && of.eq_ignore_ascii_case(person) => {
            Standing::Machine
        }
        _ => Standing::Member,
    }
}

/// Whether a query from a sender of `standing` to this machine's owner may reach its agents,
/// under `queries`.
#[must_use]
pub fn may_query(queries: Queries, standing: Standing) -> bool {
    match queries {
        Queries::Off => false,
        Queries::Mine => matches!(standing, Standing::Owner | Standing::Machine),
        Queries::Anyone => standing != Standing::Nobody,
    }
}

/// Whose machine the sender of a line with `source` (`nick!user@host`) is, as the server says
/// by its host (`zirc_proto::names::machine_host`): None for a person, their own client, or a
/// machine whose host doesn't say.
#[must_use]
pub fn host_owner(source: Option<&str>) -> Option<&str> {
    match Hosted::of_source(source?)? {
        Hosted::Machine(owner) => Some(owner),
        Hosted::Client(_) => None,
    }
}

/// Whose own client the sender of a line with `source` is, as its host says
/// (`zirc_proto::names::client_host`).
#[must_use]
pub fn client_owner(source: Option<&str>) -> Option<&str> {
    match Hosted::of_source(source?)? {
        Hosted::Client(owner) => Some(owner),
        Hosted::Machine(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Who the account `account`, with `host` if the server gave it one, is to mattias.
    fn to_mattias(account: Option<&str>, host: Option<&str>, zirc_dev: bool) -> Standing {
        let source = host.map(|host| format!("nick!u@{host}"));
        standing(
            Sender::of(account, source.as_deref()),
            Some("mattias"),
            zirc_dev,
        )
    }

    #[test]
    fn only_a_persons_own_account_or_client_is_their_word() {
        assert_eq!(to_mattias(Some("mattias"), None, true), Standing::Owner);
        assert_eq!(to_mattias(Some("Mattias"), None, true), Standing::Owner);
        // Their IRC client, as its host says; its name says nothing.
        let client = Some("client/mattias");
        assert_eq!(
            to_mattias(Some("mattias-irc"), client, true),
            Standing::Owner
        );
        assert_eq!(
            to_mattias(Some("mattias-irc"), None, true),
            Standing::Member
        );
        assert_eq!(
            to_mattias(Some("mattias-irc"), Some("client/bob"), true),
            Standing::Member
        );
        // Only on zirc.dev's servers, where only zirc.dev's operator sets hosts.
        assert_eq!(
            to_mattias(Some("mattias-irc"), client, false),
            Standing::Member
        );
        assert_eq!(to_mattias(Some("mattias"), None, false), Standing::Owner);
    }

    #[test]
    fn a_persons_machine_is_theirs_but_never_their_word() {
        let host = Some("machine/mattias");
        assert_eq!(
            host_owner(Some("m3-mac!u@machine/mattias")),
            Some("mattias")
        );
        assert_eq!(to_mattias(Some("m3-mac"), host, true), Standing::Machine);
        // Someone else's, one whose host says nothing, or whose name alone looks like ours.
        assert_eq!(
            to_mattias(Some("m3-mac"), Some("machine/bob"), true),
            Standing::Member
        );
        assert_eq!(
            to_mattias(Some("m3-mac"), Some("a1b2.cloak"), true),
            Standing::Member
        );
        assert_eq!(
            to_mattias(Some("mattias-mbp"), None, true),
            Standing::Member
        );
        assert_eq!(to_mattias(Some("m3-mac"), host, false), Standing::Member);
        assert_eq!(host_owner(Some("m3-mac")), None);
        // A client is no machine, and a machine no client.
        assert_eq!(host_owner(Some("mattias-irc!u@client/mattias")), None);
        assert_eq!(
            client_owner(Some("mattias-irc!u@client/mattias")),
            Some("mattias")
        );
        assert_eq!(client_owner(Some("m3-mac!u@machine/mattias")), None);
    }

    #[test]
    fn no_account_or_a_guests_is_nobody() {
        // No account: the nick says nothing.
        assert_eq!(to_mattias(None, None, true), Standing::Nobody);
        assert_eq!(to_mattias(Some("*"), None, true), Standing::Nobody);
        assert_eq!(
            to_mattias(Some(""), Some("machine/mattias"), true),
            Standing::Nobody
        );
        assert_eq!(
            to_mattias(Some("guest-a1b2c3"), None, true),
            Standing::Nobody
        );
        assert_eq!(to_mattias(Some("bob"), None, true), Standing::Member);
        // With no owner known, no one is the owner or theirs.
        let sender = Sender::of(Some("mattias"), None);
        assert_eq!(standing(sender, None, true), Standing::Member);
    }

    #[test]
    fn trusts_account_names_only_from_zirc_dev_itself() {
        assert!(is_zirc_dev("https://zirc.dev"));
        assert!(is_zirc_dev("https://zirc.dev/"));
        assert!(is_zirc_dev("https://staging.zirc.dev"));
        assert!(!is_zirc_dev("http://zirc.dev"));
        assert!(!is_zirc_dev("https://zirc.example.com"));
        assert!(!is_zirc_dev("https://evilzirc.dev"));
        assert!(!is_zirc_dev("https://zirc.dev.example.com"));
        assert!(!is_zirc_dev("https://zirc.dev@example.com"));
        assert!(!is_zirc_dev("https://zirc.dev:8443"));
        assert!(!is_zirc_dev("https://localhost:8787"));
    }

    #[test]
    fn queries_reach_agents_as_the_setting_says() {
        let all = [
            Standing::Owner,
            Standing::Machine,
            Standing::Member,
            Standing::Nobody,
        ];
        let let_in = |queries| all.map(|standing| may_query(queries, standing));
        assert_eq!(let_in(Queries::Off), [false, false, false, false]);
        assert_eq!(let_in(Queries::Mine), [true, true, false, false]);
        assert_eq!(let_in(Queries::Anyone), [true, true, true, false]);
    }

    #[test]
    fn keeps_the_setting_in_its_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.toml");
        assert_eq!(Agents::load(&path).queries, Queries::Mine);
        Agents {
            queries: Queries::Anyone,
        }
        .save(&path)
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "queries = \"anyone\"\n"
        );
        assert_eq!(Agents::load(&path).queries, Queries::Anyone);
        assert_eq!(Queries::from_word("off"), Some(Queries::Off));
        assert_eq!(Queries::from_word("all"), None);
    }
}
