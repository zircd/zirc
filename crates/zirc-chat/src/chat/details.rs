//! A message's details, as every app's "Message details" shows them: what the server and the
//! sender attached to it, beyond what it says. Labelled rows, then a zirc message's data as
//! JSON, pretty-printed, to read and copy. Every app shows them as they come: the rows and
//! their labels are decided here, never in an app.

use chrono::SecondsFormat;
use serde::Serialize;
use ts_rs::TS;

use super::{Chat, Entry, EntryKind};

/// A message's menu, and the title of what it opens: `Message details`.
pub const MESSAGE_DETAILS: &str = "Message details";

/// The heading over a zirc message's data, as JSON: `Data`.
pub const DATA: &str = "Data";

/// One line of a message's details: what it is, and its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct Detail {
    /// What it is, such as `Session`.
    pub label: String,
    /// Its value, as it was sent, such as `cc-1a2b3c`.
    pub value: String,
}

/// What "Message details" shows of one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Details {
    /// Its title: [`MESSAGE_DETAILS`].
    pub title: String,
    /// What's known of it, in order, only those it has.
    pub rows: Vec<Detail>,
    /// The heading over [`Details::data`]: [`DATA`].
    pub data_heading: String,
    /// A zirc message's data, as JSON, pretty-printed; `None` for any other message.
    pub data: Option<String>,
}

/// What kind of entry it is, as its details say: `message`, `action`, `notice`, a zirc
/// message's type (`text`, `push`, …), and so on.
fn kind(entry: &Entry) -> String {
    match &entry.kind {
        EntryKind::Message(_) => "message".to_owned(),
        EntryKind::Action(_) => "action".to_owned(),
        EntryKind::Notice(_) => "notice".to_owned(),
        EntryKind::Zirc(message) => serde_json::to_value(&message.event)
            .ok()
            .and_then(|event| event.get("type")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "zirc".to_owned()),
        EntryKind::Unreadable(_) => "unreadable".to_owned(),
        EntryKind::Join => "join".to_owned(),
        EntryKind::Part(_) => "part".to_owned(),
        EntryKind::Quit(_) => "quit".to_owned(),
        EntryKind::Kick { .. } => "kick".to_owned(),
        EntryKind::Nick(_) => "nick".to_owned(),
        EntryKind::Topic(_) => "topic".to_owned(),
        EntryKind::Info(_) => "info".to_owned(),
    }
}

/// `entry`'s details ([`Details`]).
#[must_use]
pub fn details(entry: &Entry) -> Details {
    let zirc = match &entry.kind {
        EntryKind::Zirc(message) => Some(message),
        _ => None,
    };
    let rows = [
        ("Id", entry.id.clone()),
        (
            "Time",
            Some(entry.time.to_rfc3339_opts(SecondsFormat::Millis, true)),
        ),
        ("Kind", Some(kind(entry))),
        ("From", Some(entry.from.clone())),
        ("Machine of", entry.owner.clone()),
        ("Session", zirc.map(|m| m.session.clone())),
        ("Session name", zirc.and_then(|m| m.name.clone())),
        ("Tool", zirc.and_then(|m| m.tool.clone())),
        ("App", zirc.and_then(|m| m.app.clone())),
        ("Task", zirc.and_then(|m| m.task.clone())),
        (
            "Level",
            zirc.and_then(zirc_proto::Message::level)
                .map(|l| l.as_str().to_owned()),
        ),
        ("Format", zirc.map(|m| m.v.to_string())),
        ("Sent from", entry.sent_from.clone()),
        ("Reply to", entry.reply_to.clone()),
        ("Edited", entry.edited.then(|| "yes".to_owned())),
        (
            "Sealed parts",
            entry
                .sealed
                .as_ref()
                .and_then(|sealed| zirc_proto::sealed::Sealed::from_tag(&sealed.value))
                .map(|sealed| sealed.parts.len().to_string()),
        ),
        ("From history", entry.history.then(|| "yes".to_owned())),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        value.filter(|v| !v.is_empty()).map(|value| Detail {
            label: label.to_owned(),
            value,
        })
    })
    .collect();
    Details {
        title: MESSAGE_DETAILS.to_owned(),
        rows,
        data_heading: DATA.to_owned(),
        data: zirc.and_then(|message| serde_json::to_string_pretty(message).ok()),
    }
}

impl Chat {
    /// The details of message `id` in `target` ([`details`]), or `None` when it isn't
    /// loaded there.
    #[must_use]
    pub fn details(&self, target: &str, id: &str) -> Option<Details> {
        self.conversation(target)?.entry(id).map(details)
    }
}
