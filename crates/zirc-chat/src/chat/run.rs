//! A typed `/command` carried out ([`Chat::run`]), as every app carries it out: the lines it
//! sends, or what the app does as its own buttons do (a card, a nick change, closing a
//! direct conversation, asking before leaving), or what it says back. What a command reads
//! as is [`crate::command::parse`]'s; what it says back is [`crate::command::said`]'s.

use zirc_line::Line;

use super::{Chat, Kind, is_channel, is_group};
use crate::command::{Command, said};
use crate::emoji::{self, Newest};

/// Where a typed line is carried out, and what the app knows there that zirc-chat doesn't.
#[derive(Debug, Clone, Copy)]
pub struct Here<'a> {
    /// The conversation shown, by name.
    pub name: &'a str,
    /// The message a reaction or `/reply` goes to, when the person picked one; else the
    /// newest said in it.
    pub picked: Option<&'a str>,
    /// The message the reply being written answers, if one is: what's typed is sent as its
    /// answer.
    pub replying: Option<&'a str>,
    /// The message of ours being corrected, if one is: what's typed goes in its place
    /// ([`Chat::say_edit`]).
    pub editing: Option<&'a str>,
    /// The newest emoji the app draws, for `/react`.
    pub newest: Newest,
}

impl<'a> Here<'a> {
    /// In the conversation `name`, nothing picked or being answered, for an app drawing
    /// emoji up to `newest`.
    #[must_use]
    pub fn new(name: &'a str, newest: Newest) -> Self {
        Self {
            name,
            picked: None,
            replying: None,
            editing: None,
            newest,
        }
    }
}

/// What a typed line comes to ([`Chat::run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    /// Send these, maybe none.
    Send(Vec<Line>),
    /// Send these, which answer the message being replied to or picked: the app puts the
    /// reply away.
    Replied(Vec<Line>),
    /// Send these, which correct the message being edited, maybe none when it's too late
    /// (a note says so): the app puts the editing away.
    Edited(Vec<Line>),
    /// `/react`: send these, and keep `emoji` among those used lately.
    Reacted {
        /// The emoji given.
        emoji: &'static str,
        /// What to send.
        lines: Vec<Line>,
    },
    /// `/whois nick`: show `nick`'s card, as tapping them does.
    Card(String),
    /// `/nick name`: change our nick to `name`, as the app's profile does: through zirc.dev
    /// once signed in there.
    Nick(String),
    /// `/close` in a direct conversation: close it, as its close button does, which the app
    /// keeps closed ([`Chat::close_direct`]).
    CloseDirect(String),
    /// `/part`: leave `channel`, asking first as the app's Leave does.
    Leave {
        /// The channel.
        channel: String,
        /// Why, if said.
        reason: Option<String>,
    },
    /// Nothing to send: say this, as a note or an error.
    Note(String),
    /// A command zirc-chat leaves to the app: one only some apps carry out (the terminal
    /// client's picker, `/quote`, `/sessions`…) or one that asks zirc.dev.
    /// An app that doesn't carry it out says [`said::refused`].
    App(Command),
}

fn note(words: impl Into<String>) -> Ran {
    Ran::Note(words.into())
}

/// For how many seconds after a line is typed in the Server channel every reply shows there.
const RAW_REPLIES_FOR: i64 = 10;

impl Chat {
    /// What's typed in the Server channel: a line that goes to the server as it is (the user's
    /// choice, 16 Nov 2026), and stays there under what it answers.
    fn send_raw(&mut self, text: String) -> Ran {
        let Ok(line) = text.parse::<Line>() else {
            return note(said::NOT_A_LINE);
        };
        let now = (self.clock)();
        let me = self.me.clone();
        self.server_said(now, &me, text);
        // What answers it, whatever it is, shows here too.
        self.raw_until = Some(now + chrono::TimeDelta::seconds(RAW_REPLIES_FOR));
        Ran::Send(vec![line])
    }

    /// Carries out `command`, typed in the conversation `here`, as every app does.
    pub fn run(&mut self, here: &Here<'_>, command: Command) -> Ran {
        let name = here.name;
        let shown = self.conversation(name);
        let server = shown.is_none_or(|c| c.kind == Kind::Server);
        // A channel we were in, to join again once connected, takes what's said too: it waits
        // and goes then ([`Chat::release`]).
        let joined = shown.is_some_and(|c| {
            c.joined || c.kind == Kind::Query || c.standing == super::Attendance::Rejoin
        });
        match command {
            Command::Say(text) if server && self.server_shown => self.send_raw(text),
            Command::Say(_) | Command::Me(_) if server => note(said::NOT_A_CONVERSATION),
            Command::Say(_) | Command::Me(_) if !self.can_message(name) => note(
                self.read_only_note(name)
                    .map_or_else(|| said::TAKES_NO_MESSAGES.to_owned(), |note| note.text),
            ),
            Command::Say(_) | Command::Me(_) if !joined => note(said::JOIN_FIRST),
            // Nothing between || markers goes unsealed: sealed in a new message or a reply,
            // refused anywhere else.
            Command::Say(text) if super::seals(&text) => match (here.editing, here.replying) {
                (Some(_), _) => note(super::SEALS_ONLY_SAYING),
                (None, to) => match self.say_sealed(name, &text, to) {
                    Ok(lines) if to.is_some() => Ran::Replied(lines),
                    Ok(lines) => Ran::Send(lines),
                    Err(why) => note(why),
                },
            },
            Command::Me(text) | Command::Reply(Some(text)) | Command::Msg { text, .. }
                if super::seals(&text) =>
            {
                note(super::SEALS_ONLY_SAYING)
            }
            Command::Say(text) => match (here.editing, here.replying) {
                (Some(id), _) => Ran::Edited(self.say_edit(name, id, &text)),
                (None, Some(id)) => Ran::Replied(self.say_reply(name, id, &text)),
                (None, None) => Ran::Send(self.say(name, &text, false)),
            },
            Command::Me(text) => Ran::Send(self.say(name, &text, true)),
            Command::Reply(Some(text)) => match self.answered(here) {
                Some(id) => Ran::Replied(self.say_reply(name, &id, &text)),
                None => note(said::NOTHING_TO_REPLY),
            },
            Command::React(Some(typed)) => {
                let Some(emoji) = emoji::find(&typed, here.newest) else {
                    return note(said::no_emoji(&typed));
                };
                match self.answered(here) {
                    Some(id) => Ran::Reacted {
                        emoji,
                        lines: self.react(name, &id, emoji),
                    },
                    None => note(said::NOTHING_TO_REACT),
                }
            }
            Command::Msg { target, text } => Ran::Send(self.msg(&target, &text)),
            Command::Query(nick) => Ran::Send(self.open(&nick)),
            Command::Group(nicks) => Ran::Send(self.create_group(&nicks)),
            Command::Invite(nicks) if nicks.is_empty() => note(said::INVITE_NEEDS),
            Command::Invite(nicks) if is_group(name) => Ran::Send(self.add_to_group(name, &nicks)),
            Command::Invite(_) => note(said::NOT_A_GROUP),
            Command::Join(channels) => Ran::Send(self.join(&channels)),
            Command::Part { channel, reason } => {
                match channel.or_else(|| is_channel(name).then(|| name.to_owned())) {
                    Some(channel) => Ran::Leave { channel, reason },
                    None => note(said::PART_NEEDS),
                }
            }
            Command::Close => self.close_here(name),
            Command::Topic(None) => note(
                shown
                    .and_then(|c| c.topic.clone())
                    .unwrap_or_else(|| said::NO_TOPIC.to_owned()),
            ),
            Command::Topic(Some(topic)) if is_channel(name) && !server => {
                Ran::Send(vec![Line::new("TOPIC", [name.to_owned(), topic])])
            }
            Command::Topic(Some(_)) => note(said::TOPIC_FOR_CHANNELS),
            Command::Away(why) => Ran::Send(self.set_away(why.as_deref())),
            Command::Whois(nick) => Ran::Card(nick),
            Command::Nick(nick) => Ran::Nick(nick),
            other => Ran::App(other),
        }
    }

    /// The message a reaction or `/reply` goes to: the one picked, or the newest said here.
    fn answered(&self, here: &Here<'_>) -> Option<String> {
        here.picked.map(str::to_owned).or_else(|| {
            self.conversation(here.name)?
                .newest_said_id()
                .map(str::to_owned)
        })
    }

    /// `/msg`: says `text` to `target`, a channel or someone,
    /// opening the conversation first unless it's a channel we're in.
    fn msg(&mut self, target: &str, text: &str) -> Vec<Line> {
        let joined = self.conversation(target).is_some_and(|c| c.joined);
        let mut lines = if is_channel(target) && joined {
            Vec::new()
        } else {
            self.open(target)
        };
        lines.extend(self.say(target, text, false));
        lines
    }

    /// `/close` in `name`: a channel left and closed, a direct conversation closed by the
    /// app.
    fn close_here(&mut self, name: &str) -> Ran {
        match self.position(name) {
            Some(_) if !is_channel(name) => Ran::CloseDirect(name.to_owned()),
            Some(at) => Ran::Send(self.close(at)),
            None => note(said::JOIN_FIRST),
        }
    }
}
