//! People the person blocked: what they say isn't shown, counted, searched or notified,
//! in channels and in direct messages, and a direct message conversation with them isn't
//! listed. Nothing is deleted: unblocking brings back what they said. The list is the
//! account's, kept at zirc.dev ([`zirc_proto::report`]) and given here on connecting.

use zirc_proto::report::{Kind, Reason, Report};

use super::{Chat, fold};

/// What's being reported: a message by its id, or a person by their nick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reported {
    /// The message with this id.
    Message(String),
    /// The person with this nick.
    Person(String),
}

/// What the report dialog says under its title.
pub const REPORT_EXPLAINED: &str = "The organisation's owners and zirc are told who and why, \
    and act on it within 24 hours. They aren't told it was you.";

/// What the list of people blocked says over it: what blocking does.
pub const BLOCKING_EXPLAINED: &str = "You don't see what they say, in channels or direct \
    messages, nor hear about it. They aren't told. Block someone from their card, where you \
    can also report them.";

/// Why a report can't be filed, as every app says it.
pub const REPORT_NO_REASON: &str = "Pick a reason for the report.";
/// The message to report is no longer here.
pub const REPORT_GONE: &str = "That message is gone.";
/// The message is ours, our agent's or a system line.
pub const REPORT_NOT_REPORTABLE: &str = "That message can't be reported.";
/// The person is us, or our own agent.
pub const REPORT_SELF: &str = "You can't report yourself.";

impl Chat {
    /// Sets whom the person blocks to `nicks`, as zirc.dev's list says: what the newly
    /// blocked said is hidden, and what those no longer blocked said is shown again.
    pub fn set_blocked<I, S>(&mut self, nicks: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let blocked: std::collections::BTreeSet<String> = nicks
            .into_iter()
            .map(|nick| fold(nick.as_ref()))
            .filter(|nick| !nick.is_empty() && *nick != fold(&self.me))
            .collect();
        if blocked == self.blocked {
            return;
        }
        self.blocked = blocked;
        let max = self.options.max_entries;
        for conversation in &mut self.conversations {
            conversation.unhide(&self.blocked, max);
            conversation.hide_from(&self.blocked, &self.me);
        }
    }

    /// Whether `nick` is blocked.
    #[must_use]
    pub fn is_blocked(&self, nick: &str) -> bool {
        !self.blocked.is_empty() && self.blocked.contains(&fold(nick))
    }

    /// Whom the person blocks, as folded nicks, in order.
    #[must_use]
    pub fn blocked(&self) -> Vec<String> {
        self.blocked.iter().cloned().collect()
    }

    /// Where the message with id `id` is, and who said it: the conversation's name (a
    /// channel, or the other person's nick) and the speaker's nick. For reporting it.
    #[must_use]
    pub fn message_origin(&self, id: &str) -> Option<(String, String)> {
        self.conversations.iter().find_map(|conversation| {
            conversation
                .entries
                .iter()
                .find(|entry| entry.id.as_deref() == Some(id) && entry.is_said())
                .map(|entry| (conversation.name.clone(), entry.from.clone()))
        })
    }

    /// Whether the message with id `id` can be reported: someone else's words, or their
    /// agent's (zirc.dev files it against the agent's owner), not ours or our own agents'
    /// and not a system line.
    #[must_use]
    pub fn can_report(&self, id: &str) -> bool {
        self.message_origin(id)
            .is_some_and(|(_, from)| self.can_report_person(&from))
    }

    /// Whether `nick` can be reported and blocked: anyone but us and our own agents
    /// (`<us>-<machine>`).
    #[must_use]
    pub fn can_report_person(&self, nick: &str) -> bool {
        let (nick, me) = (fold(nick), fold(&self.me));
        !nick.trim().is_empty() && nick != me && !nick.starts_with(&format!("{me}-"))
    }

    /// The report dialog's title for `what`.
    #[must_use]
    pub fn report_title(&self, what: &Reported) -> String {
        match what {
            Reported::Message(id) => match self.message_origin(id) {
                Some((_, from)) => format!("Report a message from {}", self.shown_name(&from)),
                None => "Report a message".to_owned(),
            },
            Reported::Person(nick) => format!("Report {}", self.shown_name(nick)),
        }
    }

    /// The report of `what` for `reason`, with the note `typed`, for zirc.dev
    /// ([`zirc_proto::report`]). Only ids go: never a message's text.
    ///
    /// # Errors
    ///
    /// When no reason is picked, the note is too long, the message is gone, or `what` is us,
    /// our agent's or a system line: in words to show.
    pub fn report(
        &self,
        what: &Reported,
        reason: Option<Reason>,
        typed: &str,
    ) -> Result<Report, String> {
        let reason = reason.ok_or(REPORT_NO_REASON)?;
        let note = zirc_proto::report::note(Some(typed))?;
        Ok(match what {
            Reported::Message(id) => {
                let (channel, from) = self.message_origin(id).ok_or(REPORT_GONE)?;
                if !self.can_report_person(&from) {
                    return Err(REPORT_NOT_REPORTABLE.to_owned());
                }
                Report {
                    kind: Kind::Message,
                    reason,
                    account: from,
                    channel: Some(channel),
                    message: Some(id.clone()),
                    note,
                }
            }
            Reported::Person(nick) => {
                if !self.can_report_person(nick) {
                    return Err(REPORT_SELF.to_owned());
                }
                Report {
                    kind: Kind::Person,
                    reason,
                    account: nick.clone(),
                    channel: None,
                    message: None,
                    note,
                }
            }
        })
    }
}
