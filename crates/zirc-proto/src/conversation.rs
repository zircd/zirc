//! One person's direct conversation with someone, as the office's server answers it whole for a
//! page that shows that one conversation and writes to it, such as zirc.dev's setup page saying
//! hi to a coworker (`GET` and `POST` [`PATH`] on the content API, zirc-sidecar's
//! `accounts::content`). The server reads it from its own history and sends what's written as
//! the person, so a page needs neither the web chat nor a connection to IRC, and what's said
//! never passes through zirc.dev.
//!
//! What a page draws is decided here: the entries, oldest first, each said by the person or by
//! whom they're talking to; the composer's words. An [`Entry`] is tagged by its `kind`, so
//! what else a conversation comes to hold (an incident's card, say) is an entry of its own,
//! beside what was said.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a conversation is, under the content API's `/api/v1`: `/conversations/<nick>`.
pub const PATH: &str = "/conversations/{nick}";

/// The most characters one message written on a page may be: the server sends it in as many
/// lines as it needs.
pub const MOST_CHARS: usize = 2000;

/// How many messages a conversation shows, the newest.
pub const SHOWN: usize = 50;

/// One person's direct conversation with someone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "OneConversation")]
pub struct Conversation {
    /// Whom it's with: their nick.
    pub with: String,
    /// Their name as people see it: a coworker's shown name, or the nick.
    pub shown: String,
    /// What the conversation is called, for a screen reader: [`label`].
    pub label: String,
    /// What's in it, oldest first: the newest [`SHOWN`].
    pub entries: Vec<Entry>,
    /// What the composer says while it's empty: `Message John`.
    pub placeholder: String,
    /// The button that sends: `Send`.
    pub send: String,
    /// What it says with nothing in it yet.
    pub empty: String,
}

/// One thing in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(rename = "ConversationEntry")]
pub enum Entry {
    /// A message.
    Said(Said),
}

/// A message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "ConversationSaid")]
pub struct Said {
    /// The server's id for it.
    pub id: String,
    /// When, RFC 3339.
    pub at: String,
    /// Whether the person said it, rather than whom they're talking to.
    pub mine: bool,
    /// Who said it, as people see them: `You` for the person.
    pub who: String,
    /// What it says.
    pub text: String,
}

/// What's written on a page, to send (`POST` [`PATH`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "ConversationWrite")]
pub struct Write {
    /// The words.
    pub text: String,
}

impl Write {
    /// The words to send, trimmed, or why they can't be.
    ///
    /// # Errors
    ///
    /// When there are none, or more than [`MOST_CHARS`].
    pub fn checked(&self) -> Result<&str, String> {
        let text = self.text.trim();
        if text.is_empty() {
            return Err("Write something first.".to_owned());
        }
        if text.chars().count() > MOST_CHARS {
            return Err(too_long());
        }
        Ok(text)
    }
}

/// Why a message is too long to send.
#[must_use]
pub fn too_long() -> String {
    format!("That's more than {MOST_CHARS} characters: send it in parts.")
}

/// What a conversation with `shown` is called, for a screen reader.
#[must_use]
pub fn label(shown: &str) -> String {
    format!("Conversation with {shown}")
}

/// What a conversation says with nothing in it yet.
#[must_use]
pub fn empty(shown: &str) -> String {
    format!("Nothing here yet. Say hi to {shown}.")
}

/// What a conversation's newest messages, newest first as the history gives them, amount to,
/// each as `said` reads it.
#[must_use]
pub fn entries<M>(newest_first: &[M], said: impl Fn(&M) -> Said) -> Vec<Entry> {
    newest_first
        .iter()
        .take(SHOWN)
        .rev()
        .map(|message| Entry::Said(said(message)))
        .collect()
}

/// Its types, for zirc.dev.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Said::decl(config),
        Entry::decl(config),
        Conversation::decl(config),
        Write::decl(config),
        format!("const CONVERSATION_MOST_CHARS = {MOST_CHARS};"),
        format!("const CONVERSATION_TOO_LONG = {:?};", too_long()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_is_trimmed_and_kept_to_its_length() {
        let write = |text: &str| Write {
            text: text.to_owned(),
        };
        assert_eq!(write("  hi John \n").checked(), Ok("hi John"));
        assert!(write(" \n ").checked().is_err());
        assert_eq!(
            write(&"a".repeat(MOST_CHARS + 1)).checked(),
            Err(too_long())
        );
        assert!(write(&"å".repeat(MOST_CHARS)).checked().is_ok());
    }

    #[test]
    fn entries_go_oldest_first_and_say_their_kind() {
        let said = |id: &str| Said {
            id: id.into(),
            at: "2026-11-27T10:00:00Z".into(),
            mine: id == "1",
            who: "John".into(),
            text: id.into(),
        };
        let shown = entries(&["2", "1"], |id| said(id));
        assert_eq!(shown, [Entry::Said(said("1")), Entry::Said(said("2"))]);
        let json = serde_json::to_value(&shown[0]).unwrap();
        assert_eq!(json["kind"], "said");
        assert_eq!(json["mine"], true);
    }
}
