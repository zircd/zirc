//! Who is typing, as IRCv3's `+typing` client tag says (`TAGMSG <target>` with
//! `+typing=active`, `paused` or `done`): someone typing is shown for [`EXPIRES`] after
//! they last said so, until they say they're done or what they typed arrives. What the person
//! types is said at most every [`EVERY`] while they type, and `done` when they clear it,
//! unless [`super::Options::send_typing`] is off. An agent session's machine says which of its
//! sessions is at work in [`SESSION_TAG`], so it shows in that session's conversation, as
//! thinking; a machine at work for itself, such as a host's `zirc-remote` answering a person,
//! thinks too. The server keeps none of it in history.

use chrono::{DateTime, Duration, Utc};
use zirc_line::Line;

use super::conversation::{SESSION_MARK, agent, fold, is_channel, target};
use super::{Chat, nick_of};

/// The client-only tag that says someone is typing.
pub const TYPING_TAG: &str = "+typing";
/// What clients before the tag was ratified send.
const DRAFT_TYPING_TAG: &str = "+draft/typing";
/// The client-only tag with which a machine says which of its agent sessions is typing:
/// `zirc thinking` sends it with `+typing`.
pub const SESSION_TAG: &str = "+zirc.dev/session";
/// How often, at most, typing is said while the person types.
pub const EVERY: Duration = Duration::seconds(3);
/// How long someone shows as typing after they last said so.
pub const EXPIRES: Duration = Duration::seconds(6);

/// Someone typing in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typist {
    /// Their nick: a machine's, for one of its agent sessions.
    pub nick: String,
    /// The agent session at work, which is thinking rather than typing.
    pub session: Option<String>,
    /// When they last said they're typing.
    pub at: DateTime<Utc>,
}

/// People typing and agent sessions thinking, in words: each verb with its own names,
/// `a and b are typing…`, `cc-1 and cc-2 are thinking…`, `bob is typing, cc-1 is
/// thinking…`; more than three in all are `Several people are typing…`, as in Slack.
#[must_use]
pub fn typing_words(people: &[String], agents: &[String]) -> Option<String> {
    let said = |names: &[String], verb: &str| match names {
        [] => None,
        [one] => Some(format!("{one} is {verb}")),
        [rest @ .., last] => Some(format!("{} and {last} are {verb}", rest.join(", "))),
    };
    if people.len() + agents.len() > 3 {
        return Some("Several people are typing…".to_owned());
    }
    let parts: Vec<String> = [said(people, "typing"), said(agents, "thinking")]
        .into_iter()
        .flatten()
        .collect();
    (!parts.is_empty()).then(|| format!("{}…", parts.join(", ")))
}

impl Chat {
    /// Who is typing in the conversation called `name`, as of now.
    #[must_use]
    pub fn typing(&self, name: &str) -> Vec<Typist> {
        let now = (self.clock)();
        self.typing
            .get(&fold(name))
            .into_iter()
            .flatten()
            .filter(|typist| now - typist.at < EXPIRES)
            .cloned()
            .collect()
    }

    /// Who is typing in the conversation called `name`, in words every app shows above its
    /// composer: `bob is typing…`, `cc-1531f8 is thinking…`, `bob and carol are typing…`,
    /// `bob is typing, cc-1531f8 is thinking…`. A machine, with or without a session, thinks:
    /// it's a program at work, not a person typing. See [`typing_words`].
    #[must_use]
    pub fn typing_text(&self, name: &str) -> Option<String> {
        let (mut people, mut agents) = (Vec::new(), Vec::new());
        for typist in self.typing(name) {
            match typist.session {
                Some(session) => agents.push(session),
                None if self.is_machine(&typist.nick) => agents.push(typist.nick),
                None => people.push(self.title(&typist.nick)),
            }
        }
        typing_words(&people, &agents)
    }

    /// Whether to send typing at all: not when the person has turned it off.
    pub fn set_send_typing(&mut self, send: bool) {
        self.options.send_typing = send;
    }

    /// What the person's composer for the conversation called `name` holds changed: `empty`
    /// when they cleared it. Returns what to send: `active` at most every [`EVERY`] while
    /// they type, and `done` once when they clear it.
    pub fn typed(&mut self, name: &str, empty: bool) -> Vec<Line> {
        // Nobody reads the Server channel: the server would answer that there's no such nick.
        if !self.options.send_typing
            || !self.is_connected()
            || self.position(name).is_none()
            || self.is_server(name)
        {
            return Vec::new();
        }
        let now = (self.clock)();
        let key = fold(name);
        let state = if empty {
            if self.sent_typing.remove(&key).is_none() {
                return Vec::new();
            }
            "done"
        } else {
            if self
                .sent_typing
                .get(&key)
                .is_some_and(|at| now - *at < EVERY)
            {
                return Vec::new();
            }
            self.sent_typing.insert(key, now);
            "active"
        };
        let mut line = Line::new("TAGMSG", [target(name)]);
        line.tags = vec![(TYPING_TAG.to_owned(), Some(state.to_owned()))];
        vec![line]
    }

    /// What's said ends typing it, for us: the next keystroke says it again.
    pub(super) fn sent(&mut self, name: &str) {
        self.sent_typing.remove(&fold(name));
    }

    /// A message from `from` arrived in the conversation called `name`: they're no longer
    /// typing there.
    pub(super) fn arrived(&mut self, name: &str, from: &str) {
        if let Some(typists) = self.typing.get_mut(&fold(name)) {
            typists.retain(|typist| !typist.nick.eq_ignore_ascii_case(from));
        }
    }

    /// A `TAGMSG` saying someone is typing, has paused, or is done. Whether it was one.
    pub(super) fn typing_tag(&mut self, line: &Line) -> bool {
        let Some(state) = line.tag(TYPING_TAG).or_else(|| line.tag(DRAFT_TYPING_TAG)) else {
            return false;
        };
        let from = nick_of(line);
        let to = line.params.first().map_or("", String::as_str);
        if from.eq_ignore_ascii_case(&self.me) {
            return true;
        }
        let session = line
            .tag(SESSION_TAG)
            .filter(|session| !session.is_empty() && !session.contains(SESSION_MARK))
            // In a channel, any machine's session thinks out loud; to us, only our own
            // machines' sessions have conversations of their own.
            .filter(|_| is_channel(to) || self.ours(from))
            .map(str::to_owned);
        let name = match (&session, is_channel(to)) {
            (_, true) => to.to_owned(),
            (Some(session), false) => super::agent_channel(from, session),
            (None, false) => from.to_owned(),
        };
        debug_assert!(session.is_none() || is_channel(to) || agent(&name).is_some());
        let at = self.time_of(line);
        let typists = self.typing.entry(fold(&name)).or_default();
        typists.retain(|typist| {
            !(typist.nick.eq_ignore_ascii_case(from) && typist.session == session)
        });
        if state != "done" {
            typists.push(Typist {
                nick: from.to_owned(),
                session,
                at,
            });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_who_is_typing_and_which_agents_are_thinking_apart() {
        let names = |names: &[&str]| names.iter().map(|&n| n.to_owned()).collect::<Vec<_>>();
        let said = |people: &[&str], agents: &[&str]| typing_words(&names(people), &names(agents));
        assert_eq!(said(&[], &[]), None);
        assert_eq!(said(&["bob"], &[]).as_deref(), Some("bob is typing…"));
        assert_eq!(
            said(&["a", "b"], &[]).as_deref(),
            Some("a and b are typing…")
        );
        assert_eq!(
            said(&[], &["cc-1", "cc-2"]).as_deref(),
            Some("cc-1 and cc-2 are thinking…")
        );
        assert_eq!(
            said(&["bob"], &["cc-55aa66"]).as_deref(),
            Some("bob is typing, cc-55aa66 is thinking…")
        );
        assert_eq!(
            said(&["a", "b", "c"], &[]).as_deref(),
            Some("a, b and c are typing…")
        );
        assert_eq!(
            said(&["a", "b"], &["cc-1", "cc-2"]).as_deref(),
            Some("Several people are typing…")
        );
    }
}
