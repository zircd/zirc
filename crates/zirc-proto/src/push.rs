//! What people's apps and the tenant's pusher say to each other. The pusher (the tenant
//! sidecar, nick [`PUSHER`]) decides what reaches a person's phone while its app is closed,
//! by the rules Activity lists by (zirc-chat's `push`), and sends it encrypted for the phone
//! (RFC 8291) to the endpoint zirc.dev made for it, which relays it unread.
//!
//! Each goes as a `TAGMSG` to the pusher, or from it to the person's nick, with one tag,
//! [`TAG`], whose value is a [`Said`] as JSON in base64: never kept in a history, never shown
//! to anyone else. Ergo vouches for who sent one (its `account` tag), so a phone registers
//! only for its own account, and an app says only its own person's reads.
//!
//! - A phone registers ([`Said::Register`]) on each connect and whenever how much its
//!   conversations notify changes, and the pusher answers [`Said::Registered`]; a phone
//!   unregisters ([`Said::Unregister`]) on signing out. A registration not renewed in
//!   [`EXPIRY_DAYS`] goes.
//! - Every app says what its person read ([`Said::Read`]) as it tells the server
//!   (`MARKREAD`), so a push waiting for something already read on another device isn't
//!   sent, and the phone's notification for one already sent goes.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

/// The nick the tenant's pusher goes by.
pub const PUSHER: &str = "zirc-pusher";

/// The tag a [`Said`] goes in.
pub const TAG: &str = "+zirc/push";

/// The tag on a pushed line saying why it was pushed ([`Why`]).
pub const WHY_TAG: &str = "+zirc/push-why";

/// The tag on a pushed reaction carrying the start of what it reacts to.
pub const QUOTE_TAG: &str = "+zirc/push-quote";

/// How long a registration lasts without being renewed, as Ergo's did.
pub const EXPIRY_DAYS: i64 = 14;

/// What an app and the pusher say to each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Said {
    /// Push to this phone, by these settings.
    Register {
        /// The endpoint zirc.dev made for the phone's push token.
        endpoint: String,
        /// The phone's public key, the uncompressed P-256 point, base64url.
        p256dh: String,
        /// Its auth secret, 16 bytes, base64url.
        auth: String,
        /// How much each conversation notifies on the phone: zirc-chat's `NotifySettings`.
        #[serde(default)]
        settings: serde_json::Value,
    },
    /// The pusher took a registration.
    Registered {
        /// Its endpoint.
        endpoint: String,
    },
    /// Stop pushing to this endpoint.
    Unregister {
        /// The endpoint.
        endpoint: String,
    },
    /// The person read a conversation up to a time.
    Read {
        /// The channel, or the other nick.
        target: String,
        /// Up to when, RFC 3339.
        at: String,
    },
}

impl Said {
    /// What goes in [`TAG`].
    ///
    /// # Panics
    ///
    /// Never: each serializes to JSON.
    #[must_use]
    pub fn tag(&self) -> String {
        BASE64.encode(serde_json::to_vec(self).expect("a push message serializes"))
    }

    /// What a [`TAG`]'s value says, if it's well formed.
    #[must_use]
    pub fn of(tag: &str) -> Option<Self> {
        serde_json::from_slice(&BASE64.decode(tag).ok()?).ok()
    }
}

/// Why something was pushed, as [`WHY_TAG`] says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Why {
    /// A direct message to the person.
    Direct,
    /// A channel message naming them.
    Mention,
    /// An answer to one of their messages.
    Reply,
    /// A reaction to one of their messages.
    Reaction,
    /// A message in a conversation they set to notify everything.
    All,
    /// An agent's question to them, open ([`crate::request`]): it waits on their answer.
    Question,
}

impl Why {
    /// As [`WHY_TAG`] says it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Mention => "mention",
            Self::Reply => "reply",
            Self::Reaction => "reaction",
            Self::All => "all",
            Self::Question => "question",
        }
    }

    /// From [`WHY_TAG`]'s value.
    #[must_use]
    pub fn of(tag: &str) -> Option<Self> {
        [
            Self::Direct,
            Self::Mention,
            Self::Reply,
            Self::Reaction,
            Self::All,
            Self::Question,
        ]
        .into_iter()
        .find(|why| why.as_str() == tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_and_reads_back() {
        let said = Said::Register {
            endpoint: "https://zirc.dev/api/push/abc".into(),
            p256dh: "BP".into(),
            auth: "au".into(),
            settings: serde_json::json!({"levels": {"#api": "all"}}),
        };
        assert_eq!(Said::of(&said.tag()), Some(said));
        assert_eq!(Said::of("not base64!"), None);
        assert_eq!(Why::of(Why::Reaction.as_str()), Some(Why::Reaction));
    }

    #[test]
    fn its_nick_is_reserved() {
        assert!(crate::names::RESERVED_INTEGRATION_NICKS.contains(&PUSHER));
    }
}
