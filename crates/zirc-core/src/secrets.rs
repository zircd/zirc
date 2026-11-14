//! Where zirc's apps keep the passphrases they sign in with: the system's secret store (the
//! macOS Keychain, the Windows Credential Manager, or the Secret Service on Linux and the
//! BSDs), or a file of zirc's own where there's no such store, as on a server, and in tests.
//!
//! Every app keeps them under one service, [`SERVICE`], and the same [`key`], so signing in
//! once on a machine serves each of a person's apps there. A zirc directory other than the
//! usual `~/.zirc`, such as a trial's `$ZIRC_HOME`, keeps its own ([`service`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The service passphrases are kept under in the system's store.
pub const SERVICE: &str = "dev.zirc";

/// Why a secret couldn't be kept or forgotten.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The system's secret store refused. Only the innermost reason is said: keyring's
    /// errors repeat their sources' words.
    #[error("the system's secret store: {}", innermost(.0))]
    Keyring(keyring::Error),
    /// The file couldn't be read or written.
    #[error("{path}: {source}")]
    File {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The file isn't the TOML zirc writes, so it's left as it is rather than replaced.
    #[error("{path} isn't readable, so zirc leaves it alone: {source}")]
    Unreadable {
        /// The file.
        path: PathBuf,
        /// What's wrong with it.
        source: toml::de::Error,
    },
}

/// A store of secrets by name.
pub trait Secrets: Send + Sync {
    /// The secret named `key`, if there is one.
    fn get(&self, key: &str) -> Option<String>;

    /// Keeps `secret` as `key`, replacing any.
    ///
    /// # Errors
    ///
    /// When the store refuses it.
    fn set(&self, key: &str, secret: &str) -> Result<(), Error>;

    /// Forgets `key`; nothing happens if it isn't there.
    ///
    /// # Errors
    ///
    /// When the store refuses it.
    fn delete(&self, key: &str) -> Result<(), Error>;

    /// The device's own key, where the store keeps one apart from its secrets, as a phone's
    /// Secure Enclave or Keystore does. `None` for a store whose device key is a secret like
    /// any other ([`crate::device_key::of`]).
    fn device_key(&self) -> Option<std::sync::Arc<dyn crate::device_key::DeviceKey>> {
        None
    }
}

impl From<keyring::Error> for Error {
    fn from(err: keyring::Error) -> Self {
        Self::Keyring(err)
    }
}

/// What `err` says at its root, which is all its sources say again.
fn innermost(err: &keyring::Error) -> String {
    let mut root: &dyn std::error::Error = err;
    while let Some(source) = root.source() {
        root = source;
    }
    root.to_string()
}

/// The name a sign-in's passphrase is kept under: the account and the server it's for, as
/// `mattias@acme.irc.zirc.dev`, so accounts on several organisations' servers don't collide.
#[must_use]
pub fn key(account: &str, host: &str) -> String {
    format!("{account}@{host}")
}

/// The secrets of the person using this machine: in the system's store, falling back to
/// `secrets.toml` in zirc's directory `home` where the system has no store, or it refuses.
#[must_use]
pub fn system(home: &Path) -> Box<dyn Secrets> {
    let file = File(crate::paths::secrets(home));
    if keyring::Entry::store_status().is_ok() {
        Box::new(Fallback {
            first: Keyring::new(&service(home)),
            then: file,
        })
    } else {
        Box::new(file)
    }
}

/// The service the system's store keeps zirc directory `home`'s passphrases under:
/// [`SERVICE`] for `~/.zirc`, which every app on the machine shares, and one naming the
/// directory for any other, so a trial with its own `$ZIRC_HOME` never reads or replaces
/// the sign-ins of the person using the machine.
#[must_use]
pub fn service(home: &Path) -> String {
    service_beside(home, std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

fn service_beside(home: &Path, user_home: Option<&Path>) -> String {
    if user_home.is_some_and(|user_home| home == user_home.join(".zirc")) {
        SERVICE.to_owned()
    } else {
        format!("{SERVICE} {}", home.display())
    }
}

/// The system's secret store, under one service.
#[derive(Debug, Clone)]
pub struct Keyring {
    service: String,
}

impl Keyring {
    /// The store's secrets under `service`, such as [`SERVICE`].
    #[must_use]
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, Error> {
        Ok(keyring::Entry::new(&self.service, key)?)
    }
}

impl Secrets for Keyring {
    fn get(&self, key: &str) -> Option<String> {
        self.entry(key).ok()?.get_password().ok()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), Error> {
        Ok(self.entry(key)?.set_password(secret)?)
    }

    fn delete(&self, key: &str) -> Result<(), Error> {
        match self.entry(key)?.delete_credential() {
            Err(keyring::Error::NoEntry) => Ok(()),
            other => Ok(other?),
        }
    }
}

/// Secrets in a TOML file readable by its owner only.
#[derive(Debug, Clone)]
pub struct File(pub PathBuf);

impl File {
    /// The secrets in the file, none when there's no file yet. A file that can't be read is
    /// an error, so that keeping one secret never replaces the others with nothing.
    fn read(&self) -> Result<BTreeMap<String, String>, Error> {
        let text = match std::fs::read_to_string(&self.0) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(source) => {
                return Err(Error::File {
                    path: self.0.clone(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| Error::Unreadable {
            path: self.0.clone(),
            source,
        })
    }

    fn write(&self, secrets: &BTreeMap<String, String>) -> Result<(), Error> {
        let text = toml::to_string(secrets).expect("strings serialize to TOML");
        crate::paths::write_private(&self.0, &text).map_err(|source| Error::File {
            path: self.0.clone(),
            source,
        })
    }
}

impl Secrets for File {
    fn get(&self, key: &str) -> Option<String> {
        self.read().ok()?.remove(key)
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), Error> {
        let mut secrets = self.read()?;
        secrets.insert(key.to_owned(), secret.to_owned());
        self.write(&secrets)
    }

    fn delete(&self, key: &str) -> Result<(), Error> {
        let mut secrets = self.read()?;
        if secrets.remove(key).is_some() {
            self.write(&secrets)?;
        }
        Ok(())
    }
}

/// One store, and another for when it refuses: a Secret Service that isn't running, say.
struct Fallback<A, B> {
    first: A,
    then: B,
}

impl<A: Secrets, B: Secrets> Secrets for Fallback<A, B> {
    fn get(&self, key: &str) -> Option<String> {
        self.first.get(key).or_else(|| self.then.get(key))
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), Error> {
        match self.first.set(key, secret) {
            // Kept in the first: an older copy in the other would only mislead. One that
            // can't be forgotten, as in a file that can't be read, can't be read either.
            Ok(()) => {
                let _ = self.then.delete(key);
                Ok(())
            }
            Err(_) => self.then.set(key, secret),
        }
    }

    fn delete(&self, key: &str) -> Result<(), Error> {
        let first = self.first.delete(key);
        self.then.delete(key)?;
        match first {
            // Refused by a store that doesn't hold it, as a Keychain that can't be found:
            // there was nothing there to forget.
            Err(_) if self.first.get(key).is_none() => Ok(()),
            first => first,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store that refuses everything, as a Secret Service that isn't running does.
    struct Refusing;

    impl Secrets for Refusing {
        fn get(&self, _: &str) -> Option<String> {
            None
        }

        fn set(&self, _: &str, _: &str) -> Result<(), Error> {
            Err(keyring::Error::NoStorageAccess("no Secret Service".into()).into())
        }

        fn delete(&self, _: &str) -> Result<(), Error> {
            Err(keyring::Error::NoStorageAccess("no Secret Service".into()).into())
        }
    }

    #[test]
    fn keeps_another_zirc_directorys_passphrases_apart() {
        let user = Path::new("/Users/mattias");
        assert_eq!(service_beside(&user.join(".zirc"), Some(user)), SERVICE);
        assert_eq!(
            service_beside(Path::new("/Users/mattias/.zirc/"), Some(user)),
            SERVICE
        );
        assert_eq!(
            service_beside(Path::new("/tmp/trial"), Some(user)),
            "dev.zirc /tmp/trial"
        );
        assert_eq!(
            service_beside(Path::new("/tmp/trial"), None),
            "dev.zirc /tmp/trial"
        );
    }

    #[test]
    fn keeps_and_forgets_secrets_in_a_private_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let file = File(dir.path().join("zirc/secrets.toml"));
        let key = key("mattias", "acme.irc.zirc.dev");
        assert_eq!(key, "mattias@acme.irc.zirc.dev");
        assert_eq!(file.get(&key), None);
        file.set(&key, "pass phrase").unwrap();
        assert_eq!(file.get(&key).as_deref(), Some("pass phrase"));
        let mode = std::fs::metadata(&file.0).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        file.delete(&key).unwrap();
        file.delete(&key).unwrap();
        assert_eq!(file.get(&key), None);
    }

    #[test]
    fn leaves_an_unreadable_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = File(dir.path().join("secrets.toml"));
        std::fs::write(&file.0, "not = [toml").unwrap();
        assert_eq!(file.get("k"), None);
        assert!(matches!(
            file.set("k", "secret"),
            Err(Error::Unreadable { .. })
        ));
        assert!(matches!(file.delete("k"), Err(Error::Unreadable { .. })));
        assert_eq!(std::fs::read_to_string(&file.0).unwrap(), "not = [toml");
    }

    #[test]
    fn falls_back_to_the_file_where_the_system_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let store = Fallback {
            first: Refusing,
            then: File(dir.path().join("secrets.toml")),
        };
        store.set("k", "secret").unwrap();
        assert_eq!(store.get("k").as_deref(), Some("secret"));
        // The system refuses, but holds nothing: forgotten, as where there's no Keychain.
        store.delete("k").unwrap();
        assert_eq!(store.get("k"), None);
    }

    #[test]
    fn says_when_the_first_store_holds_it_and_wont_forget() {
        /// Holds every secret, and won't forget any.
        struct Holding;

        impl Secrets for Holding {
            fn get(&self, _: &str) -> Option<String> {
                Some("kept".into())
            }

            fn set(&self, _: &str, _: &str) -> Result<(), Error> {
                Ok(())
            }

            fn delete(&self, _: &str) -> Result<(), Error> {
                Refusing.delete("")
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let store = Fallback {
            first: Holding,
            then: File(dir.path().join("secrets.toml")),
        };
        assert!(store.delete("k").is_err());
    }

    #[test]
    fn keeps_one_copy_where_the_first_store_takes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (first, then) = (dir.path().join("a.toml"), dir.path().join("b.toml"));
        File(then.clone()).set("k", "old").unwrap();
        let store = Fallback {
            first: File(first.clone()),
            then: File(then.clone()),
        };
        store.set("k", "new").unwrap();
        assert_eq!(File(first).get("k").as_deref(), Some("new"));
        assert_eq!(File(then).get("k"), None);
    }
}
