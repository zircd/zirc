//! Signing in as the person through zirc.dev, as zirc's apps do, rather than as a machine
//! as `zirc login` does: the person confirms a code in the browser, zirc.dev hands over
//! their account, and the app keeps the account's passphrase in the system's secret store
//! ([`crate::secrets`]) and the rest, a [`SignIn`], in a file of its own. A server without
//! accounts yet gives only its gate password, which the [`SignIn`] keeps.
//!
//! Every app keeps the passphrase under the same [`secrets::key`], so signing in once on a
//! machine serves each of a person's apps there.

use std::path::Path;

use serde::{Deserialize, Serialize};
use zirc_proto::login::{Credentials, LoginKind, LoginRequest, LoginStarted, Server};

use crate::login::{self, Client};
use crate::secrets::{self, Secrets};

/// Why signing in, or connecting as the sign-in, didn't work.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The login with zirc.dev didn't work.
    #[error(transparent)]
    Login(#[from] login::Error),
    /// The passphrase couldn't be kept or forgotten.
    #[error("the account's passphrase")]
    Secrets(#[from] secrets::Error),
    /// The account's passphrase isn't in the secret store any more.
    #[error("the passphrase for {key} is gone: sign in again")]
    PassphraseGone {
        /// What it was kept under.
        key: String,
    },
}

/// Who the person signed in as, and where: everything but the passphrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignIn {
    /// The person's handle, their account's name when there is one.
    pub handle: String,
    /// The nick to use.
    pub nick: String,
    /// Whether the server gave an account, whose passphrase is in the secret store.
    pub account: bool,
    /// The zirc.dev it was signed in at.
    pub web: String,
    /// The organisation's server.
    pub server: Server,
    /// The IRC real name to connect with, the person's name as the organisation shows it,
    /// when zirc.dev gave one ([`zirc_proto::login::Credentials::realname`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realname: Option<String>,
    /// Whether the server has this device's key for the account ([`crate::device_key`]):
    /// then it signs in with the key, and the passphrase stays for a server that refuses it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub device_key: bool,
    /// Where files are shared, when it isn't `web`: a self-hosted organisation's own server
    /// ([`zirc_proto::login::Credentials::media`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
}

impl SignIn {
    /// The sign-in `credentials` from `web` give, with the account's passphrase put in
    /// `secrets`. Without an account or a nick, the handle is made from the user's name.
    ///
    /// # Errors
    ///
    /// When the passphrase can't be kept.
    pub fn from_credentials(
        credentials: &Credentials,
        web: &str,
        secrets: &dyn Secrets,
    ) -> Result<Self, Error> {
        let handle = credentials
            .account
            .as_ref()
            .map(|account| account.name.clone())
            .or_else(|| credentials.nick.clone())
            .unwrap_or_else(|| {
                crate::names::nick_from(&std::env::var("USER").unwrap_or_else(|_| "zirc".into()))
            });
        if let Some(account) = &credentials.account {
            secrets.set(
                &secrets::key(&account.name, &credentials.server.host),
                &account.password,
            )?;
        }
        if let Some(id) = &credentials.account_id {
            secrets.set(&crate::account_keys::account_id_key(web), id)?;
        }
        Ok(Self {
            nick: credentials.nick.clone().unwrap_or_else(|| handle.clone()),
            handle,
            account: credentials.account.is_some(),
            web: web.to_owned(),
            server: credentials.server.clone(),
            realname: credentials.realname.clone(),
            device_key: credentials.account.as_ref().is_some_and(|a| a.device_key),
            media: credentials.media.clone(),
        })
    }

    /// Where its organisation's files are shared, without a trailing slash: its own server
    /// for a self-hosted organisation ([`SignIn::media`]), or else the zirc.dev signed in at.
    #[must_use]
    pub fn files(&self) -> &str {
        self.media
            .as_deref()
            .unwrap_or(&self.web)
            .trim_end_matches('/')
    }

    /// The organisation it's for, named as every client names it
    /// ([`crate::settings::org_name`]).
    #[must_use]
    pub fn org(&self) -> String {
        crate::settings::org_name(&self.server)
    }

    /// What the passphrase is kept under in the secret store.
    #[must_use]
    pub fn key(&self) -> String {
        secrets::key(&self.handle, &self.server.host)
    }

    /// The connection it makes: with SASL as the account, with the passphrase from
    /// `secrets`, or with the gate password alone.
    ///
    /// # Errors
    ///
    /// When the account's passphrase is missing from `secrets`.
    pub fn connection(&self, secrets: &dyn Secrets) -> Result<zirc_irc::Config, Error> {
        let mut config = crate::settings::connection(&self.server, self.nick.clone());
        if let Some(realname) = &self.realname {
            config.realname.clone_from(realname);
        }
        if self.account {
            let key = self.key();
            let Some(password) = secrets.get(&key) else {
                return Err(Error::PassphraseGone { key });
            };
            config.sasl = Some(zirc_irc::Credentials {
                account: self.handle.clone(),
                password,
            });
        }
        Ok(config)
    }

    /// [`SignIn::connection`], signing in with the device's `key` where the server has it.
    ///
    /// # Errors
    ///
    /// As [`SignIn::connection`].
    pub fn connection_with(
        &self,
        secrets: &dyn Secrets,
        key: Option<std::sync::Arc<dyn crate::device_key::DeviceKey>>,
    ) -> Result<zirc_irc::Config, Error> {
        let mut config = self.connection(secrets)?;
        if let Some(key) = key {
            self.sign_in_with(&mut config, key);
        }
        Ok(config)
    }

    /// Has `config` sign in with the device's `key`, where the server has it for this
    /// sign-in's account; else leaves it as it is.
    pub fn sign_in_with(
        &self,
        config: &mut zirc_irc::Config,
        key: std::sync::Arc<dyn crate::device_key::DeviceKey>,
    ) {
        if self.account && self.device_key {
            config.bearer = Some(crate::device_key::bearer(
                key,
                self.handle.clone(),
                self.server.host.clone(),
            ));
        }
    }

    /// This sign-in after the person changed their handle to `handle` at zirc.dev
    /// ([`login::Client::set_handle`]): the same passphrase, kept under the new account's
    /// name, and the nick follows when it was the handle.
    ///
    /// # Errors
    ///
    /// When the passphrase is gone or the secret store refuses.
    pub fn renamed(&self, handle: &str, secrets: &dyn Secrets) -> Result<Self, Error> {
        let mut next = self.clone();
        handle.clone_into(&mut next.handle);
        if self.nick == self.handle {
            handle.clone_into(&mut next.nick);
        }
        if self.account {
            let Some(password) = secrets.get(&self.key()) else {
                return Err(Error::PassphraseGone { key: self.key() });
            };
            secrets.set(&next.key(), &password)?;
            secrets.delete(&self.key())?;
        }
        Ok(next)
    }

    /// This sign-in as zirc.dev has it now, asked after the server refused its login: when
    /// the person changed their handle elsewhere, it's [`SignIn::renamed`] to the new one
    /// ([`login::Client::person`]). `None` when nothing changed, or there's no account to ask
    /// about, and retrying is all there is.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses, or the passphrase can't be moved.
    pub async fn renew(&self, secrets: &dyn Secrets) -> Result<Option<Self>, Error> {
        if !self.account {
            return Ok(None);
        }
        let Some(password) = secrets.get(&self.key()) else {
            return Err(Error::PassphraseGone { key: self.key() });
        };
        let account = zirc_proto::login::Account {
            name: self.handle.clone(),
            password,
            device_key: false,
        };
        let now = login::Client::new(&self.web)?.person(&account).await?;
        match now.and_then(|credentials| credentials.account) {
            Some(now) if now.name != self.handle => Ok(Some(self.renamed(&now.name, secrets)?)),
            _ => Ok(None),
        }
    }

    /// Forgets the account's passphrase, as signing out does.
    ///
    /// # Errors
    ///
    /// When the secret store refuses.
    pub fn forget(&self, secrets: &dyn Secrets) -> Result<(), Error> {
        if self.account {
            secrets.delete(&self.key())?;
        }
        Ok(())
    }
}

/// The sign-ins an app keeps, one per organisation, in the order signed in to. One reads
/// and writes as the lone [`SignIn`] apps kept before several organisations, so their older
/// releases still read it; several are a list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignIns(Vec<SignIn>);

impl SignIns {
    /// Each sign-in, in the order signed in to.
    pub fn iter(&self) -> impl Iterator<Item = &SignIn> {
        self.0.iter()
    }

    /// How many organisations it signs in to.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it signs in to none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The sign-in for the organisation `org` ([`SignIn::org`]).
    #[must_use]
    pub fn get(&self, org: &str) -> Option<&SignIn> {
        self.0.iter().find(|it| it.org() == org)
    }

    /// The sign-ins `credentials` from `web` give, one per organisation they name
    /// ([`login::each_org`]), with each account's passphrase put in `secrets`.
    ///
    /// # Errors
    ///
    /// When the web app named an organisation zirc can't keep, or a passphrase can't be
    /// kept.
    pub fn from_credentials(
        credentials: &Credentials,
        web: &str,
        secrets: &dyn Secrets,
    ) -> Result<Self, Error> {
        let mut signins = Self::default();
        for org in login::each_org(credentials, web)? {
            signins.insert(SignIn::from_credentials(&org.credentials, web, secrets)?);
        }
        Ok(signins)
    }

    /// Adds `signin`, or replaces its organisation's where it was.
    pub fn insert(&mut self, signin: SignIn) {
        match self.0.iter_mut().find(|it| it.org() == signin.org()) {
            Some(it) => *it = signin,
            None => self.0.push(signin),
        }
    }

    /// Signs out of the organisation `org` alone, forgetting its passphrase in `secrets`;
    /// the others stay signed in. Answers whether it was signed in.
    ///
    /// # Errors
    ///
    /// When the secret store refuses, and then it stays signed in.
    pub fn remove(&mut self, org: &str, secrets: &dyn Secrets) -> Result<bool, Error> {
        let Some(at) = self.0.iter().position(|it| it.org() == org) else {
            return Ok(false);
        };
        self.0[at].forget(secrets)?;
        self.0.remove(at);
        Ok(true)
    }

    /// The sign-ins kept in the file at `path`, for an app that keeps them in a file of
    /// their own: none when there's no file.
    ///
    /// # Errors
    ///
    /// When the file is there but can't be read.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err),
        };
        let file: File = toml::from_str(&text)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        Ok(match file {
            File::One(signin) => Self(vec![signin]),
            File::Several { signin } => Self(signin),
        })
    }

    /// Keeps them in the file at `path`, readable by this user only, since a sign-in can
    /// hold a gate password: one as the lone sign-in it was, several as `[[signin]]`.
    ///
    /// # Errors
    ///
    /// When the file can't be written.
    ///
    /// # Panics
    ///
    /// Never: every field serializes to TOML.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = match self.0.as_slice() {
            [one] => toml::to_string(one),
            several => toml::to_string(&File::Several {
                signin: several.to_vec(),
            }),
        }
        .expect("sign-ins serialize to TOML");
        crate::paths::write_private(path, &text)
    }
}

/// [`SignIns`] in a file of their own.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum File {
    One(SignIn),
    Several { signin: Vec<SignIn> },
}

/// One sign-in as itself, several as a list.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Shape {
    One(SignIn),
    Several(Vec<SignIn>),
}

impl Serialize for SignIns {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.as_slice() {
            [one] => one.serialize(serializer),
            several => several.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for SignIns {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Shape::deserialize(deserializer)? {
            Shape::One(signin) => Self(vec![signin]),
            Shape::Several(signins) => Self(signins),
        })
    }
}

impl From<SignIn> for SignIns {
    fn from(signin: SignIn) -> Self {
        Self(vec![signin])
    }
}

/// Signs in at `login`'s zirc.dev as the person, for the app `client` (such as
/// `zirc-tui 0.1.5`) on the machine `host` (usually [`crate::machine::host_name`]; a phone
/// says its own name): `started` is told the link and code to show, and the credentials come
/// back once the person confirms them in the browser. With `several`, zirc.dev lets the
/// person choose several organisations, for an app that keeps a chat for each. Turn them into
/// sign-ins with [`SignIns::from_credentials`], or for one organisation
/// [`SignIn::from_credentials`]. `device_key` is the public half of the device's own key
/// ([`crate::device_key::DeviceKey::public_key`]), for zirc.dev to register on the server.
///
/// # Errors
///
/// When zirc.dev can't be reached or doesn't take sign-ins, or the person turns it down or
/// lets it expire.
pub async fn sign_in(
    login: &Client,
    client: &str,
    host: &str,
    several: bool,
    device_key: Option<String>,
    started: impl FnOnce(&LoginStarted),
) -> Result<Credentials, Error> {
    let request = LoginRequest {
        client: client.to_owned(),
        host: host.to_owned(),
        kind: Some(LoginKind::Person),
        orgs: several.then_some(true),
        org: None,
        machine_id: None,
        device_key,
        connected: None,
    };
    let login_started = login.start(&request).await?;
    started(&login_started);
    Ok(login.wait(&login_started).await?)
}

/// As [`sign_in`], for an app signed in to the servers `connected` (their hosts, as
/// [`crate::account::Account::connected_hosts`] says) adding more: zirc.dev shows those as
/// "Connected on this device" and lets the person choose others, each of which comes back in
/// [`Credentials::orgs`] to merge in ([`SignIns::from_credentials`]); `org` is offered first,
/// as from an invite link.
///
/// # Errors
///
/// As [`sign_in`].
pub async fn sign_in_adding(
    login: &Client,
    client: &str,
    host: &str,
    connected: Vec<String>,
    org: Option<String>,
    device_key: Option<String>,
    started: impl FnOnce(&LoginStarted),
) -> Result<Credentials, Error> {
    let request = LoginRequest {
        client: client.to_owned(),
        host: host.to_owned(),
        kind: Some(LoginKind::Person),
        orgs: Some(true),
        org,
        machine_id: None,
        device_key,
        connected: Some(connected),
    };
    let login_started = login.start(&request).await?;
    started(&login_started);
    Ok(login.wait(&login_started).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::File;
    use zirc_proto::login::Account;

    fn credentials(account: bool, nick: Option<&str>) -> Credentials {
        Credentials {
            server: Server {
                host: "acme.irc.zirc.dev".into(),
                port: None,
                tls: None,
                password: Some("gate".into()),
            },
            account: account.then(|| Account {
                name: "mattias".into(),
                password: "the passphrase".into(),
                device_key: false,
            }),
            nick: nick.map(str::to_owned),
            owner: None,
            realname: None,
            account_id: None,
            organisation_id: None,
            media: None,
            web_session: None,
            orgs: Vec::new(),
        }
    }

    /// The organisation `name` at `name.irc.zirc.dev`, where the person is `handle`.
    fn org(name: &str, handle: &str) -> zirc_proto::login::OrgCredentials {
        zirc_proto::login::OrgCredentials {
            name: name.into(),
            server: Server {
                host: format!("{name}.irc.zirc.dev"),
                port: None,
                tls: None,
                password: None,
            },
            account: Some(Account {
                name: handle.into(),
                password: format!("{name}'s passphrase"),
                device_key: false,
            }),
            nick: Some(format!("{handle}-mbp")),
            owner: Some(handle.into()),
            realname: None,
            organisation_id: None,
            media: None,
        }
    }

    #[test]
    fn connects_with_the_name_zirc_dev_gave() {
        let home = tempfile::tempdir().unwrap();
        let secrets = crate::secrets::File(home.path().join("secrets.toml"));
        let named = Credentials {
            realname: Some("Mattias Granlund".into()),
            ..credentials(false, Some("mattias"))
        };
        let signin = SignIn::from_credentials(&named, "https://zirc.dev", &secrets).unwrap();
        assert_eq!(
            signin.connection(&secrets).unwrap().realname,
            "Mattias Granlund"
        );
        let plain = SignIn::from_credentials(
            &credentials(false, Some("mattias")),
            "https://zirc.dev",
            &secrets,
        )
        .unwrap();
        assert_eq!(plain.connection(&secrets).unwrap().realname, "mattias");
    }

    #[test]
    fn a_new_handle_keeps_the_passphrase_under_the_new_account() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let signin =
            SignIn::from_credentials(&credentials(true, None), "https://zirc.dev", &secrets)
                .unwrap();
        let old_key = signin.key();
        let renamed = signin.renamed("lovelace", &secrets).unwrap();
        assert_eq!(
            (renamed.handle.as_str(), renamed.nick.as_str()),
            ("lovelace", "lovelace")
        );
        assert_eq!(
            secrets.get(&renamed.key()).as_deref(),
            Some("the passphrase")
        );
        assert_eq!(secrets.get(&old_key), None);
        // It connects as the new account.
        let config = renamed.connection(&secrets).unwrap();
        assert_eq!(config.sasl.unwrap().account, "lovelace");
        // A nick of its own, a machine's, stays.
        let machine = SignIn {
            nick: "mattias-mbp".into(),
            ..signin
        };
        let secrets = File(dir.path().join("other.toml"));
        secrets.set(&machine.key(), "x").unwrap();
        assert_eq!(
            machine.renamed("lovelace", &secrets).unwrap().nick,
            "mattias-mbp"
        );
    }

    #[test]
    fn signs_in_to_each_organisation_a_login_names() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let mut several = credentials(true, None);
        several.orgs = vec![org("acme", "mattias"), org("globex", "matt")];
        let signins = SignIns::from_credentials(&several, "https://zirc.dev", &secrets).unwrap();
        let orgs: Vec<(String, &str)> = signins
            .iter()
            .map(|signin| (signin.org(), signin.nick.as_str()))
            .collect();
        assert_eq!(
            orgs,
            [
                ("acme".to_owned(), "mattias-mbp"),
                ("globex".to_owned(), "matt-mbp")
            ]
        );
        // Each passphrase kept under its own organisation's key.
        assert_eq!(
            secrets
                .get(&signins.get("globex").unwrap().key())
                .as_deref(),
            Some("globex's passphrase")
        );

        // A login that names none is its top level alone, as before.
        let one = SignIns::from_credentials(&credentials(true, None), "https://zirc.dev", &secrets)
            .unwrap();
        assert_eq!(one.iter().map(SignIn::org).collect::<Vec<_>>(), ["acme"]);
    }

    #[test]
    fn keeps_nothing_when_a_login_names_an_organisation_it_cant_keep() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let mut several = credentials(true, None);
        several.orgs = vec![org("acme", "mattias"), org("../etc", "matt")];
        assert!(matches!(
            SignIns::from_credentials(&several, "https://zirc.dev", &secrets),
            Err(Error::Login(login::Error::OrgName { .. }))
        ));
        assert_eq!(secrets.get("mattias@acme.irc.zirc.dev"), None);
    }

    #[test]
    fn keeps_the_passphrase_in_the_secret_store_alone() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let signin =
            SignIn::from_credentials(&credentials(true, None), "https://zirc.dev", &secrets)
                .unwrap();
        assert_eq!(
            (signin.handle.as_str(), signin.nick.as_str()),
            ("mattias", "mattias")
        );
        let text = toml::to_string(&signin).unwrap();
        assert!(!text.contains("the passphrase"), "{text}");

        let config = signin.connection(&secrets).unwrap();
        assert_eq!(
            (config.host.as_str(), config.port),
            ("acme.irc.zirc.dev", 6697)
        );
        let sasl = config.sasl.unwrap();
        assert_eq!(
            (sasl.account.as_str(), sasl.password.as_str()),
            ("mattias", "the passphrase")
        );

        signin.forget(&secrets).unwrap();
        assert!(matches!(
            signin.connection(&secrets),
            Err(Error::PassphraseGone { .. })
        ));
    }

    #[test]
    fn connects_with_the_gate_password_where_there_are_no_accounts_yet() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let signin = SignIn::from_credentials(
            &credentials(false, Some("mattias")),
            "https://zirc.dev",
            &secrets,
        )
        .unwrap();
        assert!(!signin.account);
        let config = signin.connection(&secrets).unwrap();
        assert_eq!(config.server_password.as_deref(), Some("gate"));
        assert!(config.sasl.is_none());
        assert_eq!(config.nick, "mattias");
    }

    #[test]
    fn makes_a_nick_when_zirc_dev_gives_none() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let signin =
            SignIn::from_credentials(&credentials(false, None), "https://zirc.dev", &secrets)
                .unwrap();
        assert!(crate::names::is_nick(&signin.nick), "{}", signin.nick);
        assert_eq!(signin.handle, signin.nick);
    }

    #[test]
    fn signs_in_through_the_browser() {
        let started = r#"{"id":"L1","code":"WDJB-MJHT","url":"https://zirc.test/l","interval":1,"expires_in":60}"#;
        let done = r#"{"status":"done","server":{"host":"acme.irc.zirc.dev"},"nick":"mattias"}"#;
        let (web, served) =
            zirc_test_ergo::web::serve(vec![(200, started.into()), (200, done.into())]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut shown = None;
        let credentials = runtime
            .block_on(async {
                let login = Client::new(&web)?;
                sign_in(&login, "zirc-test 1", "test-host", true, None, |s| {
                    shown = Some(s.code.clone());
                })
                .await
            })
            .unwrap();
        assert_eq!(shown.as_deref(), Some("WDJB-MJHT"));
        assert_eq!(credentials.nick.as_deref(), Some("mattias"));
        let requests = served.join().unwrap();
        assert!(
            requests[0].contains(r#""kind":"person""#)
                && requests[0].contains(r#""host":"test-host""#)
                && requests[0].contains(r#""orgs":true"#),
            "{}",
            requests[0]
        );
    }

    fn signin(host: &str) -> SignIn {
        SignIn::from_credentials(
            &Credentials {
                server: Server {
                    host: host.into(),
                    port: None,
                    tls: None,
                    password: Some("gate".into()),
                },
                ..credentials(false, Some("mattias"))
            },
            "https://zirc.dev",
            &File(std::path::PathBuf::from("/nonexistent")),
        )
        .unwrap()
    }

    #[test]
    fn one_sign_in_is_kept_as_it_was_and_several_as_a_list() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Settings {
            signin: SignIns,
        }
        let acme = signin("acme.irc.zirc.dev");
        assert_eq!(acme.org(), "acme");
        // In an app's own settings, as the desktop keeps them: one is the old table.
        let one = Settings {
            signin: acme.clone().into(),
        };
        let text = toml::to_string(&one).unwrap();
        assert!(text.starts_with("[signin]\n"), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), one);
        let mut several = one;
        several.signin.insert(signin("globex.irc.zirc.dev"));
        let text = toml::to_string(&several).unwrap();
        assert!(text.contains("[[signin]]"), "{text}");
        let read: Settings = toml::from_str(&text).unwrap();
        let orgs: Vec<String> = read.signin.iter().map(SignIn::org).collect();
        assert_eq!(orgs, ["acme", "globex"]);

        // In a file of their own, as the terminal client keeps them.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui.toml");
        assert!(SignIns::load(&path).unwrap().is_empty());
        SignIns::from(acme.clone()).save(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            toml::to_string(&acme).unwrap()
        );
        read.signin.save(&path).unwrap();
        assert_eq!(SignIns::load(&path).unwrap(), read.signin);
    }

    #[test]
    fn signing_out_of_one_organisation_leaves_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let mut signins = SignIns::default();
        signins.insert(signin("acme.irc.zirc.dev"));
        signins.insert(signin("globex.irc.zirc.dev"));
        // Signing in again replaces that organisation's, where it was.
        let mut again = signin("acme.irc.zirc.dev");
        again.nick = "matt".into();
        signins.insert(again);
        assert_eq!(signins.len(), 2);
        assert_eq!(signins.get("acme").unwrap().nick, "matt");
        assert!(signins.remove("acme", &secrets).unwrap());
        assert!(!signins.remove("acme", &secrets).unwrap());
        let orgs: Vec<String> = signins.iter().map(SignIn::org).collect();
        assert_eq!(orgs, ["globex"]);
    }
}
