//! Sealing parts of a message with the organisation's shared key, and opening them.
//!
//! Someone writes `the key is ||sk_test_123||`; [`split`] takes the part between the
//! markers out, leaving `the key is [sealed]`, and [`seal_message`] seals it into the value
//! of the message's `+zirc.dev/sealed` tag ([`zirc_proto::sealed`]). Devices holding the key
//! [`open_message`] it again.
//!
//! - **The key** is 32 random bytes, one for each organisation, which people hand to each
//!   other's devices as a string ([`Key::export`], [`Key::parse`]) or its QR code. A device
//!   may hold several, so history sealed with an older one still opens. Its id, base64url
//!   of the first 8 bytes of its SHA-256, says in each part which key opens it.
//! - **The cipher** is AES-256-GCM with a random 96-bit nonce for each part. Its associated
//!   data names the key's organisation, where the message was sent and the part's place in
//!   it ([`Context`]), so a part moved to another channel or place fails to open.
//!
//! The randomness comes from whoever calls this, as the browser and the other apps each have
//! their own: `zirc_core::sealed` brings the system's.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use zirc_proto::sealed::{PLACEHOLDER, Part, Sealed, TAG, VERSION};

/// How long a key is, in bytes.
pub const KEY_LEN: usize = 32;

/// How long each part's nonce is, in bytes.
pub const NONCE_LEN: usize = 12;

/// What marks a part to seal, on both sides of it: `||secret||`.
pub const MARKER: &str = "||";

/// What a key's string starts with; the `1` is the format's version.
const PREFIX: &str = "zirc-key1-";

/// How long a key's base64url is: 32 bytes, unpadded.
const KEY_TEXT_LEN: usize = 43;

/// How long a key string's checksum is, in base64url characters: 24 bits, so a mistyped or
/// cut-off paste is caught.
const CHECK_LEN: usize = 4;

/// How many bytes of a key's SHA-256 its id takes.
const ID_LEN: usize = 8;

/// What the associated data starts with: the cipher's use and version.
const LABEL: &[u8] = b"zirc-sealed1";

/// An organisation's shared key. Its bytes are never shown: [`fmt::Debug`] gives only its
/// organisation and id, and only [`Key::export`] gives the string to hand on.
///
/// [`fmt::Debug`]: std::fmt::Debug
#[derive(Clone, PartialEq, Eq)]
pub struct Key {
    org: String,
    bytes: [u8; KEY_LEN],
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Key")
            .field("org", &self.org)
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

/// Why a string isn't a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// It doesn't start as one does.
    NotAKey,
    /// It starts as one does, but isn't whole or its checksum doesn't match: mistyped, or cut
    /// short.
    Mistyped,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotAKey => "that isn't a zirc organisation key, which starts zirc-key1-",
            Self::Mistyped => "that key's checksum doesn't match: it was mistyped or cut short",
        })
    }
}

impl std::error::Error for KeyError {}

impl Key {
    /// The key for `org` made of `bytes`, which should be random.
    #[must_use]
    pub fn new(org: impl Into<String>, bytes: [u8; KEY_LEN]) -> Self {
        Self {
            org: org.into(),
            bytes,
        }
    }

    /// The organisation it's for.
    #[must_use]
    pub fn org(&self) -> &str {
        &self.org
    }

    /// Its id, which each part sealed with it names.
    #[must_use]
    pub fn id(&self) -> String {
        URL_SAFE_NO_PAD.encode(&Sha256::digest(self.bytes)[..ID_LEN])
    }

    /// The string to hand to another device: `zirc-key1-<org>-<key>-<checksum>`.
    #[must_use]
    pub fn export(&self) -> String {
        let key = URL_SAFE_NO_PAD.encode(self.bytes);
        let check = checksum(&self.org, &key);
        format!("{PREFIX}{}-{key}-{check}", self.org)
    }

    /// The key a string from [`Key::export`] holds, spaces around it ignored.
    ///
    /// # Errors
    ///
    /// When it isn't one, or its checksum doesn't match.
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        let rest = text.trim().strip_prefix(PREFIX).ok_or(KeyError::NotAKey)?;
        // `<org>-<key>-<check>`: the key and checksum are of fixed lengths, and base64url
        // has `-` in it, so it's read from the end. Past the prefix, anything amiss is a
        // slip of the keyboard or a paste cut short.
        Self::parse_rest(rest).ok_or(KeyError::Mistyped)
    }

    fn parse_rest(rest: &str) -> Option<Self> {
        if !rest.is_ascii() || rest.len() < 2 + KEY_TEXT_LEN + 1 + CHECK_LEN {
            return None;
        }
        let (head, check) = rest.split_at(rest.len() - CHECK_LEN);
        let (org, key) = head
            .strip_suffix('-')?
            .split_at(head.len() - 1 - KEY_TEXT_LEN);
        let org = org.strip_suffix('-').filter(|org| !org.is_empty())?;
        if checksum(org, key) != check {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(key).ok()?;
        Some(Self::new(org, bytes.try_into().ok()?))
    }
}

fn checksum(org: &str, key: &str) -> String {
    let digest = Sha256::digest(format!("{PREFIX}{org}-{key}"));
    let mut check = URL_SAFE_NO_PAD.encode(digest);
    check.truncate(CHECK_LEN);
    check
}

/// Where a message was said, which its sealed parts are bound to, with the organisation of
/// the key that seals them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context<'a> {
    /// The channel or nick it was sent to, as its PRIVMSG names it.
    pub target: &'a str,
}

impl Context<'_> {
    /// The associated data for the part at `index`, sealed with `key`. IRC's names don't
    /// care about case, and none has a NUL in it. Not the sender: an app doesn't know the
    /// account the server tags its own lines with, and anyone holding the key can seal as
    /// anyone anyway.
    fn aad(&self, key: &Key, index: usize) -> Vec<u8> {
        let mut aad = LABEL.to_vec();
        for name in [key.org.as_str(), self.target] {
            aad.push(0);
            aad.extend(name.to_lowercase().bytes());
        }
        aad.push(0);
        aad.extend(index.to_string().bytes());
        aad
    }
}

/// A message's text with its parts to seal taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// The text, with a placeholder where each part was.
    pub text: String,
    /// The parts, in order.
    pub parts: Vec<String>,
}

/// Takes each `||…||` out of `text`, leaving the placeholder. A marker without its pair, or
/// with nothing between them, stays as it is, as do markers in code (`` `a || b` ``, or a
/// fenced block): shell's `||` is no secret.
#[must_use]
pub fn split(text: &str) -> Split {
    let mut split = Split {
        text: String::with_capacity(text.len()),
        parts: Vec::new(),
    };
    let mut rest = text;
    while let Some(start) = next_marker(rest) {
        let after = &rest[start + MARKER.len()..];
        let Some(end) = next_marker(after).filter(|&end| end > 0) else {
            break;
        };
        split.text.push_str(&rest[..start]);
        split.text.push_str(PLACEHOLDER);
        split.parts.push(after[..end].to_owned());
        rest = &after[end + MARKER.len()..];
    }
    split.text.push_str(rest);
    split
}

/// Where the next [`MARKER`] outside code starts. Code is between runs of as many
/// backticks, as Markdown has it; a run without its pair is only backticks.
fn next_marker(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'`' {
            let run = backticks(&bytes[at..]);
            at += run + closing(&bytes[at + run..], run).map_or(0, |end| end + run);
        } else if bytes[at..].starts_with(MARKER.as_bytes()) {
            return Some(at);
        } else {
            at += 1;
        }
    }
    None
}

fn backticks(bytes: &[u8]) -> usize {
    bytes.iter().take_while(|&&byte| byte == b'`').count()
}

/// Where a run of exactly `run` backticks starts in `bytes`.
fn closing(bytes: &[u8], run: usize) -> Option<usize> {
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'`' {
            let found = backticks(&bytes[at..]);
            if found == run {
                return Some(at);
            }
            at += found;
        } else {
            at += 1;
        }
    }
    None
}

/// Why parts couldn't be sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    /// Sealed, they'd take more than the message's client tags have room for.
    TooLong,
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLong => "Too long to seal",
        })
    }
}

impl std::error::Error for SealError {}

/// One part sealed with `key`, as the part at `index` of a message said in `context`.
#[must_use]
pub fn seal(
    key: &Key,
    context: &Context,
    index: usize,
    nonce: [u8; NONCE_LEN],
    part: &str,
) -> Part {
    let aad = context.aad(key, index);
    let sealed = Aes256Gcm::new(&key.bytes.into())
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: part.as_bytes(),
                aad: &aad,
            },
        )
        // AES-GCM refuses only what's longer than 64 GB.
        .unwrap_or_default();
    Part {
        k: key.id(),
        n: URL_SAFE_NO_PAD.encode(nonce),
        c: URL_SAFE_NO_PAD.encode(sealed),
    }
}

/// The value of the `+zirc.dev/sealed` tag for `parts`, each sealed with `key` and a nonce
/// from `nonce`, which must give a random one each time.
///
/// # Errors
///
/// [`SealError::TooLong`] when the tag, its name and `=` included, would take more than
/// `room` bytes: what's left of the message's client tags after its others.
pub fn seal_message(
    key: &Key,
    context: &Context,
    parts: &[String],
    mut nonce: impl FnMut() -> [u8; NONCE_LEN],
    room: usize,
) -> Result<String, SealError> {
    let sealed = Sealed {
        v: VERSION,
        parts: parts
            .iter()
            .enumerate()
            .map(|(index, part)| seal(key, context, index, nonce(), part))
            .collect(),
    };
    let value = sealed.to_tag();
    if TAG.len() + 1 + value.len() > room {
        return Err(SealError::TooLong);
    }
    Ok(value)
}

/// What a sealed part holds, for this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// Its value.
    Value(String),
    /// This device has no key with the id it names.
    NoKey,
    /// It doesn't open with the key it names: it was changed, or moved to another channel or
    /// place.
    Failed,
}

/// Opens one part, the one at `index` of a message said in `context`, with whichever of
/// `keys` it names.
#[must_use]
pub fn open(keys: &[Key], context: &Context, index: usize, part: &Part) -> Opened {
    let Some(key) = keys.iter().find(|key| key.id() == part.k) else {
        return Opened::NoKey;
    };
    let aad = context.aad(key, index);
    let opened = URL_SAFE_NO_PAD
        .decode(&part.n)
        .ok()
        .filter(|nonce| nonce.len() == NONCE_LEN)
        .zip(URL_SAFE_NO_PAD.decode(&part.c).ok())
        .and_then(|(nonce, sealed)| {
            Aes256Gcm::new(&key.bytes.into())
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &sealed,
                        aad: &aad,
                    },
                )
                .ok()
        })
        .and_then(|value| String::from_utf8(value).ok());
    opened.map_or(Opened::Failed, Opened::Value)
}

/// Each part a `+zirc.dev/sealed` tag's value holds, opened with `keys`, in order; `None`
/// when the value isn't sealed parts this knows.
#[must_use]
pub fn open_message(keys: &[Key], context: &Context, value: &str) -> Option<Vec<Opened>> {
    let sealed = Sealed::from_tag(value)?;
    Some(
        sealed
            .parts
            .iter()
            .enumerate()
            .map(|(index, part)| open(keys, context, index, part))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use zirc_proto::sealed::CLIENT_TAGS_MAX;

    use super::*;

    const ACME: Context = Context { target: "#api" };

    fn key(byte: u8) -> Key {
        Key::new("acme", [byte; KEY_LEN])
    }

    /// Nonces that differ, as random ones would.
    fn nonces() -> impl FnMut() -> [u8; NONCE_LEN] {
        let mut next = 0u8;
        move || {
            next += 1;
            [next; NONCE_LEN]
        }
    }

    fn sealed(parts: &[&str]) -> String {
        let parts: Vec<String> = parts.iter().map(|&part| part.to_owned()).collect();
        seal_message(&key(7), &ACME, &parts, nonces(), CLIENT_TAGS_MAX).unwrap()
    }

    #[test]
    fn takes_the_parts_between_markers_out_of_the_text() {
        assert_eq!(
            split("the key is ||sk_test_123|| and the url ||https://x||."),
            Split {
                text: "the key is [sealed] and the url [sealed].".into(),
                parts: vec!["sk_test_123".into(), "https://x".into()],
            }
        );
        // A lone marker, or two with nothing between, are only text.
        for text in ["a || b", "a |||| b", "no markers", "||x|| ||"] {
            let split = split(text);
            assert_eq!(split.parts.len(), usize::from(text.starts_with("||x")));
        }
    }

    #[test]
    fn leaves_markers_in_code_alone() {
        for text in [
            "run `make || true` then `exit || 1`",
            "```sh\ntest -f a || touch a\nls || true\n```",
            "``a `||` b || c``",
        ] {
            assert_eq!(split(text).parts, Vec::<String>::new(), "{text}");
        }
        assert_eq!(
            split("`a || b` and ||sk_1|| and `c || d`"),
            Split {
                text: "`a || b` and [sealed] and `c || d`".into(),
                parts: vec!["sk_1".into()],
            }
        );
        // A backtick without its pair is only a backtick.
        assert_eq!(split("it`s ||sk_1||").parts, ["sk_1"]);
    }

    #[test]
    fn opens_what_it_sealed() {
        let value = sealed(&["sk_test_123", "ünïcødé 🔑"]);
        assert_eq!(
            open_message(&[key(1), key(7)], &ACME, &value),
            Some(vec![
                Opened::Value("sk_test_123".into()),
                Opened::Value("ünïcødé 🔑".into()),
            ])
        );
        // Names are compared as IRC compares them, without case.
        let shouted = Context { target: "#API" };
        assert_eq!(
            open_message(&[key(7)], &shouted, &value).unwrap()[0],
            Opened::Value("sk_test_123".into())
        );
    }

    #[test]
    fn needs_the_key_it_names() {
        let value = sealed(&["sk_test_123"]);
        assert_eq!(
            open_message(&[key(1)], &ACME, &value),
            Some(vec![Opened::NoKey])
        );
        assert_eq!(open_message(&[key(7)], &ACME, "not sealed parts"), None);
    }

    #[test]
    fn wont_open_a_part_changed_or_moved() {
        let value = sealed(&["first", "second"]);
        let opened = |context: &Context, value: &str| open_message(&[key(7)], context, value);
        let elsewhere = Context { target: "#ops" };
        assert_eq!(
            opened(&elsewhere, &value),
            Some(vec![Opened::Failed, Opened::Failed])
        );
        // The same bytes as another organisation's key.
        let globex = Key::new("globex", [7; KEY_LEN]);
        assert_eq!(
            open_message(&[globex], &ACME, &value),
            Some(vec![Opened::Failed, Opened::Failed])
        );

        let mut parts = Sealed::from_tag(&value).unwrap();
        // Swapped places.
        parts.parts.swap(0, 1);
        assert_eq!(
            opened(&ACME, &parts.to_tag()),
            Some(vec![Opened::Failed, Opened::Failed])
        );
        // A byte of the ciphertext flipped.
        parts.parts.swap(0, 1);
        let mut ciphertext = URL_SAFE_NO_PAD.decode(&parts.parts[0].c).unwrap();
        ciphertext[0] ^= 1;
        parts.parts[0].c = URL_SAFE_NO_PAD.encode(ciphertext);
        assert_eq!(
            opened(&ACME, &parts.to_tag()),
            Some(vec![Opened::Failed, Opened::Value("second".into())])
        );
    }

    #[test]
    fn refuses_what_wont_fit_in_the_tags() {
        // About 3 KB of sealed text fits in a message's tags.
        let fits = "x".repeat(2950);
        assert!(sealed(&[&fits]).len() < CLIENT_TAGS_MAX);
        let too_long = vec!["x".repeat(3050)];
        assert_eq!(
            seal_message(&key(7), &ACME, &too_long, nonces(), CLIENT_TAGS_MAX),
            Err(SealError::TooLong)
        );
        // Room taken by the message's other tags counts.
        let small = vec!["sk_test_123".to_owned()];
        assert_eq!(
            seal_message(&key(7), &ACME, &small, nonces(), 50),
            Err(SealError::TooLong)
        );
    }

    #[test]
    fn hands_its_key_on_as_a_string() {
        let key = Key::new("acme-eu", [0xfb; KEY_LEN]);
        let text = key.export();
        assert!(text.starts_with("zirc-key1-acme-eu-"), "{text}");
        assert_eq!(Key::parse(&format!("  {text}\n")), Ok(key.clone()));
        // Its bytes never show in a log.
        assert!(!format!("{key:?}").contains(&text[18..30]), "{key:?}");
        assert_eq!(key.id().len(), 11);
    }

    #[test]
    fn refuses_a_mistyped_or_cut_off_key() {
        let text = key(7).export();
        let mut mistyped = text.clone().into_bytes();
        let middle = mistyped.len() - 20;
        mistyped[middle] = if mistyped[middle] == b'A' { b'B' } else { b'A' };
        let mistyped = String::from_utf8(mistyped).unwrap();
        assert_eq!(Key::parse(&mistyped), Err(KeyError::Mistyped));
        for cut in [&text[..text.len() - 1], &text[..30], "zirc-key1-"] {
            assert_eq!(Key::parse(cut), Err(KeyError::Mistyped), "{cut}");
        }
        for not_a_key in ["", "sk_test_123", &text[4..]] {
            assert_eq!(Key::parse(not_a_key), Err(KeyError::NotAKey), "{not_a_key}");
        }
    }
}
