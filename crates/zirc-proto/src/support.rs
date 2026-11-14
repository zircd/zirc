//! What staff do with a conversation with a guest, said in it as a `TAGMSG` to the guest's
//! nick with the [`TAG`] client tag: who opened it, who it's assigned to, that it's closed,
//! the email the guest left. Ergo keeps these tags in history, so every app folds the same
//! state from the conversation, with no store of its own. The guest's gateway lets only
//! [`Support::Joined`] through to the guest.

use std::fmt;

/// The client tag the events travel in.
pub const TAG: &str = "+zirc/support";

/// Something done with a conversation with a guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// The sender opened the conversation.
    Opened,
    /// The sender assigned it to this nick, who is notified of what the guest says.
    Assigned(String),
    /// The sender closed it; what the guest says next opens it again.
    Closed,
    /// The guest left this email, confirmed, for replies while they're away.
    Email(String),
    /// What the guest is told of an assignment: this nick joined.
    Joined(String),
}

impl Support {
    /// The event a tag's value says, or `None` for one this doesn't know.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let (event, about) = value.split_once(':').unwrap_or((value, ""));
        let named = || Some(about.to_owned()).filter(|about| !about.is_empty());
        match event {
            "opened" if about.is_empty() => Some(Self::Opened),
            "closed" if about.is_empty() => Some(Self::Closed),
            "assigned" => named().map(Self::Assigned),
            "email" => named().map(Self::Email),
            "joined" => named().map(Self::Joined),
            _ => None,
        }
    }

    /// Whether the gateway passes it on to the guest.
    #[must_use]
    pub fn for_guest(&self) -> bool {
        matches!(self, Self::Joined(_))
    }
}

/// The tag's value: `opened`, `assigned:bob`, `closed`, `email:anna@example.com`,
/// `joined:bob`.
impl fmt::Display for Support {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opened => f.write_str("opened"),
            Self::Assigned(nick) => write!(f, "assigned:{nick}"),
            Self::Closed => f.write_str("closed"),
            Self::Email(email) => write!(f, "email:{email}"),
            Self::Joined(nick) => write!(f, "joined:{nick}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_what_it_writes() {
        for event in [
            Support::Opened,
            Support::Assigned("bob".into()),
            Support::Closed,
            Support::Email("anna@example.com".into()),
            Support::Joined("bob".into()),
        ] {
            assert_eq!(Support::parse(&event.to_string()), Some(event));
        }
    }

    #[test]
    fn knows_nothing_of_what_it_doesnt_write() {
        for value in [
            "",
            "reopened",
            "assigned",
            "assigned:",
            "opened:bob",
            "closed:x",
        ] {
            assert_eq!(Support::parse(value), None, "{value}");
        }
    }

    #[test]
    fn passes_only_joins_to_the_guest() {
        assert!(Support::Joined("bob".into()).for_guest());
        assert!(!Support::Assigned("bob".into()).for_guest());
        assert!(!Support::Closed.for_guest());
    }
}
