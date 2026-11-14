//! Trimming a message: removing it from the server's history, with `IRCv3`'s
//! `draft/message-redaction`. A client asks with `REDACT <target> <msgid> [:<reason>]`. The
//! server either removes it and says so, with the same `REDACT` line from whoever asked, to
//! every client in the conversation that took the capability, the asker's included; or
//! answers the asker `FAIL REDACT <code> <target> [<msgid>] :<why>`.
//!
//! Ergo offers the capability only when its `history.retention.allow-individual-delete` is
//! on. Then a channel's operators may trim any message in it, and anyone else only what
//! their own account sent: someone not logged in to an account can't trim anything.
//!
//! What it reaches: the server's history, and the clients that take the capability, zirc's
//! among them, which drop the message. A plain IRC client that already showed it keeps its
//! copy, and whatever anyone copied stays.

use zirc_line::Line;

/// The capability.
pub const CAP: &str = "draft/message-redaction";

/// Where a trimmed message goes from, as every app and `zirc trim`'s help say it: "Removes it
/// from the server's history and from Zirc's apps."
#[must_use]
pub fn reach() -> String {
    format!(
        "Removes it from the server's history and from {}'s apps.",
        zirc_proto::NAME
    )
}

/// Where it stays, as they say it.
pub const KEPT: &str = "IRC clients that already showed it keep their copy.";

/// What every app asks before deleting a message, over its Delete and Cancel buttons:
/// where it goes from, and where it stays.
#[must_use]
pub fn prompt() -> String {
    format!("{} {KEPT}", reach())
}

/// How long to wait for the server to say whether a message was trimmed.
pub const WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// The line that asks the server to trim message `id` from `target`, a channel or a nick,
/// saying why if `reason` does.
#[must_use]
pub fn request(target: &str, id: &str, reason: Option<&str>) -> Line {
    match reason.filter(|reason| !reason.is_empty()) {
        Some(reason) => Line::new("REDACT", [target, id, reason]),
        None => Line::new("REDACT", [target, id]),
    }
}

/// What the server said about a trim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `by` trimmed message `id` from `target`.
    Trimmed {
        /// The channel or nick it was trimmed from, as the asker named it.
        target: String,
        /// The message.
        id: String,
        /// The nick that trimmed it.
        by: String,
    },
    /// The server refused to trim a message from `target`.
    Refused {
        /// The channel or nick.
        target: String,
        /// The message, when the server said which.
        id: Option<String>,
        /// Why, as a code: `REDACT_FORBIDDEN`, `UNKNOWN_MSGID` or `INVALID_TARGET`.
        code: String,
        /// Why, for a person.
        text: String,
    },
}

impl Outcome {
    /// Whether it's about message `id` in `target`. A refusal that doesn't say which message
    /// is about any in `target`.
    #[must_use]
    pub fn is_about(&self, target: &str, id: &str) -> bool {
        match self {
            Self::Trimmed {
                target: t, id: i, ..
            } => t.eq_ignore_ascii_case(target) && i == id,
            Self::Refused {
                target: t, id: i, ..
            } => t.eq_ignore_ascii_case(target) && i.as_deref().is_none_or(|i| i == id),
        }
    }
}

/// What `line` says about a trim, if it's about one.
#[must_use]
pub fn outcome(line: &Line) -> Option<Outcome> {
    let param = |i: usize| line.params.get(i).cloned().unwrap_or_default();
    match line.command.as_str() {
        "REDACT" if line.params.len() >= 2 => {
            let source = line.source.as_deref().unwrap_or_default();
            Some(Outcome::Trimmed {
                target: param(0),
                id: param(1),
                by: source
                    .split_once('!')
                    .map_or(source, |(nick, _)| nick)
                    .to_owned(),
            })
        }
        // FAIL REDACT <code> <target> [<msgid>] :<text>
        "FAIL" if param(0) == "REDACT" && line.params.len() >= 4 => Some(Outcome::Refused {
            target: param(2),
            id: (line.params.len() >= 5).then(|| param(3)),
            code: param(1),
            text: line.params.last().cloned().unwrap_or_default(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_where_a_trimmed_message_goes_and_stays() {
        assert_eq!(
            reach(),
            "Removes it from the server's history and from Zirc's apps."
        );
        assert!(KEPT.ends_with("keep their copy."));
        assert_eq!(prompt(), format!("{} {KEPT}", reach()));
    }

    fn parsed(line: &str) -> Option<Outcome> {
        outcome(&line.parse().unwrap())
    }

    #[test]
    fn asks_with_a_reason_only_when_there_is_one() {
        assert_eq!(request("#api", "m1", None).to_string(), "REDACT #api m1");
        assert_eq!(
            request("#api", "m1", Some("")).to_string(),
            "REDACT #api m1"
        );
        assert_eq!(
            request("#api", "m1", Some("a secret")).to_string(),
            "REDACT #api m1 :a secret"
        );
    }

    #[test]
    fn reads_what_the_server_says() {
        let trimmed = parsed(":alice!u@h REDACT #api m1 :a secret").unwrap();
        assert_eq!(
            trimmed,
            Outcome::Trimmed {
                target: "#api".into(),
                id: "m1".into(),
                by: "alice".into(),
            }
        );
        assert!(trimmed.is_about("#API", "m1") && !trimmed.is_about("#api", "m2"));

        let refused =
            parsed(":s FAIL REDACT REDACT_FORBIDDEN #api m1 :You are not authorized").unwrap();
        assert_eq!(
            refused,
            Outcome::Refused {
                target: "#api".into(),
                id: Some("m1".into()),
                code: "REDACT_FORBIDDEN".into(),
                text: "You are not authorized".into(),
            }
        );
        let unnamed = parsed(":s FAIL REDACT INVALID_TARGET #nope :No such channel").unwrap();
        assert!(unnamed.is_about("#nope", "anything"));
        assert_eq!(parsed(":s FAIL CHATHISTORY INVALID_TARGET #api :no"), None);
        assert_eq!(parsed(":alice!u@h PRIVMSG #api :hi"), None);
    }
}
