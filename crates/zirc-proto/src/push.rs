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
//!
//! What zirc.dev's relay hands the phone beside the push's alert ([`PAYLOAD_KEY`],
//! [`ENDPOINT_KEY`], [`PUSH_ID_KEY`]), and what the phone says back when one arrives
//! ([`Arrived`], to its endpoint's [`ARRIVED_PATH`]), are declared here once: the relay and
//! its route read them as TypeScript, the apps' core as Rust.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The key of a push's encrypted line, base64, in what the push service hands the app.
pub const PAYLOAD_KEY: &str = "zirc";

/// The key of the id of the endpoint a push came to, which the app maps to its organisation.
pub const ENDPOINT_KEY: &str = "ep";

/// The key of a push's own random id, on a push that notifies: what the phone says arrived.
pub const PUSH_ID_KEY: &str = "pid";

/// Where under its endpoint's address a phone says a push arrived, sending an [`Arrived`] there.
/// zirc.dev's route lives at this path (`app/api/push/[id]/arrived`), as Next.js routes by
/// directory.
pub const ARRIVED_PATH: &str = "/arrived";

/// A phone saying a push arrived: its id ([`PUSH_ID_KEY`]), and when, in milliseconds since
/// the epoch by the phone's clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Arrived {
    /// The push's id.
    pub pid: String,
    /// When it arrived, by the phone's clock.
    pub at: u64,
}

/// The push contract's types and constants as TypeScript, for zirc.dev's relay.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Arrived::decl(config),
        format!("const PUSH_PAYLOAD_KEY = {PAYLOAD_KEY:?};"),
        format!("const PUSH_ENDPOINT_KEY = {ENDPOINT_KEY:?};"),
        format!("const PUSH_ID_KEY = {PUSH_ID_KEY:?};"),
        format!("const PUSH_ARRIVED_PATH = {ARRIVED_PATH:?};"),
    ]
}

/// The nick the tenant's pusher goes by.
pub const PUSHER: &str = "zirc-pusher";

/// The tag a [`Said`] goes in.
pub const TAG: &str = "+zirc/push";

/// The tag on a pushed line saying why it was pushed ([`Why`]).
pub const WHY_TAG: &str = "+zirc/push-why";

/// The tag on a pushed reaction carrying the start of what it reacts to.
pub const QUOTE_TAG: &str = "+zirc/push-quote";

/// The most bytes of a pushed line. The push services take 4096 bytes a push (APNs' payload,
/// FCM's data), and zirc.dev's relay sends the line encrypted (RFC 8291's header and tag),
/// in base64, beside the endpoint's id and the alert shown if the phone can't read it: a
/// longer line went without what was said, as a bare "New message", or wasn't sent at all.
pub const LINE_BYTES: usize = 2600;

/// The most characters of what was said in a line shortened to fit ([`fitted`]): more than
/// a notification shows.
const FITTED_CHARS: usize = 400;

/// `body`, what a pushed line says, as fits beside the `rest` of the line, in bytes: itself
/// when it does. Otherwise how it starts: a zirc message without its event's data, which a
/// notification doesn't show, and its words cut short, or plain words cut short. The app
/// reads it whole on opening.
#[must_use]
pub fn fitted(body: &str, rest: usize) -> String {
    let room = LINE_BYTES.saturating_sub(rest);
    if body.len() <= room {
        return body.to_owned();
    }
    let start = |words: &str| -> String {
        let mut cut: String = words.chars().take(FITTED_CHARS).collect();
        if cut.len() < words.len() {
            cut.push('…');
        }
        cut
    };
    let Ok(Some(mut message)) = crate::decode(body) else {
        return start(body);
    };
    message.event = match message.event {
        crate::Event::Custom { kind, text, .. } => crate::Event::Custom {
            kind,
            data: serde_json::Value::Null,
            text: start(&text),
        },
        event => crate::Event::Text {
            body: start(&event.human_text()),
        },
    };
    crate::encode_up_to(&message, room).unwrap_or_else(|_| start(&message.event.human_text()))
}

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

#[cfg(test)]
mod fitted_tests {
    use super::*;

    #[test]
    fn a_line_that_fits_goes_as_it_is() {
        assert_eq!(fitted("lunch?", 200), "lunch?");
    }

    #[test]
    fn long_words_are_cut_short() {
        let cut = fitted(&"word ".repeat(1000), 200);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(cut.len() + 200 <= LINE_BYTES, "{}", cut.len());
    }

    #[test]
    fn a_long_request_keeps_how_its_question_starts_and_loses_its_data() {
        let question = format!(
            "Drop the old sessions table{}?",
            ", and its index".repeat(80)
        );
        let asked = crate::request::Request::open("mbp/1", "cc-1", "alice", question, "yes", "no");
        let message = crate::Message::new("cc-1", asked.event());
        let body = crate::encode_up_to(&message, 16_384).unwrap();
        assert!(body.len() > LINE_BYTES, "{}", body.len());

        let fits = fitted(&body, 200);
        assert!(fits.len() + 200 <= LINE_BYTES, "{}", fits.len());
        let read = crate::decode(&fits).unwrap().expect("still a zirc message");
        assert_eq!(read.session, "cc-1");
        assert!(
            read.event
                .human_text()
                .contains("Drop the old sessions table"),
            "{:?}",
            read.event
        );
    }
}
