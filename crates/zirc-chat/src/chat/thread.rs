//! Threads, for every app: one is started from a message ([`Chat::start_thread`]) as a
//! channel of its own under the message's, `#api/login-times-out-on-safari`, named from the
//! message's first words ([`thread_name`]). The parent channel learns of it from the
//! `x-zirc/thread` event ([`zirc_proto::thread`]) and shows one line under the message,
//! "↳ 5 replies · last 3m" ([`Chat::thread_links`]). Everything else about a thread is an
//! ordinary channel's.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use zirc_line::Line;
use zirc_proto::thread::{Started, THREAD_OF_TAG};

use super::conversation::{Entry, EntryKind, is_channel};
use super::{Chat, ago_short, describe, room_label, room_parts};
use crate::names::channel_part;

/// How many of a message's first words name a thread started from it.
const NAME_WORDS: usize = 5;

/// The longest name a thread gets from them, in bytes.
const MAX_NAME_BYTES: usize = 40;

/// The longest quote of the starting message the thread opens with, in bytes.
const MAX_QUOTE_BYTES: usize = 300;

/// What a thread's first message starts with, before the quote of the message it hangs off.
const QUOTE: &str = "> ";

/// A thread's name made from `text`, a message's or one the person typed, as a channel's
/// part after the parent's `/`: its first few words, lowercase, joined by `-`, with only what
/// channel names agree on. "Login times out on Safari after the redirect" names
/// `login-times-out-on-safari`. `thread` when nothing in it will do.
#[must_use]
pub fn thread_name(text: &str) -> String {
    let mut name = String::new();
    for word in text.split_whitespace().take(NAME_WORDS) {
        let cleaned = channel_part(word).replace('/', "-");
        let word = cleaned.trim_matches(['-', '.', '_']);
        if word.is_empty() {
            continue;
        }
        if !name.is_empty() && name.len() + 1 + word.len() > MAX_NAME_BYTES {
            break;
        }
        if !name.is_empty() {
            name.push('-');
        }
        // Only ASCII is left, so any byte is a char's boundary.
        name.push_str(&word[..word.len().min(MAX_NAME_BYTES)]);
    }
    if name.is_empty() {
        "thread".to_owned()
    } else {
        name
    }
}

/// A thread started from a message, as the line under the message shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLink {
    /// Its channel, which the line opens: `#api/login-times-out`.
    pub name: String,
    /// Its title ([`room_label`]): `login times out`.
    pub title: String,
    /// Whether we're in it: until we are, what's said in it isn't known here.
    pub joined: bool,
    /// How many replies it has, as far as what's loaded says ([`Chat::replies`]).
    pub replies: usize,
    /// When something was last said in it.
    pub latest: Option<DateTime<Utc>>,
    /// What's unread in it.
    pub unread: usize,
    /// Of that, what mentions us.
    pub mentions: usize,
}

impl ThreadLink {
    /// The line at `now`, as every app shows it under the message: `↳ 5 replies · last 3m`,
    /// `↳ 1 reply · just now`, `↳ No replies yet`, or `↳ Thread` for one we're not in.
    #[must_use]
    pub fn label(&self, now: DateTime<Utc>) -> String {
        if !self.joined {
            return "↳ Thread".to_owned();
        }
        let count = match self.replies {
            0 => return "↳ No replies yet".to_owned(),
            1 => "1 reply".to_owned(),
            n => format!("{n} replies"),
        };
        match self.latest.map(|at| ago_short(at, now)) {
            Some(ago) if ago == "now" => format!("↳ {count} · just now"),
            Some(ago) => format!("↳ {count} · last {ago}"),
            None => format!("↳ {count}"),
        }
    }
}

/// Whether `entry` is something someone said that a thread's replies count: a message, an
/// action, a notice, or a zirc message's text or file, never another event.
fn is_reply(entry: &Entry) -> bool {
    match &entry.kind {
        EntryKind::Message(_) | EntryKind::Action(_) | EntryKind::Notice(_) => true,
        EntryKind::Zirc(message) => matches!(
            message.event,
            zirc_proto::Event::Text { .. } | zirc_proto::Event::File(_)
        ),
        _ => false,
    }
}

/// What `entry` says, as a quote of it reads: a message's text, or a zirc message's
/// headline.
fn said(entry: &Entry) -> Option<String> {
    match &entry.kind {
        EntryKind::Message(text) | EntryKind::Action(text) | EntryKind::Notice(text) => {
            Some(text.clone())
        }
        EntryKind::Zirc(message) => Some(describe::describe(&message.event).headline),
        _ => None,
    }
}

/// `text` cut to at most `max` bytes, at a char's boundary, with `…` when it was cut.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

impl Chat {
    /// The threads started from messages in the channel called `parent`, as its
    /// `x-zirc/thread` events say: each message's id and the thread's channel, the first
    /// thread a message got.
    pub(super) fn started(&self, parent: &str) -> Vec<(String, String)> {
        let mut started: Vec<(String, String)> = Vec::new();
        let Some(conversation) = self.conversation(parent) else {
            return started;
        };
        for entry in &conversation.entries {
            if let EntryKind::Zirc(message) = &entry.kind
                && let Some(thread) = Started::of(&message.event)
                && !started.iter().any(|(of, _)| *of == thread.of)
            {
                started.push((thread.of, thread.channel));
            }
        }
        started
    }

    /// How many replies the thread called `name` has, as far as what's loaded of it says:
    /// what people and agents said in it, not counting the quote it opens with.
    #[must_use]
    pub fn replies(&self, name: &str) -> usize {
        let Some(conversation) = self.conversation(name) else {
            return 0;
        };
        let said = conversation.entries.iter().filter(|entry| is_reply(entry));
        let quoted = said.clone().next().is_some_and(
            |first| matches!(&first.kind, EntryKind::Message(text) if text.starts_with(QUOTE)),
        );
        said.count() - usize::from(quoted)
    }

    /// The threads started from messages in the channel called `channel`, by the server's
    /// id for each message, for the line under it ([`ThreadLink::label`]).
    #[must_use]
    pub fn thread_links(&self, channel: &str) -> HashMap<String, ThreadLink> {
        let threads = self.threads(channel);
        self.started(channel)
            .into_iter()
            .map(|(of, name)| {
                let link = match threads
                    .iter()
                    .find(|thread| thread.name.eq_ignore_ascii_case(&name))
                {
                    Some(thread) => ThreadLink {
                        name: thread.name.clone(),
                        title: thread.title.clone(),
                        joined: true,
                        replies: thread.replies,
                        latest: thread.latest_ms.and_then(DateTime::from_timestamp_millis),
                        unread: thread.unread,
                        mentions: thread.mentions,
                    },
                    None => ThreadLink {
                        title: room_label(&name).unwrap_or_else(|| name.clone()),
                        joined: self.conversation(&name).is_some_and(|c| c.joined),
                        replies: self.replies(&name),
                        latest: None,
                        unread: 0,
                        mentions: 0,
                        name,
                    },
                };
                (of, link)
            })
            .collect()
    }

    /// Starts a thread called `name` ([`thread_name`] cleans it) from the message `of`, by
    /// the server's id, in the channel `parent`: joins `#<parent>/<name>`, opens it with a
    /// quote of the message tagged [`THREAD_OF_TAG`], and tells `parent` with an
    /// `x-zirc/thread` event. A message that has a thread already opens that one instead.
    /// The thread's channel and what to send; `None` when `parent` isn't a channel, is a
    /// thread itself, or the message isn't loaded there.
    pub fn start_thread(
        &mut self,
        parent: &str,
        of: &str,
        name: &str,
    ) -> Option<(String, Vec<Line>)> {
        if !is_channel(parent) || room_parts(parent).is_some() {
            return None;
        }
        if let Some((_, channel)) = self.started(parent).into_iter().find(|(id, _)| id == of) {
            let lines = self.join(std::slice::from_ref(&channel));
            return Some((channel, lines));
        }
        let entry = self
            .conversation(parent)?
            .entries
            .iter()
            .find(|entry| entry.id.as_deref() == Some(of))?;
        let session = match &entry.kind {
            EntryKind::Zirc(message) => Some(message.session.clone()).filter(|s| !s.is_empty()),
            _ => None,
        };
        let text = said(entry)?;
        let who = self.sender_name(&entry.from, session.as_deref());
        let first = text.lines().next().unwrap_or_default();
        let quote = format!("{QUOTE}{who}: {}", cut(first, MAX_QUOTE_BYTES));
        let channel = format!("{parent}/{}", thread_name(name));

        let mut lines = self.join(std::slice::from_ref(&channel));
        let mut opening = self.say(&channel, &quote, false);
        if let Some(line) = opening.iter_mut().find(|line| line.command == "PRIVMSG") {
            line.tags
                .push((THREAD_OF_TAG.to_owned(), Some(of.to_owned())));
        }
        lines.extend(opening);
        let started = Started {
            channel: channel.clone(),
            of: of.to_owned(),
        };
        lines.extend(self.post_event(parent, "", started.event()));
        Some((channel, lines))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::super::{Link, Options};
    use super::*;

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
    }

    fn chat() -> Chat {
        let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
        chat.clock = noon;
        chat.connected(Link {
            nick: "me".into(),
            metadata: false,
            chathistory: true,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
        });
        feed(&mut chat, &[":me!u@h JOIN #api"]);
        chat
    }

    fn feed(chat: &mut Chat, lines: &[&str]) {
        for line in lines {
            let _ = chat.receive(line.parse().unwrap());
        }
    }

    #[test]
    fn names_a_thread_from_the_first_words() {
        assert_eq!(
            thread_name("Login times out on Safari after the redirect"),
            "login-times-out-on-safari"
        );
        // Only what channel names agree on, never a "/" that would make a thread's thread.
        assert_eq!(thread_name("fix a/b: now!"), "fix-a-b-now");
        assert_eq!(thread_name("  ✨ "), "thread");
        assert!(thread_name(&"x".repeat(100)).len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn starts_a_thread_from_a_message_and_shows_it_under_the_message() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                "@time=2026-10-30T11:00:00.000Z;msgid=m1 :carol!u@h PRIVMSG #api :Login times out on Safari after the redirect",
            ],
        );
        let (channel, lines) = chat
            .start_thread("#api", "m1", "Login times out on Safari")
            .unwrap();
        assert_eq!(channel, "#api/login-times-out-on-safari");
        let lines: Vec<String> = lines.iter().map(ToString::to_string).collect();
        // Join it, open it with a quote tagged with the message it hangs off, and tell #api.
        assert!(
            lines[0].ends_with("JOIN #api/login-times-out-on-safari"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains("+zirc.dev/thread-of=m1")
                && lines[1].ends_with(
                    "PRIVMSG #api/login-times-out-on-safari :> carol: Login times out on Safari after the redirect"
                ),
            "{}",
            lines[1]
        );
        assert!(lines[2].contains("PRIVMSG #api :"), "{}", lines[2]);

        // The server echoes them back, and someone replies.
        let event = zirc_proto::encode(&zirc_proto::Message::new(
            "",
            Started {
                channel: channel.clone(),
                of: "m1".into(),
            }
            .event(),
        ))
        .unwrap();
        let started =
            format!("@time=2026-10-30T11:01:00.000Z;msgid=e1 :me!u@h PRIVMSG #api :{event}");
        feed(
            &mut chat,
            &[
                ":me!u@h JOIN #api/login-times-out-on-safari",
                "@time=2026-10-30T11:01:00.000Z;msgid=q1;+zirc.dev/thread-of=m1 :me!u@h PRIVMSG #api/login-times-out-on-safari :> carol: Login times out on Safari after the redirect",
                &started,
                "@time=2026-10-30T11:57:00.000Z;msgid=r1 :bob!u@h PRIVMSG #api/login-times-out-on-safari :me: which version?",
            ],
        );
        // Under the message, one line for the thread; the quote isn't a reply.
        let under = chat.thread_links("#api");
        let link = &under["m1"];
        assert_eq!(link.name, channel);
        assert_eq!(link.replies, 1);
        assert_eq!(link.label(noon()), "↳ 1 reply · last 3m");
        assert_eq!(chat.threads("#api")[0].of.as_deref(), Some("m1"));

        // The event itself isn't a row in #api once its message is there.
        let api = chat.conversation("#api").unwrap();
        let rows = chat.items(api, |at| at.date_naive());
        assert!(
            !rows.iter().any(|row| matches!(row, super::super::Item::Event { text, .. } if text.contains("started a thread"))),
            "{rows:?}"
        );

        // Starting it again from the same message opens the one it has.
        let (again, lines) = chat.start_thread("#api", "m1", "other").unwrap();
        assert_eq!(again, channel);
        assert!(lines.iter().all(|line| line.command != "PRIVMSG"));
        // Not from a thread, nor from what isn't loaded.
        assert!(chat.start_thread(&channel, "r1", "deeper").is_none());
        assert!(chat.start_thread("#api", "gone", "x").is_none());
    }

    #[test]
    fn says_how_a_thread_stands_in_one_line() {
        let link = |joined, replies, latest| ThreadLink {
            name: "#api/x".into(),
            title: "x".into(),
            joined,
            replies,
            latest,
            unread: 0,
            mentions: 0,
        };
        let now = noon();
        assert_eq!(link(false, 0, None).label(now), "↳ Thread");
        assert_eq!(link(true, 0, None).label(now), "↳ No replies yet");
        assert_eq!(
            link(true, 12, Some(now - chrono::Duration::hours(2))).label(now),
            "↳ 12 replies · last 2h"
        );
        assert_eq!(link(true, 1, Some(now)).label(now), "↳ 1 reply · just now");
    }
}
