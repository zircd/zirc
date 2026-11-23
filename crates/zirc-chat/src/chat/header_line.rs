//! The line under a conversation's name in its header, decided once for every app.

use super::Chat;

/// What a conversation's header line is, for how an app draws it: an agent's at work after a
/// green dot, the rest muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum HeaderLineKind {
    /// An agent session's ([`Chat::agent_header`]), while it's at work.
    AgentAtWork,
    /// An agent session's, idle: when it was heard, and whose it is.
    Agent,
    /// A person who's away, in a direct message: `Away: lunch`; or an agent session away
    /// with messages waiting: `Not listening for 17 min · 1 message waiting`
    /// ([`Chat::not_listening`]), drawn in the warning colour.
    Away,
    /// A channel's people, threads and topic ([`Chat::channel_about`]).
    About,
    /// A conversation's topic alone, where there's nothing else to say.
    Topic,
}

/// The line under a conversation's name ([`Chat::header_line`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct HeaderLine {
    /// What it says.
    pub text: String,
    /// What it is.
    pub kind: HeaderLineKind,
}

impl Chat {
    /// The one line under the name of the conversation `name` in its header, the first of
    /// these there is, in every app (the user's choice, 17 Nov 2026): an agent session's, what
    /// it's doing or when it was heard; that the person in a direct message is away, `Away: lunch`; a
    /// channel's people, threads and topic; its topic alone. `None` when there's none.
    #[must_use]
    pub fn header_line(&self, name: &str) -> Option<HeaderLine> {
        let line = |text: String, kind| Some(HeaderLine { text, kind });
        if let Some(header) = self.agent_header(name) {
            let not_listening = super::agent(name)
                .is_some_and(|(nick, session)| self.not_listening(nick, session).is_some());
            let kind = if header.active {
                HeaderLineKind::AgentAtWork
            } else if not_listening {
                HeaderLineKind::Away
            } else {
                HeaderLineKind::Agent
            };
            return line(header.line, kind);
        }
        if !super::is_channel(name)
            && let Some(why) = self.away(name)
        {
            return line(super::away_title(why), HeaderLineKind::Away);
        }
        if let Some(about) = self.channel_about(name) {
            return line(about, HeaderLineKind::About);
        }
        let topic = self.conversation(name)?.topic.as_deref()?.trim();
        (!topic.is_empty()).then(|| HeaderLine {
            text: topic.to_owned(),
            kind: HeaderLineKind::Topic,
        })
    }
}
