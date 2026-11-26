//! The device's own key, which it signs in to its organisations' servers with
//! ([`zirc_proto::device_key`]): the server checks it by itself, so the device keeps
//! connecting while zirc.dev is down.
//!
//! A [`DeviceKey`] is wherever the device keeps it: a phone's Secure Enclave or Keystore,
//! through the app (`oss/apps/mobile/core`), or, on a computer, a [`Software`] key, kept as
//! PKCS#8 in the system's secret store (the desktop app) or a file only its user can read
//! (zircd, as its machine passphrase is in `config.toml`). One key serves every
//! organisation the device signs in to: zirc.dev registers it for each account.

use std::path::PathBuf;
use std::sync::Arc;
use zirc_proto::hex;

use ring::rand::{SecureRandom as _, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use zirc_proto::device_key::{self, Claims};

use crate::secrets::Secrets;

/// A device's key pair, whose private half never leaves where it's kept.
pub trait DeviceKey: Send + Sync {
    /// The public half, as text ([`device_key::key_text`]), the key made the first time.
    /// `None` when it can't be had.
    fn public_key(&self) -> Option<String>;

    /// `data` signed with the private half, as ES256's `r || s`.
    fn sign(&self, data: &[u8]) -> Option<[u8; 64]>;
}

/// What the secret store keeps the key under.
pub const SECRET: &str = "zirc-device-key";

/// A key made here with ring, kept as hex PKCS#8.
pub struct Software {
    kept: Kept,
    pair: std::sync::Mutex<Option<Arc<EcdsaKeyPair>>>,
}

enum Kept {
    Secrets(Arc<dyn Secrets>),
    File(PathBuf),
}

impl Software {
    /// A key kept in `secrets`.
    #[must_use]
    pub fn in_secrets(secrets: Arc<dyn Secrets>) -> Self {
        Self::new(Kept::Secrets(secrets))
    }

    /// The machine's key, which `zirc login` sends and zircd signs in with, in zirc's
    /// directory `home` ([`crate::paths::device_key`]).
    #[must_use]
    pub fn machine(home: &std::path::Path) -> Self {
        Self::in_file(crate::paths::device_key(home))
    }

    /// A key kept in the file at `path`, readable by its user alone.
    #[must_use]
    pub fn in_file(path: PathBuf) -> Self {
        Self::new(Kept::File(path))
    }

    fn new(kept: Kept) -> Self {
        Self {
            kept,
            pair: std::sync::Mutex::new(None),
        }
    }

    fn read(&self) -> Option<String> {
        match &self.kept {
            Kept::Secrets(secrets) => secrets.get(SECRET),
            Kept::File(path) => std::fs::read_to_string(path).ok(),
        }
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
    }

    fn write(&self, text: &str) -> Option<()> {
        match &self.kept {
            Kept::Secrets(secrets) => secrets.set(SECRET, text).ok(),
            Kept::File(path) => crate::paths::write_private(path, text).ok(),
        }
    }

    fn pair(&self) -> Option<Arc<EcdsaKeyPair>> {
        let mut pair = self
            .pair
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pair) = &*pair {
            return Some(pair.clone());
        }
        let rng = SystemRandom::new();
        let pkcs8 = if let Some(pkcs8) = self.read().and_then(|hex| zirc_proto::unhex(&hex)) {
            pkcs8
        } else {
            let made = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .ok()?
                .as_ref()
                .to_vec();
            self.write(&hex(&made))?;
            // Another program may have made one meanwhile: that one's is the key.
            self.read().and_then(|hex| zirc_proto::unhex(&hex))?
        };
        let made = Arc::new(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng).ok()?,
        );
        *pair = Some(made.clone());
        Some(made)
    }
}

impl DeviceKey for Software {
    fn public_key(&self) -> Option<String> {
        Some(device_key::key_text(self.pair()?.public_key().as_ref()))
    }

    fn sign(&self, data: &[u8]) -> Option<[u8; 64]> {
        let signature = self.pair()?.sign(&SystemRandom::new(), data).ok()?;
        signature.as_ref().try_into().ok()
    }
}

/// The device's key that `secrets` has: the store's own ([`Secrets::device_key`]), or a
/// [`Software`] key kept among its secrets.
#[must_use]
pub fn of(secrets: &Arc<dyn Secrets>) -> Arc<dyn DeviceKey> {
    secrets
        .device_key()
        .unwrap_or_else(|| Arc::new(Software::in_secrets(secrets.clone())))
}

/// A token for `account` on `server`, signed by `key` now: what the device logs in with.
#[must_use]
pub fn token(key: &dyn DeviceKey, account: &str, server: &str) -> Option<String> {
    token_lasting(key, account, server, device_key::MAX_LIFETIME)
}

/// As [`token`], good for `lifetime` seconds: zirc.dev takes the account-level ones for at
/// most 60 ([`crate::account_keys`]).
#[must_use]
pub fn token_lasting(
    key: &dyn DeviceKey,
    account: &str,
    server: &str,
    lifetime: u64,
) -> Option<String> {
    let kid = key.public_key()?;
    let mut jti = [0u8; 16];
    SystemRandom::new().fill(&mut jti).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let mut claims = Claims::new(account, server, now, hex(&jti));
    claims.exp = now + lifetime;
    let input = device_key::signing_input(&kid, &claims);
    let signature = key.sign(input.as_bytes())?;
    Some(device_key::token(&input, &signature))
}

/// What [`zirc_irc::Config::bearer`] takes: a fresh token at each connection.
#[must_use]
pub fn bearer(key: Arc<dyn DeviceKey>, account: String, server: String) -> zirc_irc::Bearer {
    Arc::new(move || token(&*key, &account, &server))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifies(token: &str, account: &str, server: &str) -> bool {
        let parsed = device_key::parse(token).unwrap();
        let point = device_key::point(&parsed.kid).unwrap();
        parsed.claims.sub == account
            && parsed.claims.check(server, parsed.claims.iat).is_ok()
            && ring::signature::UnparsedPublicKey::new(
                &ring::signature::ECDSA_P256_SHA256_FIXED,
                point,
            )
            .verify(parsed.signing_input.as_bytes(), &parsed.signature)
            .is_ok()
    }

    #[test]
    fn a_key_in_a_file_is_made_once_and_signs_tokens_the_server_takes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-key");
        let key = Software::in_file(path.clone());
        let public = key.public_key().unwrap();
        assert!(device_key::point(&public).is_some());
        // The same key again, from the file.
        assert_eq!(Software::in_file(path.clone()).public_key(), Some(public));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let one = token(&key, "ada-laptop", "zirc.irc.zirc.dev").unwrap();
        let two = token(&key, "ada-laptop", "zirc.irc.zirc.dev").unwrap();
        assert!(verifies(&one, "ada-laptop", "zirc.irc.zirc.dev"));
        assert_ne!(
            device_key::parse(&one).unwrap().claims.jti,
            device_key::parse(&two).unwrap().claims.jti
        );
    }

    #[test]
    fn a_bearer_makes_a_fresh_token_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let key: Arc<dyn DeviceKey> = Arc::new(Software::in_file(dir.path().join("k")));
        let make = bearer(key, "ada".into(), "zirc.irc.zirc.dev".into());
        let (one, two) = (make().unwrap(), make().unwrap());
        assert_ne!(one, two);
        assert!(verifies(&two, "ada", "zirc.irc.zirc.dev"));
    }
}
