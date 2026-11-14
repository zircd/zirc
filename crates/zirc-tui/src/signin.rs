//! Signing in as the person through zirc.dev, as zirc's other apps do (see
//! [`zirc_core::signin`]), with the prompts on the terminal: the passphrase goes in the
//! system's secret store and the rest, a [`SignIn`] for each organisation signed in to, in
//! `tui.toml` in zirc's directory.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use zirc_core::account_keys::Leaving;
use zirc_core::device_key::DeviceKey;
use zirc_core::login::{self, Client};
use zirc_core::secrets::Secrets;

pub use zirc_core::signin::{SignIn, SignIns};

/// Where the sign-ins are kept, in zirc's directory `home`.
#[must_use]
pub fn path(home: &Path) -> PathBuf {
    home.join("tui.toml")
}

/// The sign-ins kept at `path`, one per organisation, in the order signed in to: none when
/// there's no file.
///
/// # Errors
///
/// When the file is there but can't be read.
pub fn load_all(path: &Path) -> Result<SignIns> {
    SignIns::load(path).with_context(|| format!("reading {}", path.display()))
}

/// The first sign-in kept at `path`, if there is one.
///
/// # Errors
///
/// As [`load_all`].
pub fn load(path: &Path) -> Result<Option<SignIn>> {
    Ok(load_all(path)?.iter().next().cloned())
}

/// Keeps `signin` at `path` beside the other organisations' sign-ins, in place of its own
/// organisation's if there was one, readable by this user only: it can hold the gate
/// password.
///
/// # Errors
///
/// When the file can't be read or written.
pub fn save(signin: &SignIn, path: &Path) -> Result<()> {
    let mut signins = load_all(path)?;
    signins.insert(signin.clone());
    signins
        .save(path)
        .with_context(|| format!("writing {}", path.display()))
}

/// Puts the sign-in to the organisation `org` under its new handle, after the person changed
/// it at zirc.dev ([`Client::set_handle`]): the same passphrase, kept under the new account.
///
/// # Errors
///
/// When there's no sign-in to `org`, or the file or the passphrase can't be written.
pub fn rename(home: &Path, secrets: &dyn Secrets, org: &str, handle: &str) -> Result<()> {
    let path = path(home);
    let signins = load_all(&path)?;
    let signin = signins
        .get(org)
        .with_context(|| format!("not signed in to {org}"))?;
    save(&signin.renamed(handle, secrets)?, &path)
}

/// Signs in at `web` through the browser, telling the person what to do on `out`, and
/// keeps the sign-in in zirc's directory `home` and `secrets`.
///
/// # Errors
///
/// When zirc.dev can't be reached or doesn't take sign-ins, or the person turns it down or
/// lets it expire.
pub async fn sign_in(
    web: &str,
    browser: bool,
    home: &Path,
    secrets: &dyn Secrets,
    out: &mut (impl Write + Send),
) -> Result<SignIn> {
    let client = Client::new(web)?;
    let mut said = Ok(());
    let credentials = zirc_core::signin::sign_in(
        &client,
        &format!("zirc-tui {}", env!("CARGO_PKG_VERSION")),
        &zirc_core::machine::host_name(),
        // The TUI keeps one sign-in per `zirc-tui login`; several come from `zirc login`.
        false,
        // The TUI signs in with the passphrase alone, for now.
        None,
        |started| said = prompt(out, started, browser),
    )
    .await;
    said?;
    let credentials = credentials?;
    if let Some(account_id) = &credentials.account_id {
        zirc_core::telemetry::link(home, account_id);
    }
    let signin = SignIn::from_credentials(&credentials, web, secrets)?;
    save(&signin, &path(home))?;
    let how = if signin.account {
        format!("as {}", signin.handle)
    } else {
        format!("as {}, with the server's password", signin.nick)
    };
    writeln!(out)?;
    // words-guard: a line the terminal prints of where it signed in, not Settings' label
    writeln!(out, "Signed in to {} {how}.", signin.server.host)?;
    Ok(signin)
}

/// Tells the person where to confirm the sign-in, and opens it in the browser if asked.
fn prompt(
    out: &mut impl Write,
    started: &zirc_proto::login::LoginStarted,
    browser: bool,
) -> std::io::Result<()> {
    writeln!(out, "Open this link to sign in to your team's chat:")?;
    writeln!(out)?;
    writeln!(out, "  {}", started.url)?;
    writeln!(out)?;
    writeln!(
        out,
        "It shows the code {}; check that it's this one.",
        started.code
    )?;
    if browser {
        login::open_browser(&started.url);
    }
    writeln!(
        out,
        "Waiting for you there. The link works for {}; Ctrl-C stops waiting.",
        login::link_lifetime(started)
    )?;
    out.flush()
}

/// Signs out of the sign-ins in zirc's directory `home`, saying so on `out`: every
/// organisation's, or `org`'s alone, leaving the others signed in. zirc.dev is told first, as
/// every app tells it (zirc-core's one rule: each key revoked, and with none left, the device
/// signed out of zirc.dev), and it signs out here whatever zirc.dev said. When the system's secret
/// store won't say it's forgotten a passphrase, as where there's no keychain to ask, it's
/// signed out all the same, and says so.
///
/// # Errors
///
/// When the sign-ins can't be read, written or removed.
pub async fn sign_out(
    home: &Path,
    secrets: &dyn Secrets,
    key: &Arc<dyn DeviceKey>,
    org: Option<&str>,
    out: &mut impl Write,
) -> Result<()> {
    let path = path(home);
    let signins = load_all(&path)?;
    let (leaving, staying): (Vec<&SignIn>, Vec<&SignIn>) = signins
        .iter()
        .partition(|signin| org.is_none_or(|org| signin.org() == org));
    if leaving.is_empty() {
        match org {
            Some(org) => writeln!(out, "Not signed in to {org}.")?,
            None => writeln!(out, "Not signed in.")?,
        }
        return Ok(());
    }
    let told = Leaving::of(
        leaving.iter().map(|&s| s.clone()).collect(),
        staying.is_empty(),
        |_| None,
    );
    for (what, err) in told.tell(key, secrets, None).await.failed {
        writeln!(out, "zirc.dev didn't hear this device leaves {what}: {err}")?;
    }
    if staying.is_empty() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    } else {
        let mut kept = SignIns::default();
        for signin in staying {
            kept.insert(signin.clone());
        }
        kept.save(&path)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    for signin in leaving {
        writeln!(
            out,
            "{}",
            zirc_core::account_text::signed_out(&signin.org())
        )?;
        if let Err(err) = signin.forget(secrets) {
            let why = std::error::Error::source(&err)
                .map_or_else(|| err.to_string(), ToString::to_string);
            writeln!(
                out,
                "Its passphrase is still kept, as {}, where the system's secret store \
                 wouldn't remove it: {}.",
                signin.key(),
                why.trim_end_matches('.')
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_core::secrets::File;

    /// A software device key, kept beside the tests' secrets.
    fn key(dir: &tempfile::TempDir) -> Arc<dyn DeviceKey> {
        let store: Arc<dyn Secrets> = Arc::new(File(dir.path().join("key.toml")));
        zirc_core::device_key::of(&store)
    }
    use zirc_proto::login::{Account, Credentials, Server};

    #[tokio::test]
    async fn keeps_the_passphrase_out_of_the_file_and_forgets_both() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let credentials = Credentials {
            guest: None,
            server: Server {
                host: "acme.irc.zirc.dev".into(),
                port: None,
                tls: None,
                password: Some("gate".into()),
            },
            account: Some(Account {
                name: "mattias".into(),
                password: "the passphrase".into(),
                device_key: false,
            }),
            nick: Some("mattias".into()),
            owner: None,
            realname: None,
            account_id: None,
            organisation_id: None,
            web_session: None,
            orgs: Vec::new(),
            media: None,
        };
        let signin = SignIn::from_credentials(&credentials, "https://zirc.dev", &secrets).unwrap();
        let path = path(dir.path());
        save(&signin, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("the passphrase"), "{text}");
        assert_eq!(load(&path).unwrap(), Some(signin.clone()));

        sign_out(dir.path(), &secrets, &key(&dir), None, &mut Vec::new())
            .await
            .unwrap();
        assert_eq!(load(&path).unwrap(), None);
        let mut out = Vec::new();
        sign_out(dir.path(), &secrets, &key(&dir), None, &mut out)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "Not signed in.\n");
        assert!(
            signin
                .connection(&secrets)
                .unwrap_err()
                .to_string()
                .contains("sign in again")
        );
    }

    /// A secret store that won't forget, as macOS's is with no keychain to open.
    struct Refusing(File);

    impl Secrets for Refusing {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key)
        }

        fn set(&self, key: &str, secret: &str) -> Result<(), zirc_core::secrets::Error> {
            self.0.set(key, secret)
        }

        fn delete(&self, _: &str) -> Result<(), zirc_core::secrets::Error> {
            Err(zirc_core::secrets::Error::File {
                path: "login.keychain".into(),
                source: std::io::Error::other("no keychain"),
            })
        }
    }

    #[tokio::test]
    async fn signs_out_once_said_when_the_passphrase_cant_be_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Refusing(File(dir.path().join("secrets.toml")));
        let credentials = Credentials {
            guest: None,
            server: Server {
                host: "acme.irc.zirc.dev".into(),
                port: None,
                tls: None,
                password: None,
            },
            account: Some(Account {
                name: "mattias".into(),
                password: "the passphrase".into(),
                device_key: false,
            }),
            nick: None,
            owner: None,
            realname: None,
            account_id: None,
            organisation_id: None,
            web_session: None,
            orgs: Vec::new(),
            media: None,
        };
        let signin = SignIn::from_credentials(&credentials, "https://zirc.dev", &secrets).unwrap();
        save(&signin, &path(dir.path())).unwrap();
        let mut out = Vec::new();
        sign_out(dir.path(), &secrets, &key(&dir), None, &mut out)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Signed out of Acme.\nIts passphrase is still kept, as \
             mattias@acme.irc.zirc.dev, where the system's secret store wouldn't remove it: \
             login.keychain: no keychain.\n"
        );
        assert_eq!(load(&path(dir.path())).unwrap(), None);
    }

    #[tokio::test]
    async fn keeps_a_sign_in_per_organisation_and_signs_out_of_one() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = File(dir.path().join("secrets.toml"));
        let signin = |host: &str| {
            let credentials = Credentials {
                guest: None,
                server: Server {
                    host: host.into(),
                    port: None,
                    tls: None,
                    password: Some("gate".into()),
                },
                account: None,
                nick: Some("mattias".into()),
                owner: None,
                realname: None,
                account_id: None,
                organisation_id: None,
                web_session: None,
                orgs: Vec::new(),
                media: None,
            };
            SignIn::from_credentials(&credentials, "https://zirc.dev", &secrets).unwrap()
        };
        let path = path(dir.path());
        save(&signin("acme.irc.zirc.dev"), &path).unwrap();
        save(&signin("irc.zirc.dev"), &path).unwrap();
        let orgs = |path: &Path| -> Vec<String> {
            load_all(path).unwrap().iter().map(SignIn::org).collect()
        };
        assert_eq!(orgs(&path), ["acme", "public"]);

        let mut out = Vec::new();
        sign_out(dir.path(), &secrets, &key(&dir), Some("public"), &mut out)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "Signed out of Public.\n");
        assert_eq!(orgs(&path), ["acme"]);
        let mut out = Vec::new();
        sign_out(dir.path(), &secrets, &key(&dir), Some("public"), &mut out)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Not signed in to public.\n"
        );
        // One organisation's file is the one sign-in it always was.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            toml::to_string(&signin("acme.irc.zirc.dev")).unwrap()
        );
    }
}
