//! Sealed parts of messages with the system's randomness: zirc-chat's [`zirc_chat::sealed`],
//! which the browser runs with its own, for the apps, zircd and the CLI; and the
//! organisations' keys a device holds, in its secret store ([`Keys`]).

use ring::rand::{SecureRandom as _, SystemRandom};

use crate::secrets::Secrets;
pub use zirc_chat::sealed::{
    Context, KEY_LEN, Key, KeyError, MARKER, NONCE_LEN, Opened, SealError, Split, open_message,
    split,
};

/// Why a key couldn't be made or parts sealed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The system gave no random bytes.
    #[error("the system gave no random bytes")]
    NoRandom,
    /// The parts couldn't be sealed.
    #[error("{0}")]
    Seal(#[from] SealError),
    /// The secret store refused.
    #[error(transparent)]
    Secrets(#[from] crate::secrets::Error),
}

/// A new random key for `org`.
///
/// # Errors
///
/// [`Error::NoRandom`] when the system gives no random bytes.
pub fn new_key(org: impl Into<String>) -> Result<Key, Error> {
    Ok(Key::new(org, random()?))
}

/// The value of the `+zirc.dev/sealed` tag for `parts`, each sealed with `key` and a random
/// nonce. `room` is what's left of the message's client tags after its others.
///
/// # Errors
///
/// [`Error::Seal`] when they won't fit in `room`, [`Error::NoRandom`] when the system gives
/// no random bytes.
pub fn seal_message(
    key: &Key,
    context: &Context,
    parts: &[String],
    room: usize,
) -> Result<String, Error> {
    let mut nonces = Vec::with_capacity(parts.len());
    for _ in parts {
        nonces.push(random::<NONCE_LEN>()?);
    }
    let mut nonces = nonces.into_iter();
    Ok(zirc_chat::sealed::seal_message(
        key,
        context,
        parts,
        || nonces.next().unwrap_or_default(),
        room,
    )?)
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::NoRandom)?;
    Ok(bytes)
}

/// Where zircd and the `zirc` CLI keep the organisations' keys: `secrets.toml` in zirc's
/// directory `home`, readable by its user only, as they keep their sign-in in its
/// `config.toml`. The desktop app's store ([`crate::secrets::system`]) reads it too.
#[must_use]
pub fn machine_store(home: &std::path::Path) -> crate::secrets::File {
    crate::secrets::File(crate::paths::secrets(home))
}

/// What zircd and the CLI say when this machine has no key for `org`.
#[must_use]
pub fn no_key(org: &str) -> String {
    format!(
        "this machine has no key for {org}'s sealed parts: add the one a teammate has with \
         `zirc team-key add`, or make the organisation's first with `zirc team-key new`"
    )
}

/// The organisations' keys a device holds, in its secret store: for each organisation, one
/// secret named [`Keys::name`] with a key string a line, the newest first. The newest seals;
/// the others still open what was sealed with them.
pub struct Keys<'a> {
    secrets: &'a dyn Secrets,
}

impl<'a> Keys<'a> {
    /// The keys `secrets` holds.
    #[must_use]
    pub fn new(secrets: &'a dyn Secrets) -> Self {
        Self { secrets }
    }

    /// The name of the secret holding `org`'s keys.
    #[must_use]
    pub fn name(org: &str) -> String {
        format!("sealed@{org}")
    }

    /// `org`'s keys, the newest first. A line that isn't a key, which zirc never writes, is
    /// passed over.
    #[must_use]
    pub fn of(&self, org: &str) -> Vec<Key> {
        self.secrets
            .get(&Self::name(org))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| Key::parse(line).ok())
            .filter(|key| key.org() == org)
            .collect()
    }

    /// The key `org`'s parts are sealed with now, the newest.
    #[must_use]
    pub fn sealing(&self, org: &str) -> Option<Key> {
        self.of(org).into_iter().next()
    }

    /// Keeps `key` as its organisation's newest; `false` when it was already the newest.
    ///
    /// # Errors
    ///
    /// When the secret store refuses.
    pub fn add(&self, key: &Key) -> Result<bool, Error> {
        let mut keys = self.of(key.org());
        if keys.first() == Some(key) {
            return Ok(false);
        }
        keys.retain(|kept| kept != key);
        keys.insert(0, key.clone());
        self.keep(key.org(), &keys)?;
        Ok(true)
    }

    /// Forgets `org`'s key with id `id`; `false` when there's none.
    ///
    /// # Errors
    ///
    /// When the secret store refuses.
    pub fn forget(&self, org: &str, id: &str) -> Result<bool, Error> {
        let mut keys = self.of(org);
        let before = keys.len();
        keys.retain(|key| key.id() != id);
        if keys.len() == before {
            return Ok(false);
        }
        self.keep(org, &keys)?;
        Ok(true)
    }

    fn keep(&self, org: &str, keys: &[Key]) -> Result<(), Error> {
        if keys.is_empty() {
            return Ok(self.secrets.delete(&Self::name(org))?);
        }
        let lines: Vec<String> = keys.iter().map(Key::export).collect();
        Ok(self.secrets.set(&Self::name(org), &lines.join("\n"))?)
    }
}

#[cfg(test)]
mod tests {
    use zirc_proto::sealed::{CLIENT_TAGS_MAX, Sealed};

    use super::*;

    const ACME: Context = Context {
        org: "acme",
        target: "#api",
        sender: "alice",
    };

    #[test]
    fn keeps_each_organisations_keys_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let file = crate::secrets::File(dir.path().join("secrets.toml"));
        let keys = Keys::new(&file);
        let (old, new, other) = (
            new_key("acme").unwrap(),
            new_key("acme").unwrap(),
            new_key("globex").unwrap(),
        );
        assert_eq!(keys.sealing("acme"), None);
        assert!(keys.add(&old).unwrap());
        assert!(keys.add(&new).unwrap());
        assert!(!keys.add(&new).unwrap());
        keys.add(&other).unwrap();
        assert_eq!(keys.of("acme"), [new.clone(), old.clone()]);
        assert_eq!(keys.sealing("acme"), Some(new.clone()));
        assert_eq!(keys.of("globex"), [other]);

        // Added again, an older key seals once more.
        keys.add(&old).unwrap();
        assert_eq!(keys.of("acme"), [old.clone(), new.clone()]);

        assert!(keys.forget("acme", &old.id()).unwrap());
        assert!(!keys.forget("acme", &old.id()).unwrap());
        assert_eq!(keys.of("acme"), std::slice::from_ref(&new));
        keys.forget("acme", &new.id()).unwrap();
        assert_eq!(file.get(&Keys::name("acme")), None);
    }

    #[test]
    fn makes_a_different_key_each_time() {
        let key = new_key("acme").unwrap();
        assert_ne!(key, new_key("acme").unwrap());
        assert_eq!(key.org(), "acme");
        assert_eq!(Key::parse(&key.export()), Ok(key));
    }

    #[test]
    fn seals_each_part_with_its_own_nonce() {
        let key = new_key("acme").unwrap();
        let parts = vec!["same".to_owned(), "same".to_owned()];
        let value = seal_message(&key, &ACME, &parts, CLIENT_TAGS_MAX).unwrap();
        let sealed = Sealed::from_tag(&value).unwrap();
        assert_ne!(sealed.parts[0].n, sealed.parts[1].n);
        assert_eq!(
            open_message(&[key], &ACME, &value),
            Some(vec![
                Opened::Value("same".into()),
                Opened::Value("same".into())
            ])
        );
    }
}
