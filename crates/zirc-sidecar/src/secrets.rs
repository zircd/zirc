//! How a server's own secrets are made, once, when the server is: by the controller for a
//! hosted one (as Kubernetes secrets), by `zirc-server` for a self-hosted one (in a file on
//! its disk). Which they are is [`crate::environment::Secret`] and the sidecar's options.

use rand::distr::{Alphanumeric, SampleString};

/// A password as `ergo genpasswd` makes them: 24 letters and digits, and its bcrypt hash
/// with `$2a$`, which Ergo reads.
///
/// # Panics
///
/// Never: bcrypt only refuses costs out of range and passwords over 72 bytes.
#[must_use]
pub fn password() -> (String, String) {
    let password = Alphanumeric.sample_string(&mut rand::rng(), 24);
    let hash = bcrypt::hash_with_result(&password, bcrypt::DEFAULT_COST)
        .expect("bcrypt hashes any password shorter than 72 bytes")
        .format_for_version(bcrypt::Version::TwoA);
    (password, hash)
}

/// A token: 43 letters and digits, about as strong as `ergo gentoken`'s.
#[must_use]
pub fn token() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 43)
}

/// Whether `a` and `b` are the same, compared in time that depends only on their lengths: for
/// tokens and passwords.
#[must_use]
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Ergo's API bearer tokens, as its config takes them, for the one `token`.
#[must_use]
pub fn bearer_tokens(token: &str) -> String {
    serde_json::json!([token]).to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_password_s_hash_is_ergo_s_kind_and_checks() {
        let (password, hash) = super::password();
        assert_eq!(password.len(), 24);
        assert!(hash.starts_with("$2a$"));
        assert!(bcrypt::verify(&password, &hash).unwrap());
    }
}
