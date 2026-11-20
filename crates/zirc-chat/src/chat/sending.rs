//! What we say - messages, replies, reactions and zirc events - and their echoes.

use super::{
    CLIENT_TAG, Chat, Entry, EntryKind, Event, Line, MAX_TEXT_BYTES, Message, Outgoing, REACT,
    REPLY, UNREACT, agent, formatted, kind_of, multiline, pieces, split_message, strip_formatting,
    to_agent,
};

impl Chat {
    /// The lines that say `text` to `target`, as an action for `/me`. A text too long for
    /// a line goes as one multiline message where the server takes them, and in pieces
    /// elsewhere. Without `echo-message` it shows here at once.
    pub fn say(&mut self, target: &str, text: &str, action: bool) -> Vec<Line> {
        self.sent(target);
        if let Some((nick, session)) = agent(target) {
            // An agent hears plain words; zircd takes only text for it.
            let lines = self.said(nick, &to_agent(session, text), false);
            self.echo(target, EntryKind::Message(strip_formatting(text)));
            return lines;
        }
        let lines = self.said(target, text, action);
        let kind = if action {
            EntryKind::Action(strip_formatting(text))
        } else {
            EntryKind::Message(strip_formatting(text))
        };
        self.echo_reply(target, kind, None, formatted(text));
        lines
    }

    /// Says `text` to `target` in reply to its message `to`, by the server's id: an
    /// ordinary message with `+draft/reply`, which zirc's apps show beneath a quote of the
    /// message it answers and plain IRC clients as an ordinary line. Returns what to send.
    pub fn say_reply(&mut self, target: &str, to: &str, text: &str) -> Vec<Line> {
        let mut lines = self.said(target, text, false);
        tag_opening(&mut lines, REPLY, to);
        self.echo_reply(
            target,
            EntryKind::Message(strip_formatting(text)),
            Some(to),
            formatted(text),
        );
        lines
    }

    /// How many messages `text` goes as, for the composer to say before it's sent: one,
    /// unless it's more than the server takes in one ([`split_message`]), or the server takes
    /// no multiline messages and it has several lines or is too long for one.
    #[must_use]
    pub fn sends_as(&self, text: &str) -> usize {
        self.outgoing(text, false).len()
    }

    /// The messages `text` goes as, as [`Chat::said`] sends them: each whole as one
    /// multiline message, or as a line of its own.
    pub(super) fn outgoing(&self, text: &str, action: bool) -> Vec<Outgoing> {
        let body = |text: &str| {
            if action {
                format!("\x01ACTION {text}\x01")
            } else {
                text.to_owned()
            }
        };
        let lines = |text: &str| {
            pieces(text, &body)
                .into_iter()
                .map(Outgoing::Line)
                .collect::<Vec<_>>()
        };
        let limits = self.link.as_ref().and_then(|link| link.multiline);
        // A line break can't go in a line: several lines are a multiline message too.
        if body(text).len() <= MAX_TEXT_BYTES && !text.contains('\n') {
            vec![Outgoing::Line(body(text))]
        } else if let (Some(limits), false) = (limits, action) {
            // Over what one message takes: as several, each whole, cut where a reader pauses.
            split_message(text, limits)
                .into_iter()
                .flat_map(|message| {
                    if multiline::batch("x", "#x", &message, limits).is_some() {
                        vec![Outgoing::Batch(message)]
                    } else {
                        lines(&message)
                    }
                })
                .collect()
        } else {
            lines(text)
        }
    }

    /// The lines that say `text` to `target`, as [`Chat::say`] sends them.
    pub(super) fn said(&mut self, target: &str, text: &str, action: bool) -> Vec<Line> {
        let lines = self.said_now(target, text, action);
        if self.is_connected() {
            return lines;
        }
        // Offline: it waits, and goes once the conversation's joined again.
        self.held.push((target.to_owned(), lines));
        Vec::new()
    }

    /// What was said to `target` while not connected, now that it can go: a channel's once
    /// joined again, a direct message's (`channel` false) once connected.
    pub(super) fn release(&mut self, target: Option<&str>) -> Vec<Line> {
        let (going, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|(to, _)| match target {
                Some(channel) => to.eq_ignore_ascii_case(channel),
                None => !super::is_channel(to),
            });
        self.held = waiting;
        going.into_iter().flat_map(|(_, lines)| lines).collect()
    }

    fn said_now(&mut self, target: &str, text: &str, action: bool) -> Vec<Line> {
        let limits = self.link.as_ref().and_then(|link| link.multiline);
        let mut lines = Vec::new();
        for message in self.outgoing(text, action) {
            match (message, limits) {
                (Outgoing::Batch(text), Some(limits)) => {
                    let reference = self.next_batch();
                    lines.extend(
                        multiline::batch(&reference, target, &text, limits).unwrap_or_default(),
                    );
                }
                (Outgoing::Line(body) | Outgoing::Batch(body), _) => {
                    lines.push(Line::new("PRIVMSG", [target.to_owned(), body]));
                }
            }
        }
        self.tag_client(lines)
    }

    /// The lines that post `text` to `target` as a zirc message from `session`, which
    /// agents hear in `zirc watch` and plain clients read as text. One too long for a line
    /// goes whole as a multiline message where the server takes them, and as plain text
    /// elsewhere.
    pub fn post(&mut self, target: &str, session: &str, text: &str) -> Vec<Line> {
        let event = Event::Text {
            body: text.to_owned(),
        };
        self.send_event(target, Message::new(session, event))
    }

    /// The lines that share `file`, uploaded to zirc.dev, in `target` as a zirc message from
    /// `session`, which zirc's apps draw inline and plain clients read as its link. As
    /// [`Chat::post`], one too long for a line goes as a multiline message, or as its link.
    pub fn share(&mut self, target: &str, session: &str, file: zirc_proto::File) -> Vec<Line> {
        self.send_event(target, Message::new(session, Event::File(file)))
    }

    /// The lines that send `event` to `target` as a zirc message from `session`, as
    /// [`Chat::post`] sends text: on one line, as one multiline message, or as its text when
    /// neither fits.
    pub fn post_event(&mut self, target: &str, session: &str, event: Event) -> Vec<Line> {
        self.send_event(target, Message::new(session, event))
    }

    /// The lines that send `message` to `target`: on one line, as one multiline message, or
    /// as its text when neither fits.
    pub(super) fn send_event(&mut self, target: &str, message: Message) -> Vec<Line> {
        let lines = if let Ok(body) = zirc_proto::encode(&message) {
            vec![Line::new("PRIVMSG", [target.to_owned(), body])]
        } else {
            let limits = self.link.as_ref().and_then(|link| link.multiline);
            let whole = limits.and_then(|limits| {
                let body = zirc_proto::encode_up_to(&message, limits.max_bytes).ok()?;
                multiline::batch(&self.next_batch(), target, &body, limits)
            });
            match whole {
                Some(lines) => lines,
                None => return self.say(target, &message.event.human_text(), false),
            }
        };
        self.echo(target, EntryKind::Zirc(message));
        lines
    }

    /// The line that gives `emoji` to message `id` in `target`, or takes it back if we gave
    /// it already: an `IRCv3` `+draft/react` (or `+draft/unreact`) TAGMSG replying to it.
    #[must_use]
    pub fn react(&self, target: &str, id: &str, emoji: &str) -> Vec<Line> {
        let given = self
            .conversation(target)
            .and_then(|c| c.reactions(id).iter().find(|r| r.emoji == emoji))
            .is_some_and(|r| r.by(&self.me));
        let tag = if given { UNREACT } else { REACT };
        let mut line = Line::new("TAGMSG", [super::target(target)]);
        line.tags = vec![
            (tag.to_owned(), Some(emoji.to_owned())),
            (REPLY.to_owned(), Some(id.to_owned())),
        ];
        vec![line]
    }

    /// The line that asks the server to trim message `id` from `target`'s history, saying
    /// why if `reason` does. Once it has, it says so, and the message goes from here as from
    /// every zirc client; if it won't, it says why, which shows where the person is looking.
    #[must_use]
    pub fn trim(&self, target: &str, id: &str, reason: Option<&str>) -> Vec<Line> {
        vec![crate::trim::request(super::target(target), id, reason)]
    }

    /// Says `text` in the conversation shown, as for an error.
    pub fn note(&mut self, text: String) {
        self.info(self.selected, text);
    }

    /// Says `text` in the conversation at `at`.
    pub fn info(&mut self, at: usize, text: String) {
        let entry = Entry::info((self.clock)(), text);
        let max = self.options.max_entries;
        if let Some(conversation) = self.conversations.get_mut(at) {
            conversation.insert(entry, max);
        }
    }

    /// What we said, shown at once when the server won't echo it back.
    pub(super) fn echo(&mut self, target: &str, kind: EntryKind) {
        self.echo_reply(target, kind, None, None);
    }

    /// As [`Chat::echo`], for a reply to message `to`, keeping `formatted` text.
    pub(super) fn echo_reply(
        &mut self,
        target: &str,
        kind: EntryKind,
        to: Option<&str>,
        formatted: Option<String>,
    ) {
        if self.link.as_ref().is_some_and(|link| link.echo) {
            return;
        }
        let at = self.ensure(target, kind_of(target));
        let entry = Entry {
            from: self.me.clone(),
            owner: None,
            kind,
            sent_from: self.options.client.clone(),
            reply_to: to.map(str::to_owned),
            formatted,
            ..Entry::info((self.clock)(), String::new())
        };
        self.conversations[at].insert(entry, self.options.max_entries);
    }

    /// `lines` saying which app they're from, when the app said: each message, or a
    /// multiline message's opening line, whose tags the whole message takes.
    pub(super) fn tag_client(&self, mut lines: Vec<Line>) -> Vec<Line> {
        let Some(client) = &self.options.client else {
            return lines;
        };
        for line in &mut lines {
            let opens_batch =
                line.command == "BATCH" && line.params.first().is_some_and(|r| r.starts_with('+'));
            let alone = line.command == "PRIVMSG" && line.tag("batch").is_none();
            if opens_batch || alone {
                line.tags
                    .push((CLIENT_TAG.to_owned(), Some(client.clone())));
            }
        }
        lines
    }
}

/// Tags the first message in `lines` with `tag` = `value`: its own line, or a multiline
/// message's opening one, whose tags the whole message takes.
pub(super) fn tag_opening(lines: &mut [Line], tag: &str, value: &str) {
    if let Some(line) = lines.iter_mut().find(|line| {
        (line.command == "PRIVMSG" && line.tag("batch").is_none())
            || (line.command == "BATCH" && line.params.first().is_some_and(|r| r.starts_with('+')))
    }) {
        line.tags.push((tag.to_owned(), Some(value.to_owned())));
    }
}
