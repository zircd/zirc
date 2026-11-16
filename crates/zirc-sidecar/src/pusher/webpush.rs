//! Web Push as the pusher sends it: a message encrypted for one phone (RFC 8291, `aes128gcm`,
//! RFC 8188), and the pusher's own VAPID signature (RFC 8292), which zirc.dev checks before it
//! relays a push (asking the sidecar for the key, `GET /v1/push`).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::elliptic_curve::sec1::ToEncodedPoint as _;
use p256::{PublicKey, SecretKey};
use ring::rand::{SecureRandom as _, SystemRandom};
use ring::signature::{self, EcdsaKeyPair, KeyPair as _};
use ring::{aead, hkdf};

/// The record size every push says: one record holds it all.
const RECORD: u32 = 4096;

/// Why a push couldn't be made.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The phone's keys aren't a P-256 point and a 16-byte secret.
    #[error("the phone's keys are malformed")]
    Keys,
    /// The system's randomness or a primitive failed.
    #[error("the cryptography failed")]
    Crypto,
}

impl From<ring::error::Unspecified> for Error {
    fn from(_: ring::error::Unspecified) -> Self {
        Self::Crypto
    }
}

/// HKDF's output length, as ring asks for it.
struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand<const N: usize>(prk: &hkdf::Prk, info: &[u8]) -> Result<[u8; N], Error> {
    let mut out = [0; N];
    prk.expand(&[info], Len(N))?.fill(&mut out)?;
    Ok(out)
}

/// `plain` encrypted for the phone whose public key is `p256dh` and auth secret `auth`, both
/// base64url as it registered them: the body of a push.
///
/// # Errors
///
/// When the keys are malformed, or the system's randomness fails.
pub fn encrypt(p256dh: &str, auth: &str, plain: &[u8]) -> Result<Vec<u8>, Error> {
    let random = SystemRandom::new();
    let mut salt = [0; 16];
    random.fill(&mut salt)?;
    let ours = loop {
        let mut bytes = [0; 32];
        random.fill(&mut bytes)?;
        if let Ok(key) = SecretKey::from_slice(&bytes) {
            break key;
        }
    };
    encrypt_with(&ours, salt, p256dh, auth, plain)
}

/// [`encrypt`], with the sender's key and salt given, as RFC 8291's example has them.
fn encrypt_with(
    ours: &SecretKey,
    salt: [u8; 16],
    p256dh: &str,
    auth: &str,
    plain: &[u8],
) -> Result<Vec<u8>, Error> {
    let theirs_raw = URL_SAFE_NO_PAD.decode(p256dh).map_err(|_| Error::Keys)?;
    let theirs = PublicKey::from_sec1_bytes(&theirs_raw).map_err(|_| Error::Keys)?;
    let auth = URL_SAFE_NO_PAD.decode(auth).map_err(|_| Error::Keys)?;
    if auth.len() != 16 {
        return Err(Error::Keys);
    }
    let ours_raw = ours.public_key().to_encoded_point(false);
    let ours_raw = ours_raw.as_bytes();
    let shared = p256::ecdh::diffie_hellman(ours.to_nonzero_scalar(), theirs.as_affine());
    // RFC 8291 §3.4: the input keying material, from the shared secret and the auth secret.
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(&theirs_raw);
    info.extend_from_slice(ours_raw);
    let prk_key =
        hkdf::Salt::new(hkdf::HKDF_SHA256, &auth).extract(shared.raw_secret_bytes().as_slice());
    let ikm: [u8; 32] = expand(&prk_key, &info)?;
    // RFC 8188 §2.2: the content encryption key and nonce.
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &salt).extract(&ikm);
    let cek: [u8; 16] = expand(&prk, b"Content-Encoding: aes128gcm\0")?;
    let nonce: [u8; 12] = expand(&prk, b"Content-Encoding: nonce\0")?;
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek)?);
    let mut record = plain.to_vec();
    record.push(2); // the last record's delimiter, no padding
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::empty(),
        &mut record,
    )?;
    let mut body = Vec::with_capacity(16 + 4 + 1 + ours_raw.len() + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD.to_be_bytes());
    body.push(u8::try_from(ours_raw.len()).map_err(|_| Error::Crypto)?);
    body.extend_from_slice(ours_raw);
    body.extend_from_slice(&record);
    Ok(body)
}

/// The pusher's VAPID key: an ECDSA P-256 key pair, kept as PKCS#8.
pub struct Vapid {
    pair: EcdsaKeyPair,
}

impl Vapid {
    /// A new key, as PKCS#8 to keep.
    ///
    /// # Errors
    ///
    /// When the system's randomness fails.
    pub fn make() -> Result<Vec<u8>, Error> {
        let random = SystemRandom::new();
        Ok(
            EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &random)?
                .as_ref()
                .to_vec(),
        )
    }

    /// The key kept as `pkcs8`.
    ///
    /// # Errors
    ///
    /// When it isn't one.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, Error> {
        let pair = EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|_| Error::Keys)?;
        Ok(Self { pair })
    }

    /// Its public key, base64url: what zirc.dev checks pushes against.
    #[must_use]
    pub fn public(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref())
    }

    /// The `Authorization` header for a push to `endpoint`, signed for its origin, `subject`
    /// saying who sends it, valid until `expires` (seconds since the Unix epoch).
    ///
    /// # Errors
    ///
    /// When `endpoint` isn't a URL, or signing fails.
    pub fn authorization(
        &self,
        endpoint: &str,
        subject: &str,
        expires: i64,
    ) -> Result<String, Error> {
        let url = reqwest::Url::parse(endpoint).map_err(|_| Error::Keys)?;
        let audience = url.origin().ascii_serialization();
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::json!({"aud": audience, "exp": expires, "sub": subject}).to_string(),
        );
        let signed = format!("{header}.{claims}");
        let signature = self
            .pair
            .sign(&SystemRandom::new(), signed.as_bytes())
            .map_err(|_| Error::Crypto)?;
        Ok(format!(
            "vapid t={signed}.{}, k={}",
            URL_SAFE_NO_PAD.encode(signature.as_ref()),
            self.public()
        ))
    }
}

/// `body`, a push encrypted for the phone with the private key `secret` and auth secret
/// `auth`, decrypted, as the phone decrypts it: for tests.
#[cfg(test)]
pub(crate) fn decrypt(secret: &SecretKey, auth: &[u8], body: &[u8]) -> Option<Vec<u8>> {
    let (salt, rest) = body.split_at_checked(16)?;
    let (_record, rest) = rest.split_at_checked(4)?;
    let (length, rest) = rest.split_first()?;
    let (sender, sealed) = rest.split_at_checked(usize::from(*length))?;
    let sender_key = PublicKey::from_sec1_bytes(sender).ok()?;
    let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), sender_key.as_affine());
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(secret.public_key().to_encoded_point(false).as_bytes());
    info.extend_from_slice(sender);
    let prk_key =
        hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(shared.raw_secret_bytes().as_slice());
    let ikm: [u8; 32] = expand(&prk_key, &info).ok()?;
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&ikm);
    let cek: [u8; 16] = expand(&prk, b"Content-Encoding: aes128gcm\0").ok()?;
    let nonce: [u8; 12] = expand(&prk, b"Content-Encoding: nonce\0").ok()?;
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).ok()?);
    let mut record = sealed.to_vec();
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::empty(),
            &mut record,
        )
        .ok()?;
    // Without the last record's delimiter.
    Some(plain.strip_suffix(&[2])?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(text: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(text).unwrap()
    }

    /// RFC 8291's Appendix A, byte for byte.
    #[test]
    fn encrypts_as_rfc_8291s_example() {
        let ours =
            SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body = encrypt_with(
            &ours,
            salt,
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
            "BTBZMqHH6r4Tts7J_aSIgg",
            b"When I grow up, I want to be a watermelon",
        )
        .unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn a_phone_decrypts_what_it_was_sent() {
        let phone =
            SecretKey::from_slice(&b64("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")).unwrap();
        let public = URL_SAFE_NO_PAD.encode(phone.public_key().to_encoded_point(false).as_bytes());
        let auth = "BTBZMqHH6r4Tts7J_aSIgg";
        let body = encrypt(&public, auth, b"PRIVMSG ada :hi").unwrap();
        assert_eq!(
            decrypt(&phone, &b64(auth), &body).as_deref(),
            Some(&b"PRIVMSG ada :hi"[..])
        );
    }

    #[test]
    fn refuses_keys_that_arent() {
        assert!(matches!(
            encrypt("not-a-key", "BTBZMqHH6r4Tts7J_aSIgg", b"x"),
            Err(Error::Keys)
        ));
    }

    #[test]
    fn signs_for_the_endpoints_origin() {
        let vapid = Vapid::from_pkcs8(&Vapid::make().unwrap()).unwrap();
        let header = vapid
            .authorization(
                "https://zirc.dev/api/push/abc",
                "https://zirc.dev/",
                1_800_000_000,
            )
            .unwrap();
        let (token, key) = header
            .strip_prefix("vapid t=")
            .unwrap()
            .split_once(", k=")
            .unwrap();
        assert_eq!(key, vapid.public());
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&b64(signed.split_once('.').unwrap().1)).unwrap();
        assert_eq!(claims["aud"], "https://zirc.dev");
        assert_eq!(claims["exp"], 1_800_000_000);
        signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, b64(key))
            .verify(signed.as_bytes(), &b64(signature))
            .unwrap();
    }
}
