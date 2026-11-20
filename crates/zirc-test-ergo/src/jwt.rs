//! A stand-in for zirc.dev's token issuer: an Ed25519 key pair that signs JWTs naming an
//! account, and the overrides that have Ergo's `jwt-auth` accept them, as production Ergo
//! accepts zirc.dev's.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

/// The DER prefix of an Ed25519 `SubjectPublicKeyInfo`, before the 32-byte key.
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// A key pair, fresh for each test.
pub struct Issuer {
    key: Ed25519KeyPair,
}

impl Default for Issuer {
    fn default() -> Self {
        Self::new()
    }
}

impl Issuer {
    /// A new key pair.
    ///
    /// # Panics
    ///
    /// When the system has no randomness.
    #[must_use]
    pub fn new() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("a key");
        let key = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("the key just made");
        Self { key }
    }

    /// The public key, PEM-encoded, as Ergo's `jwt-auth` takes it.
    #[must_use]
    pub fn public_pem(&self) -> String {
        let der = [&SPKI_PREFIX[..], self.key.public_key().as_ref()].concat();
        format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(der)
        )
    }

    /// The value of `ERGO__ACCOUNTS__JWT_AUTH__TOKENS` that trusts this issuer, the account
    /// in claim `account`.
    #[must_use]
    pub fn ergo_tokens(&self) -> String {
        let token = format!(
            r#"[{{"algorithm": "eddsa", "key": {}, "account-claims": ["account"]}}]"#,
            json_string(&self.public_pem())
        );
        token
    }

    /// A token for `account` that expires at `exp`, in seconds since 1970.
    #[must_use]
    pub fn token(&self, account: &str, exp: u64) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"EdDSA","typ":"JWT"}"#);
        let claims = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"account":{},"exp":{exp}}}"#,
            json_string(account)
        ));
        let signed = format!("{header}.{claims}");
        let signature = URL_SAFE_NO_PAD.encode(self.key.sign(signed.as_bytes()));
        format!("{signed}.{signature}")
    }
}

/// `text` as a JSON string. Only what PEM and account names hold needs escaping.
fn json_string(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_a_token_its_public_key_checks() {
        let issuer = Issuer::new();
        let token = issuer.token("alice", 4_000_000_000);
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let pem = issuer.public_pem();
        let der = STANDARD.decode(pem.lines().nth(1).unwrap()).unwrap();
        let public = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &der[12..]);
        public
            .verify(
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
            )
            .unwrap();
        assert!(issuer.ergo_tokens().contains("BEGIN PUBLIC KEY-----\\n"));
    }
}
