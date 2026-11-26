//! zircd's settings, kept in `config.toml` in zirc's directory. `zirc init` writes them and
//! zircd reads them, both through these types.
//!
//! ```toml
//! nick = "mattias-mbp"        # optional: <user>-<host> by default
//!
//! [server]
//! host = "irc.zirc.dev"
//! port = 6697                 # optional, 6697 by default
//! tls = true                  # optional, true by default
//! password = "team-gate"      # optional: the server password
//!
//! telemetry = false           # optional: anonymous usage analytics, on unless false
//!
//! [account]                   # optional: log in with SASL
//! name = "mattias"
//! password = "…"
//! ```
//!
//! A machine signed in to several organisations lists the others after the first, each
//! with its own server and account:
//!
//! ```toml
//! [[org]]
//! name = "globex"             # optional: from its server's host by default
//! nick = "mattias-mbp"
//! owner = "mattias"
//! web = "https://zirc.dev"
//! [org.server]
//! host = "globex.irc.zirc.dev"
//! [org.account]
//! name = "mattias-mbp"
//! password = "…"
//! ```
//!
//! The first organisation stays where a machine signed in to one keeps it, so its config
//! is unchanged and zirc releases from before several organisations still read it.
//!

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The server and account types are the `zirc login` handshake's too, so they're declared
/// once, in zirc-proto.
pub use zirc_proto::login::{Account, Server};

/// The port used when none is set: IRC over TLS.
pub const DEFAULT_PORT: u16 = 6697;

/// The IRC connection to `server` as `nick`: on [`DEFAULT_PORT`] with TLS unless it says
/// otherwise, with its password if it has one. Every client connects through this, so the
/// defaults are decided once.
#[must_use]
pub fn connection(server: &Server, nick: impl Into<String>) -> zirc_irc::Config {
    let mut config = zirc_irc::Config::new(
        server.host.clone(),
        server.port.unwrap_or(DEFAULT_PORT),
        nick,
    );
    config.tls = server.tls.unwrap_or(true);
    config.server_password.clone_from(&server.password);
    config
}

/// The host suffix of the servers zirc.dev runs, one per organisation: `<org>.irc.zirc.dev`.
const ZIRC_DEV_SERVERS: &str = ".irc.zirc.dev";

/// zirc.dev's public server, where anyone can ask for help in `#support`. A machine can be
/// signed in to it beside its own organisations, as `public`, but never watches a
/// repository there: what's said about one would be public.
pub const PUBLIC_SERVER: &str = "irc.zirc.dev";

/// What zirc calls [`PUBLIC_SERVER`]: a name zirc.dev keeps for it, so no organisation of
/// its own has it.
pub const PUBLIC_ORG: &str = "public";

/// Settings zirc once wrote and reads no more, at the top and in each `[[org]]`: left out
/// before the rest is read, so a `config.toml` from before still reads, and gone from it once
/// it's next saved. `guest` was a guest's terminal's, which zirc no longer has (20 Nov 2026).
const RETIRED: [&str; 1] = ["guest"];

/// `table` without its [`RETIRED`] settings.
fn without_retired(mut table: toml::Table) -> toml::Table {
    for key in RETIRED {
        table.remove(key);
    }
    if let Some(toml::Value::Array(orgs)) = table.get_mut("org") {
        for org in orgs.iter_mut().filter_map(toml::Value::as_table_mut) {
            for key in RETIRED {
                org.remove(key);
            }
        }
    }
    table
}

/// One organisation this machine is signed in to: its server and the account there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Organisation {
    /// What zirc calls it: [`Organisation::name`] when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The nick to use on its server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    /// Its server.
    pub server: Server,
    /// The machine's account on its server, logged in to with SASL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    /// The person this machine belongs to, by their handle there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The web app the machine signed in to it at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<String>,
}

impl Organisation {
    /// Its name: as set, or else [`org_name`] of its server.
    #[must_use]
    pub fn name(&self) -> String {
        self.name.clone().unwrap_or_else(|| org_name(&self.server))
    }
}

/// Whether `name` can name an organisation: zirc keeps an organisation's things in a
/// directory named after it, so it mustn't be empty, start with a dot or hold a slash.
#[must_use]
pub fn valid_org_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}

/// What zirc calls the organisation whose server is `server`, unless told otherwise: `zirc`
/// for `zirc.irc.zirc.dev`, or the server's host. Every client names organisations by it,
/// so `config.toml`'s and the apps' sign-ins agree.
#[must_use]
pub fn org_name(server: &Server) -> String {
    let host = &server.host;
    if host == PUBLIC_SERVER {
        return PUBLIC_ORG.to_owned();
    }
    host.strip_suffix(ZIRC_DEV_SERVERS)
        .unwrap_or(host)
        .to_owned()
}

/// What a machine must have room for before its sessions ask for a lease, in `config.toml`
/// as `[leases.<name>]`: each limit left out isn't checked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRoom {
    /// The highest load average, over five minutes, at which a session may ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_load: Option<u32>,
    /// The fewest gigabytes free on [`LeaseRoom::disk`] at which a session may ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free_gb: Option<u64>,
    /// The disk `min_free_gb` is about, by a path on it: `/` unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<PathBuf>,
}

/// The limits of zircd's circuit breaker on wakes, in `config.toml` as `[wakes]`: each one
/// left out is its default, which zircd's breaker declares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WakeLimits {
    /// The most wakes of one session in the ten-minute window: 10 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_session: Option<usize>,
    /// The most wakes of all this machine's sessions in the window: 30 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<usize>,
}

/// Everything in `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The nick to use on the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    /// The server to connect to.
    pub server: Server,
    /// An account to log in to with SASL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    /// The person this machine belongs to, its IRC real name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The web app this machine logged in at, which it asks for new credentials when the
    /// server refuses its account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<String>,
    /// How many seconds the server may be silent before zircd checks it's still there with
    /// a `PING`; it then waits half as long for an answer. 60 unless set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keepalive: Option<u64>,
    /// Whether zircd installs new releases by itself, where zirc's installer put it: unless
    /// set to false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,
    /// How often at most, in seconds, zircd tells a channel what this machine is working
    /// on: five minutes unless set (tests set 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub announce_every: Option<u64>,
    /// How long, in seconds, zircd holds a yes to a request that can't be undone before it
    /// counts ([`zirc_proto::request::HOLD_SECONDS`] unless set; tests set less).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_hold: Option<u64>,
    /// Whether a request this machine's sessions ask with a recommendation is taken as
    /// recommended at once, when a yes can be undone: `accept_recommended = true`. The
    /// request is still posted, and its person's reaction still says otherwise whenever it
    /// comes; one that can't be undone always waits for them. Unless set, every request
    /// waits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_recommended: Option<bool>,
    /// Whether zirc sends anonymous usage analytics: unless set to false. See
    /// [`crate::telemetry`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<bool>,
    /// The `PostHog` project key analytics is sent with: [`crate::telemetry::DEFAULT_KEY`]
    /// unless set, and nothing is sent when it's set to `""`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posthog_key: Option<String>,
    /// Where analytics is sent: [`crate::telemetry::DEFAULT_HOST`] unless set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posthog_host: Option<String>,
    /// What this machine must have room for before its sessions ask for a lease, by the
    /// lease's name: `[leases.build]` with `max_load = 40` and `min_free_gb = 15` holds a
    /// session's request for `build` back while the machine is busier or fuller, and asks
    /// for it once it has room ([`LeaseRoom`]).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub leases: std::collections::BTreeMap<String, LeaseRoom>,
    /// The limits of the circuit breaker on wakes ([`WakeLimits`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wakes: Option<WakeLimits>,
    /// The organisations besides the first, whose server, account, nick, owner and web app
    /// are the ones above.
    #[serde(default, rename = "org", skip_serializing_if = "Vec::is_empty")]
    pub orgs: Vec<Organisation>,
}

impl Settings {
    /// The settings in the config.toml of zirc's directory `home`: none when it isn't there
    /// or doesn't read.
    #[must_use]
    pub fn of(home: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(crate::paths::config(home)).ok()?;
        Self::from_toml(&text).ok()
    }

    /// Settings read from the text of `config.toml`.
    ///
    /// # Errors
    ///
    /// When the text isn't valid TOML or has settings zirc doesn't know.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        without_retired(toml::from_str(text)?).try_into()
    }

    /// Settings read from the text of `config.toml` by a program that only reads it, an app:
    /// a setting at its top that this version doesn't know is left out and named, where
    /// [`Settings::from_toml`] refuses the file. The file is zircd's, and a newer zirc than
    /// the app writes settings the app has never heard of: `accept_recommended` kept
    /// zirc for Mac 0.5.3 from starting. Which settings are known is serde's to say, so no
    /// list of them is kept here.
    ///
    /// # Errors
    ///
    /// When the text isn't valid TOML, or a setting this version knows doesn't read.
    pub fn from_toml_skipping_unknown(text: &str) -> Result<(Self, Vec<String>), toml::de::Error> {
        let mut table = without_retired(toml::from_str(text)?);
        let mut skipped = Vec::new();
        loop {
            let err = match table.clone().try_into::<Self>() {
                Ok(settings) => return Ok((settings, skipped)),
                Err(err) => err,
            };
            let unknown = err
                .message()
                .strip_prefix("unknown field `")
                .and_then(|rest| rest.split_once('`'))
                .map(|(name, _)| name.to_owned());
            match unknown {
                Some(name) if table.remove(&name).is_some() => skipped.push(name),
                _ => return Err(err),
            }
        }
    }

    /// Every organisation this machine is signed in to, the first first.
    #[must_use]
    pub fn organisations(&self) -> Vec<Organisation> {
        let first = Organisation {
            name: None,
            nick: self.nick.clone(),
            server: self.server.clone(),
            account: self.account.clone(),
            owner: self.owner.clone(),
            web: self.web.clone(),
        };
        std::iter::once(first).chain(self.orgs.clone()).collect()
    }

    /// The organisation named `name`, if this machine is signed in to it.
    #[must_use]
    pub fn organisation(&self, name: &str) -> Option<Organisation> {
        self.organisations()
            .into_iter()
            .find(|org| org.name() == name)
    }

    /// The organisation a command acts on ([`crate::which_org::pick`]): the one `named`, else
    /// `repo`'s, else the machine's only own one.
    ///
    /// # Errors
    ///
    /// [`crate::which_org::Unpicked`] when nothing says which, or `named` isn't one of them.
    pub fn pick(
        &self,
        named: Option<&str>,
        repo: Option<&str>,
    ) -> Result<Organisation, crate::which_org::Unpicked> {
        let orgs = self.organisations();
        let listed: Vec<String> = orgs.iter().map(Organisation::name).collect();
        let choices: Vec<crate::which_org::Choice<'_>> = listed
            .iter()
            .map(|name| crate::which_org::Choice {
                name,
                own: name != PUBLIC_ORG,
            })
            .collect();
        let at = crate::which_org::pick(&choices, named, repo)?;
        Ok(orgs[at].clone())
    }

    /// Sets the organisation named `name`'s settings to `org`, adding it after the others
    /// if this machine wasn't signed in to it.
    pub fn set_organisation(&mut self, name: &str, org: Organisation) {
        if self.organisations()[0].name() == name {
            // The first keeps the name it has from its server: it has nowhere to say one.
            self.nick = org.nick;
            self.server = org.server;
            self.account = org.account;
            self.owner = org.owner;
            self.web = org.web;
        } else if let Some(it) = self.orgs.iter_mut().find(|it| it.name() == name) {
            *it = org;
        } else {
            self.orgs.push(org);
        }
    }

    /// Takes the organisation named `name` out, if it's one besides the first, which has
    /// nowhere else to go; answers whether it did.
    pub fn remove_organisation(&mut self, name: &str) -> bool {
        let before = self.orgs.len();
        self.orgs.retain(|org| org.name() != name);
        self.orgs.len() < before
    }

    /// Takes the organisation named `name` out, the first too: the next one then takes the
    /// first's place at the top, as the one repositories saved before several organisations
    /// belong to. Answers whether it did; the only organisation can't be taken out here (its
    /// file goes instead).
    pub fn take_organisation(&mut self, name: &str) -> bool {
        if self.remove_organisation(name) {
            return true;
        }
        if self.organisations()[0].name() != name || self.orgs.is_empty() {
            return false;
        }
        let next = self.orgs.remove(0);
        self.nick = next.nick;
        self.server = next.server;
        self.account = next.account;
        self.owner = next.owner;
        self.web = next.web;
        true
    }

    /// The first organisation whose name is another's too, which zirc couldn't tell apart.
    #[must_use]
    pub fn duplicate_organisation(&self) -> Option<String> {
        let mut seen = std::collections::BTreeSet::new();
        self.organisations()
            .into_iter()
            .map(|org| org.name())
            .find(|name| !seen.insert(name.clone()))
    }

    /// Writes them to `path`, readable by this user only, since they can hold passwords.
    ///
    /// # Errors
    ///
    /// When the file or its directory can't be written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::paths::write_private(path, &self.to_toml())
    }

    /// The text of `config.toml` for these settings.
    ///
    /// # Panics
    ///
    /// Never: every field serializes to TOML.
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("settings serialize to TOML")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_leaves_out_what_is_unset() {
        let settings = Settings {
            nick: None,
            server: Server {
                host: "irc.zirc.dev".into(),
                port: None,
                tls: Some(false),
                password: None,
            },
            account: None,
            owner: None,
            web: None,
            keepalive: None,
            auto_update: None,
            announce_every: None,
            request_hold: None,
            accept_recommended: None,
            telemetry: None,
            posthog_key: None,
            posthog_host: None,
            leases: std::collections::BTreeMap::new(),
            wakes: None,
            orgs: Vec::new(),
        };
        let text = settings.to_toml();
        assert_eq!(text, "[server]\nhost = \"irc.zirc.dev\"\ntls = false\n");
        assert_eq!(Settings::from_toml(&text).unwrap(), settings);
    }

    #[test]
    fn connects_with_tls_on_the_default_port_unless_told_otherwise() {
        let mut server = Server {
            host: "zirc.irc.zirc.dev".into(),
            port: None,
            tls: None,
            password: Some("gate".into()),
        };
        let config = connection(&server, "mattias");
        assert_eq!(
            (
                config.host.as_str(),
                config.port,
                config.tls,
                config.nick.as_str()
            ),
            ("zirc.irc.zirc.dev", DEFAULT_PORT, true, "mattias")
        );
        assert_eq!(config.server_password.as_deref(), Some("gate"));
        server.port = Some(6667);
        server.tls = Some(false);
        let config = connection(&server, "mattias");
        assert_eq!((config.port, config.tls), (6667, false));
    }

    #[test]
    fn one_organisation_is_the_config_as_it_was() {
        let text = "nick = \"mattias-mbp\"\n\n[server]\nhost = \"zirc.irc.zirc.dev\"\n";
        let settings = Settings::from_toml(text).unwrap();
        let orgs = settings.organisations();
        assert_eq!(orgs.len(), 1);
        assert_eq!(orgs[0].name(), "zirc");
        assert_eq!(orgs[0].nick.as_deref(), Some("mattias-mbp"));
        // Written back unchanged, so releases from before several organisations read it.
        assert_eq!(settings.to_toml(), text);
    }

    #[test]
    fn an_app_skips_the_settings_it_doesnt_know() {
        let text = "nick = \"m\"\naccept_recommended = true\nfrom_next_year = 3\n\
                    [server]\nhost = \"zirc.irc.zirc.dev\"\n[newer]\nthan = \"this\"\n";
        let (read, skipped) = Settings::from_toml_skipping_unknown(text).unwrap();
        assert_eq!(skipped, ["from_next_year", "newer"]);
        assert_eq!(read.accept_recommended, Some(true));
        assert_eq!(read.server.host, "zirc.irc.zirc.dev");
        // zircd and zirc still refuse them, as a mistyped setting is theirs to catch.
        assert!(Settings::from_toml(text).is_err());
        // What it knows must still read, and a known table's own settings too.
        for wrong in [
            "keepalive = \"soon\"\n[server]\nhost = \"x\"\n",
            "[server]\nhost = \"x\"\n[wakes]\nnewer = 1\n",
            "nick = \"m\"\n",
        ] {
            assert!(
                Settings::from_toml_skipping_unknown(wrong).is_err(),
                "{wrong}"
            );
        }
    }

    #[test]
    fn reads_and_writes_several_organisations() {
        let mut settings =
            Settings::from_toml("nick = \"m\"\n[server]\nhost = \"zirc.irc.zirc.dev\"\n").unwrap();
        let globex = Organisation {
            name: None,
            nick: Some("m".into()),
            server: Server {
                host: "globex.irc.zirc.dev".into(),
                port: None,
                tls: None,
                password: None,
            },
            account: Some(Account {
                name: "m".into(),
                password: "pw".into(),
                device_key: false,
            }),
            owner: Some("mattias".into()),
            web: None,
        };
        settings.set_organisation("globex", globex.clone());
        let text = settings.to_toml();
        assert!(text.contains("[[org]]"), "{text}");
        let read = Settings::from_toml(&text).unwrap();
        assert_eq!(read, settings);
        let names: Vec<String> = read
            .organisations()
            .iter()
            .map(Organisation::name)
            .collect();
        assert_eq!(names, ["zirc", "globex"]);
        assert_eq!(read.organisation("globex"), Some(globex));

        // Setting the first changes the settings at the top.
        let mut zirc = read.organisation("zirc").unwrap();
        zirc.nick = Some("n".into());
        settings.set_organisation("zirc", zirc);
        assert_eq!(settings.nick.as_deref(), Some("n"));
        assert_eq!(settings.orgs.len(), 1);
        assert_eq!(settings.duplicate_organisation(), None);
        settings.orgs.push(settings.orgs[0].clone());
        assert_eq!(settings.duplicate_organisation().as_deref(), Some("globex"));

        // Out again, as `zirc logout globex` takes it; the first stays.
        let mut out = read.clone();
        assert!(out.remove_organisation("globex"));
        assert!(!out.remove_organisation("globex"));
        assert!(!out.remove_organisation("zirc"), "the first stays");
        assert_eq!(out.organisations().len(), 1);
    }

    #[test]
    fn a_guests_terminal_from_before_is_left_out() {
        let text = "[server]\nhost = \"zirc.irc.zirc.dev\"\n[guest]\ngrant = \"g\"\n\
                    [[org]]\nname = \"globex\"\n[org.server]\nhost = \"globex.irc.zirc.dev\"\n\
                    [org.guest]\ngrant = \"g\"\n";
        let read = Settings::from_toml(text).unwrap();
        let names: Vec<String> = read
            .organisations()
            .iter()
            .map(Organisation::name)
            .collect();
        assert_eq!(names, ["zirc", "globex"]);
        assert!(!read.to_toml().contains("guest"), "{}", read.to_toml());
        assert!(
            Settings::from_toml_skipping_unknown(text)
                .unwrap()
                .1
                .is_empty()
        );
    }

    #[test]
    fn names_an_organisation_by_its_server() {
        let org = |host: &str, name: Option<&str>| Organisation {
            name: name.map(Into::into),
            nick: None,
            server: Server {
                host: host.into(),
                port: None,
                tls: None,
                password: None,
            },
            account: None,
            owner: None,
            web: None,
        };
        assert_eq!(org("zirc.irc.zirc.dev", None).name(), "zirc");
        assert_eq!(org("irc.example.com", None).name(), "irc.example.com");
        assert_eq!(org("irc.zirc.dev", None).name(), "public");
        assert_eq!(org("127.0.0.1", Some("beta")).name(), "beta");
    }

    #[test]
    fn refuses_names_that_arent_a_directory_of_their_own() {
        assert!(valid_org_name("zirc") && valid_org_name("127.0.0.1"));
        for name in ["", ".", "..", "../x", "a/b", "a\\b", ".hidden"] {
            assert!(!valid_org_name(name), "{name}");
        }
    }

    #[test]
    fn refuses_unknown_settings() {
        assert!(Settings::from_toml("[server]\nhost = \"x\"\nhots = 1\n").is_err());
    }
}
