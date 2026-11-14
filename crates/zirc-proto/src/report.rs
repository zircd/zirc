//! Reporting and blocking, for the apps' store reviews and for people's safety: anyone can
//! report a message or a person to the organisation's owners and to zirc, and block a person
//! so their messages, direct messages and pushes don't reach them.
//!
//! - `POST /api/organisations/<slug>/reports` with a [`Report`], as the signed-in member or
//!   guest, files it: `201 {"id": …}`. Owners and zirc are told; the reported text isn't
//!   sent, only where it is: the owners read it on their dashboard, as the organisation's
//!   Visibility allows them.
//! - `GET /api/organisations/<slug>/blocks` answers the account's [`BlockList`] there.
//! - `PUT` and `DELETE /api/organisations/<slug>/blocks/<account>` block and unblock one.
//!
//! The blocklist belongs to the person's account, so it follows them to every device. An
//! account name only means something inside one organisation, so entries are per
//! organisation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The most characters a report's note has.
pub const NOTE_MAX_CHARS: usize = 500;

/// Why something is reported: a small fixed list, in the order the apps show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "ReportReason")]
pub enum Reason {
    /// Harassment or bullying.
    Harassment,
    /// Hate speech.
    Hate,
    /// Threats of violence.
    Violence,
    /// Sexual content, and above all anything involving minors.
    Sexual,
    /// Illegal goods or activity.
    Illegal,
    /// Spam or scams.
    Spam,
    /// Anything else, which the note says.
    Other,
}

impl Reason {
    /// Every reason, in the order the apps list them.
    pub const ALL: [Self; 7] = [
        Self::Harassment,
        Self::Hate,
        Self::Violence,
        Self::Sexual,
        Self::Illegal,
        Self::Spam,
        Self::Other,
    ];

    /// What it's called in JSON and in the apps' calls, as `harassment`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Harassment => "harassment",
            Self::Hate => "hate",
            Self::Violence => "violence",
            Self::Sexual => "sexual",
            Self::Illegal => "illegal",
            Self::Spam => "spam",
            Self::Other => "other",
        }
    }

    /// What the apps show for it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Harassment => "Harassment or bullying",
            Self::Hate => "Hate speech",
            Self::Violence => "Threats of violence",
            Self::Sexual => "Sexual content, or anything involving minors",
            Self::Illegal => "Illegal goods or activity",
            Self::Spam => "Spam or scams",
            Self::Other => "Something else",
        }
    }

    /// The reason called `id`, if there is one.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.id() == id)
    }
}

/// What is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "ReportKind")]
pub enum Kind {
    /// One message, by its id.
    Message,
    /// A person, by their account.
    Person,
}

/// A report, as an app sends it to [`reports_path`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "Report")]
pub struct Report {
    /// Whether it's a message or a person.
    pub kind: Kind,
    /// Why.
    pub reason: Reason,
    /// The reported person's account name in the organisation.
    pub account: String,
    /// For a message, the channel it's in, or for a direct message the other person's
    /// nick. Never its text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<String>", optional)]
    pub channel: Option<String>,
    /// For a message, the server's id for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<String>", optional)]
    pub message: Option<String>,
    /// What the person adds, at most [`NOTE_MAX_CHARS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<String>", optional)]
    pub note: Option<String>,
}

/// What zirc.dev answers a filed report with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "ReportFiled")]
pub struct Filed {
    /// The report's id, for quoting to the owners.
    pub id: String,
}

/// A note as a person typed it, trimmed, none when empty, or why it won't do.
///
/// # Errors
///
/// When it's longer than [`NOTE_MAX_CHARS`] or has control characters other than newlines.
pub fn note(text: Option<&str>) -> Result<Option<String>, String> {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > NOTE_MAX_CHARS || text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!(
            "A note is at most {NOTE_MAX_CHARS} characters, without control characters."
        ));
    }
    Ok(Some(text.to_owned()))
}

/// Where reports are filed, in organisation `org`.
#[must_use]
pub fn reports_path(org: &str) -> String {
    format!("{}/reports", crate::irc::organisation_path(org))
}

/// Where the account's blocklist in organisation `org` is.
#[must_use]
pub fn blocks_path(org: &str) -> String {
    format!("{}/blocks", crate::irc::organisation_path(org))
}

/// Where the account blocks, or unblocks, `account` in organisation `org`.
#[must_use]
pub fn block_path(org: &str, account: &str) -> String {
    format!("{}/{account}", blocks_path(org))
}

/// One person blocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "BlockedPerson")]
pub struct Blocked {
    /// Their account name in the organisation, which is the nick they speak as.
    pub account: String,
    /// The name they show as, when they've set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<String>", optional)]
    pub name: Option<String>,
}

/// An account's blocklist in one organisation, as [`blocks_path`] answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "BlockList")]
pub struct BlockList {
    /// Whom the account blocks.
    pub blocked: Vec<Blocked>,
}

/// The words of the report and block screens, one set for every app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(rename = "SafetyWords")]
pub struct Words {
    /// The menu item and the sheet's title for reporting a message.
    pub report_message: String,
    /// The menu item and the sheet's title for reporting a person.
    pub report_person: String,
    /// Above the list of reasons.
    pub reasons_title: String,
    /// The note field's placeholder.
    pub note_hint: String,
    /// The button that sends the report.
    pub send: String,
    /// What to say once it's sent.
    pub sent: String,
    /// The menu item that blocks a person.
    pub block: String,
    /// The menu item that unblocks them.
    pub unblock: String,
    /// The Blocked people screen's title.
    pub blocked_title: String,
    /// What it says when nobody's blocked.
    pub blocked_empty: String,
}

/// The words the apps show.
#[must_use]
pub fn words() -> Words {
    Words {
        report_message: "Report message".into(),
        report_person: "Report".into(),
        reasons_title: "Why are you reporting this?".into(),
        note_hint: "Add a note (optional)".into(),
        send: "Send report".into(),
        sent: "Thanks. Your organisation's owners and Zirc have it, and reports are acted on within 24 hours.".into(),
        block: "Block".into(),
        unblock: "Unblock".into(),
        blocked_title: "Blocked people".into(),
        blocked_empty: "You haven't blocked anyone. People you block are listed here, and can be unblocked.".into(),
    }
}

/// The question before blocking `nick`.
#[must_use]
pub fn block_confirm(nick: &str) -> String {
    format!(
        "Block {nick}? You won't see their messages or direct messages, or get notifications from them, on any of your devices. They aren't told."
    )
}

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Reason::decl(config),
        format!(
            "const REPORT_REASONS: readonly ReportReason[] = [{}];",
            Reason::ALL
                .map(|reason| format!("{:?}", reason.id()))
                .join(", ")
        ),
        format!("const REPORT_NOTE_MAX_CHARS = {NOTE_MAX_CHARS};"),
        Kind::decl(config),
        Report::decl(config),
        Filed::decl(config),
        Blocked::decl(config),
        BlockList::decl(config),
        Words::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_have_ids_and_labels_and_round_trip() {
        for reason in Reason::ALL {
            assert_eq!(Reason::from_id(reason.id()), Some(reason));
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("{:?}", reason.id())
            );
            assert!(!reason.label().is_empty());
        }
        assert_eq!(Reason::from_id("nonsense"), None);
    }

    #[test]
    fn a_report_leaves_out_what_it_doesnt_have() {
        let report = Report {
            kind: Kind::Person,
            reason: Reason::Hate,
            account: "mallory".into(),
            channel: None,
            message: None,
            note: None,
        };
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            r#"{"kind":"person","reason":"hate","account":"mallory"}"#
        );
    }

    #[test]
    fn notes_are_trimmed_and_bounded() {
        assert_eq!(note(Some("  rude \n")), Ok(Some("rude".into())));
        assert_eq!(note(Some("  ")), Ok(None));
        assert_eq!(note(None), Ok(None));
        assert!(note(Some(&"a".repeat(NOTE_MAX_CHARS + 1))).is_err());
        assert!(note(Some("bell\u{7}")).is_err());
    }

    #[test]
    fn paths_are_the_organisations() {
        assert_eq!(reports_path("acme"), "/api/organisations/acme/reports");
        assert_eq!(
            block_path("acme", "mallory"),
            "/api/organisations/acme/blocks/mallory"
        );
    }
}
