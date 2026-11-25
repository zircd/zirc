//! Signing a card's press as zirc.dev relays it, and checking one as an integration takes it:
//! [`zirc_proto::card::SIGNATURE_HEADER`], an HMAC-SHA256 of the time and the body with the
//! integration's secret. zirc-proto says the format; this does the cryptography, with the
//! `ring` zirc already uses, so zirc-proto stays free of it for the web client's wasm.

use zirc_proto::card::{SIGNATURE_MAX_AGE_SECS, Signature, signature_header, signed_text};

/// The [`zirc_proto::card::SIGNATURE_HEADER`] for `body`, signed at `t` (unix seconds) with
/// `secret`.
#[must_use]
pub fn sign(secret: &str, body: &str, t: u64) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let mac = ring::hmac::sign(&key, signed_text(t, body).as_bytes());
    signature_header(t, mac.as_ref())
}

/// Whether `header` signs `body` with `secret`, within
/// [`zirc_proto::card::SIGNATURE_MAX_AGE_SECS`] of `now`.
///
/// # Errors
///
/// What's wrong with it, when it doesn't.
pub fn verify(
    secret: &str,
    header: Option<&str>,
    body: &str,
    now: u64,
) -> Result<(), &'static str> {
    let signature = Signature::parse(header.ok_or("no Zirc-Signature")?)?;
    if now.abs_diff(signature.t) > SIGNATURE_MAX_AGE_SECS {
        return Err("the signature is too old");
    }
    zirc_proto::verifies(
        secret,
        signed_text(signature.t, body).as_bytes(),
        &signature.v1,
    )
    .then_some(())
    .ok_or("the signature doesn't match")
}

/// The lowercase-hex HMAC-SHA256 of `body` under `secret`: what Gitea and Forgejo send as
/// `X-Forgejo-Signature`, and [`verify_hex`] checks.
pub use zirc_proto::sign_hex;

/// Whether `header` is the lowercase-hex HMAC-SHA256 of `body` under `secret`, as Gitea and
/// Forgejo sign a webhook (`X-Forgejo-Signature`, no timestamp: the caller keeps the delivery
/// ids it has seen). Compared in constant time.
///
/// # Errors
///
/// What's wrong with it, when it doesn't.
pub fn verify_hex(secret: &str, header: Option<&str>, body: &[u8]) -> Result<(), &'static str> {
    let header = header.ok_or("no signature")?.trim();
    if header.len() != 64 || !header.is_ascii() {
        return Err("the signature isn't SHA-256 in hex");
    }
    zirc_proto::unhex(header).ok_or("the signature isn't hex")?;
    zirc_proto::verify_hex(secret, body, header)
        .then_some(())
        .ok_or("the signature doesn't match")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_proto::card::SIGNATURE_EXAMPLE;

    #[test]
    fn signs_as_the_example_says_and_checks_what_it_signed() {
        let [secret, t, body, header] = SIGNATURE_EXAMPLE;
        let t: u64 = t.parse().unwrap();
        assert_eq!(sign(secret, body, t), header);
        assert_eq!(verify(secret, Some(header), body, t + 10), Ok(()));
        assert_eq!(
            verify(secret, Some(header), body, t + SIGNATURE_MAX_AGE_SECS + 1),
            Err("the signature is too old")
        );
        assert_eq!(
            verify("another", Some(header), body, t),
            Err("the signature doesn't match")
        );
        assert_eq!(
            verify(secret, Some(header), &body.replace("retry", "cancel"), t),
            Err("the signature doesn't match")
        );
        assert_eq!(verify(secret, None, body, t), Err("no Zirc-Signature"));
    }

    #[test]
    fn takes_a_forgejo_signature_in_hex() {
        let body = br#"{"action":"opened"}"#;
        let good = sign_hex("s3cret", body);
        assert_eq!(verify_hex("s3cret", Some(&good), body), Ok(()));
        assert!(verify_hex("other", Some(&good), body).is_err());
        assert!(verify_hex("s3cret", Some(&good), b"{}").is_err());
        assert!(verify_hex("s3cret", None, body).is_err());
        assert!(verify_hex("s3cret", Some("zz"), body).is_err());
        assert!(verify_hex("s3cret", Some(&"g".repeat(64)), body).is_err());
        assert!(verify_hex("s3cret", Some(&"é".repeat(32)), body).is_err());
    }
}
