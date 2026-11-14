//! Signing in with a key made on the device: what an app or zircd sends, and what the
//! server's sidecar checks.
//!
//! Each device makes a P-256 key pair and keeps the private half in its system's key store
//! (the Secure Enclave, Android's Keystore, the Mac's Keychain). zirc.dev registers the public
//! half for the device's account on the server. Then, at each connection, the device signs a
//! short token, an ES256 JWT, and logs in with it over SASL `OAUTHBEARER`. Ergo hands the token
//! to the sidecar (its `auth-script`), which checks it against the keys it holds, with no call
//! to zirc.dev, and keeps a log of every sign-in.
//!
//! - The public key travels as base64url (no padding) of its uncompressed SEC1 point, 65
//!   bytes starting `0x04`: what iOS's `SecKeyCopyExternalRepresentation` gives, and the end
//!   of Android's X.509 encoding. The same text is the key's id, `kid`, in the token's header,
//!   so nothing else has to be agreed.
//! - The token's claims are [`Claims`]: the account (`sub`), the server's host name (`aud`),
//!   when it was made and until when it's good (at most [`MAX_LIFETIME`] seconds), and a
//!   random `jti` the sidecar accepts once.
//! - The signature is ECDSA P-256 with SHA-256 over `<header>.<claims>`, as JWT's ES256 wants
//!   it: `r` and `s`, 32 bytes each. Key stores that give DER have it turned by [`fixed`].
//!
//! No cryptography here (the web client runs this crate as wasm): signers and the checker each
//! bring their own.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// The JWT algorithm: ECDSA on P-256 with SHA-256.
pub const ALG: &str = "ES256";

/// The longest a token may be good for, from `iat` to `exp`, in seconds: long enough for a
/// slow network, short enough that one seen by someone else is soon useless.
pub const MAX_LIFETIME: u64 = 120;

/// How far ahead of the server's clock a device's may be, in seconds.
pub const CLOCK_SKEW: u64 = 60;

/// The length of a public key's point, in bytes.
pub const POINT_LEN: usize = 65;

/// What a token says.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    /// The account signing in.
    pub sub: String,
    /// The server's host name, such as `acme.irc.zirc.dev`, so a token made for one server
    /// opens no other.
    pub aud: String,
    /// When it was made, in seconds since the epoch.
    pub iat: u64,
    /// Until when it's good, in seconds since the epoch.
    pub exp: u64,
    /// A random id, which the server accepts once.
    pub jti: String,
}

#[derive(Serialize, Deserialize)]
struct Header {
    alg: String,
    typ: String,
    kid: String,
}

/// The point of a public key given as text, if it is one: 65 bytes, uncompressed.
#[must_use]
pub fn point(key: &str) -> Option<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD.decode(key).ok()?;
    (bytes.len() == POINT_LEN && bytes[0] == 4).then_some(bytes)
}

/// A public key's point as text: its id, and how zirc.dev and the server are given it.
#[must_use]
pub fn key_text(point: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(point)
}

/// What the device signs for a token from the key `kid`.
#[must_use]
pub fn signing_input(kid: &str, claims: &Claims) -> String {
    let header = Header {
        alg: ALG.into(),
        typ: "JWT".into(),
        kid: kid.into(),
    };
    format!("{}.{}", part(&header), part(claims))
}

fn part<T: Serialize>(value: &T) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap_or_default())
}

/// The token: what was signed, and its 64-byte signature.
#[must_use]
pub fn token(signing_input: &str, signature: &[u8]) -> String {
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

/// A token taken apart, not yet checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Parsed {
    /// The key it says it was signed with.
    pub kid: String,
    /// What it says.
    pub claims: Claims,
    /// What was signed: the token up to its last dot.
    pub signing_input: String,
    /// The signature, `r` and `s`.
    pub signature: Vec<u8>,
}

/// Takes a token apart.
///
/// # Errors
///
/// When it isn't one of these: not three parts, not ES256, or claims missing.
pub fn parse(token: &str) -> Result<Parsed, &'static str> {
    let mut parts = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("not a device token");
    };
    let header: Header = URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|h| serde_json::from_slice(&h).ok())
        .ok_or("not a device token")?;
    if header.alg != ALG {
        return Err("not a device token");
    }
    let claims: Claims = URL_SAFE_NO_PAD
        .decode(claims)
        .ok()
        .and_then(|c| serde_json::from_slice(&c).ok())
        .ok_or("not a device token")?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "not a device token")?;
    if signature.len() != 64 {
        return Err("not a device token");
    }
    let signing_input =
        token[..token.len() - token.rsplit('.').next().unwrap_or_default().len() - 1].to_owned();
    Ok(Parsed {
        kid: header.kid,
        claims,
        signing_input,
        signature,
    })
}

impl Claims {
    /// New claims for `account` on server `aud` at `now`, good for [`MAX_LIFETIME`].
    #[must_use]
    pub fn new(account: &str, aud: &str, now: u64, jti: String) -> Self {
        Self {
            sub: account.into(),
            aud: aud.into(),
            iat: now,
            exp: now + MAX_LIFETIME,
            jti,
        }
    }

    /// Whether they're for server `aud` and good at `now`.
    ///
    /// # Errors
    ///
    /// Why not.
    pub fn check(&self, aud: &str, now: u64) -> Result<(), &'static str> {
        if !self.aud.eq_ignore_ascii_case(aud) {
            return Err("for another server");
        }
        if self.exp <= now {
            return Err("expired");
        }
        if self.iat > now + CLOCK_SKEW || self.exp < self.iat || self.exp - self.iat > MAX_LIFETIME
        {
            return Err("not in date");
        }
        if self.jti.is_empty() || self.jti.len() > 64 {
            return Err("no jti");
        }
        Ok(())
    }
}

/// A DER (X9.62) ECDSA signature as JWT's `r || s`, for key stores that give DER, such as
/// iOS's `SecKeyCreateSignature` and Android's `Signature`.
#[must_use]
pub fn fixed(der: &[u8]) -> Option<[u8; 64]> {
    fn int(der: &[u8]) -> Option<(&[u8], &[u8])> {
        let (&tag, rest) = der.split_first()?;
        let (&len, rest) = rest.split_first()?;
        let len = usize::from(len);
        (tag == 2 && len < 128 && rest.len() >= len).then(|| rest.split_at(len))
    }
    let (&tag, rest) = der.split_first()?;
    let (&len, rest) = rest.split_first()?;
    if tag != 0x30 || usize::from(len) != rest.len() {
        return None;
    }
    let (r, rest) = int(rest)?;
    let (s, rest) = int(rest)?;
    if !rest.is_empty() {
        return None;
    }
    let mut out = [0u8; 64];
    for (value, at) in [(r, 0), (s, 32)] {
        let value = match value {
            [0, tail @ ..] => tail,
            _ => value,
        };
        if value.len() > 32 {
            return None;
        }
        out[at + 32 - value.len()..at + 32].copy_from_slice(value);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(now: u64) -> Claims {
        Claims::new("ada", "acme.irc.zirc.dev", now, "j1".into())
    }

    #[test]
    fn a_token_comes_apart_as_it_was_made() {
        let kid = key_text(&[4; POINT_LEN]);
        let input = signing_input(&kid, &claims(1000));
        let made = token(&input, &[7; 64]);
        let parsed = parse(&made).unwrap();
        assert_eq!(parsed.kid, kid);
        assert_eq!(parsed.claims, claims(1000));
        assert_eq!(parsed.signing_input, input);
        assert_eq!(parsed.signature, vec![7; 64]);
    }

    #[test]
    fn only_es256_with_a_64_byte_signature() {
        let kid = key_text(&[4; POINT_LEN]);
        let input = signing_input(&kid, &claims(1000));
        assert!(parse(&token(&input, &[7; 70])).is_err());
        let eddsa = Header {
            alg: "EdDSA".into(),
            typ: "JWT".into(),
            kid,
        };
        let eddsa = format!("{}.{}", part(&eddsa), part(&claims(1000)));
        assert!(parse(&token(&eddsa, &[7; 64])).is_err());
        assert!(parse("a.b").is_err());
    }

    #[test]
    fn claims_are_for_one_server_and_a_short_while() {
        let c = claims(1000);
        assert_eq!(c.check("acme.irc.zirc.dev", 1000), Ok(()));
        assert_eq!(c.check("ACME.irc.zirc.dev", 1100), Ok(()));
        assert_eq!(
            c.check("other.irc.zirc.dev", 1000),
            Err("for another server")
        );
        assert_eq!(
            c.check("acme.irc.zirc.dev", 1000 + MAX_LIFETIME),
            Err("expired")
        );
        // Made in the device's future, beyond the skew.
        assert_eq!(
            c.check("acme.irc.zirc.dev", 1000 - CLOCK_SKEW - 1),
            Err("not in date")
        );
        let long = Claims {
            exp: 1000 + MAX_LIFETIME + 1,
            ..claims(1000)
        };
        assert_eq!(long.check("acme.irc.zirc.dev", 1000), Err("not in date"));
    }

    #[test]
    fn a_point_is_65_bytes_uncompressed() {
        assert!(point(&key_text(&[4; POINT_LEN])).is_some());
        assert!(point(&key_text(&[2; POINT_LEN])).is_none());
        assert!(point(&key_text(&[4; 33])).is_none());
        assert!(point("not base64!").is_none());
    }

    #[test]
    fn der_signatures_become_r_and_s() {
        // r with a leading zero (its top bit set), s short.
        let mut r = vec![0];
        r.extend([0x80; 32]);
        let s = vec![1; 31];
        let mut der = vec![0x30, u8::try_from(2 + r.len() + 2 + s.len()).unwrap()];
        der.extend([2, u8::try_from(r.len()).unwrap()]);
        der.extend(&r);
        der.extend([2, u8::try_from(s.len()).unwrap()]);
        der.extend(&s);
        let out = fixed(&der).unwrap();
        assert_eq!(out[..32], [0x80; 32]);
        assert_eq!(out[32], 0);
        assert_eq!(out[33..], [1; 31]);
        assert!(fixed(&der[..der.len() - 1]).is_none());
    }
}
