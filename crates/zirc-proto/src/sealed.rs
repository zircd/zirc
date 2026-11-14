//! Sealed parts of a message: pieces only devices holding the organisation's shared key can
//! read.
//!
//! A message's text keeps a [`PLACEHOLDER`] where each part was, so the server, its history
//! and search, push and plain IRC clients show `the key is [sealed]`. The parts themselves
//! travel in the message's client tag [`TAG`], whose value is a [`Sealed`]'s compact JSON,
//! one [`Part`] per placeholder, in order. Its strings are all base64url, so the JSON has no
//! space, `;` or `\\` that a tag's value would have to escape.
//!
//! No cryptography here (the web client runs this crate as wasm): `zirc_chat::sealed` seals
//! and opens parts.

use serde::{Deserialize, Serialize};

/// The client tag sealed parts travel in, named as `+zirc.dev/client` is.
pub const TAG: &str = "+zirc.dev/sealed";

/// What a message's text says where a part was sealed.
pub const PLACEHOLDER: &str = "[sealed]";

/// The format's version, [`Sealed::v`].
pub const VERSION: u8 = 1;

/// How long a message's client tags may be in all, their names, `=`s and `;`s included, as
/// IRCv3's message-tags allows; Ergo keeps that much in history.
pub const CLIENT_TAGS_MAX: usize = 4094;

/// A message's sealed parts, as its [`TAG`] holds them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Sealed {
    /// The format's version, [`VERSION`].
    pub v: u8,
    /// One for each placeholder in the text, in order.
    pub parts: Vec<Part>,
}

/// One sealed part.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// The id of the key it's sealed with, base64url: which key a device needs to open it.
    pub k: String,
    /// The nonce, base64url of 12 bytes.
    pub n: String,
    /// The ciphertext and its authentication tag, base64url.
    pub c: String,
}

impl Sealed {
    /// The [`TAG`]'s value for these parts.
    #[must_use]
    pub fn to_tag(&self) -> String {
        // A struct of strings and a number always serializes.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The parts a [`TAG`]'s value holds; `None` for one that isn't, or is of a version this
    /// doesn't know.
    #[must_use]
    pub fn from_tag(value: &str) -> Option<Self> {
        let sealed: Self = serde_json::from_str(value).ok()?;
        (sealed.v == VERSION).then_some(sealed)
    }

    /// How many bytes of a message's client tags this takes: the tag's name, `=` and value.
    #[must_use]
    pub fn tag_len(&self) -> usize {
        TAG.len() + 1 + self.to_tag().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed() -> Sealed {
        Sealed {
            v: VERSION,
            parts: vec![Part {
                k: "a2V5aWQx".into(),
                n: "bm9uY2Vub25jZTEy".into(),
                c: "Y2lwaGVy".into(),
            }],
        }
    }

    #[test]
    fn goes_through_its_tag_and_back() {
        let tag = sealed().to_tag();
        assert_eq!(
            tag,
            r#"{"v":1,"parts":[{"k":"a2V5aWQx","n":"bm9uY2Vub25jZTEy","c":"Y2lwaGVy"}]}"#
        );
        assert_eq!(Sealed::from_tag(&tag), Some(sealed()));
        assert_eq!(sealed().tag_len(), TAG.len() + 1 + tag.len());
    }

    #[test]
    fn refuses_what_isnt_a_known_version() {
        assert_eq!(Sealed::from_tag("not json"), None);
        assert_eq!(Sealed::from_tag("{}"), None);
        let later = Sealed { v: 2, ..sealed() };
        assert_eq!(Sealed::from_tag(&later.to_tag()), None);
    }
}
