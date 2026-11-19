//! What reaches a person's phone while its app is closed, and why: the tenant's pusher
//! (zirc-sidecar) asks this of each line its server keeps, for each person with a phone, so
//! it pushes by the rules every app's Activity and notifications go by. What's aimed at the
//! person, as Activity lists it: a channel message naming them or answering one of theirs,
//! and a reaction to one of theirs; and, as the sidebar says them, a direct message, and
//! anything said where they asked to hear everything; and an agent's open question to them,
//! which waits on their answer, as a mention does. Each by how much that conversation
//! notifies on their phone ([`NotifySettings`]), which the phone tells the pusher.

use zirc_proto::push::Why;

use super::notify::{Level, NotifySettings};
use super::{EntryKind, REACT, REPLY, entry_of, is_group};

/// What a pushed reaction's notification says: `reacted 👍: the login fix is in`, the start
/// of what was reacted to after the emoji, as Activity says it (`reacted 👍`).
#[must_use]
pub fn reacted(emoji: &str, quote: &str) -> String {
    let what = format!("{} {emoji}", super::activity::REACTED);
    if quote.is_empty() {
        what
    } else {
        format!("{what}: {quote}")
    }
}

/// One line the server kept, as the pusher heard it, for one person.
#[derive(Debug, Clone)]
pub struct Heard<'a> {
    /// The conversation as the person names it: the channel, or the other's nick.
    pub conversation: &'a str,
    /// The line's command: `PRIVMSG`, `NOTICE` or `TAGMSG`.
    pub command: &'a str,
    /// Its body, for a `PRIVMSG` or `NOTICE`.
    pub body: &'a str,
    /// Its value of `+draft/react`, for a reaction.
    pub reaction: Option<&'a str>,
    /// Whether what it answers or reacts to (`+draft/reply`) was the person's.
    pub to_theirs: bool,
}

impl<'a> Heard<'a> {
    /// The line `line` in `conversation`: `to_theirs` says whether the message its
    /// `+draft/reply` names was the person's.
    #[must_use]
    pub fn of(line: &'a zirc_line::Line, conversation: &'a str, to_theirs: bool) -> Self {
        Self {
            conversation,
            command: line.command.as_str(),
            body: line.params.get(1).map_or("", String::as_str),
            reaction: line.tag(REACT).filter(|emoji| !emoji.is_empty()),
            to_theirs: to_theirs && line.tag(REPLY).is_some_and(|id| !id.is_empty()),
        }
    }
}

/// Why `heard` is pushed to `me` (their nick), with `settings` their phone's and `now` in
/// seconds since the Unix epoch; `None` when it isn't. Never what `me` said themselves: the
/// pusher leaves those out before asking.
#[must_use]
pub fn why(settings: &NotifySettings, now: i64, me: &str, heard: &Heard<'_>) -> Option<Why> {
    let name = heard.conversation;
    let direct = !super::conversation::is_channel(name) || is_group(name);
    if heard.command == "TAGMSG" {
        // A reaction to what they said, anywhere, unless they asked to hear nothing there.
        let wanted = heard.reaction.is_some()
            && heard.to_theirs
            && !settings.is_muted(name, now)
            && settings.level(name) != Level::Nothing;
        return wanted.then_some(Why::Reaction);
    }
    // A notice is the server's or a bot's, never a person's words to them.
    if heard.command != "PRIVMSG" {
        return None;
    }
    let (kind, words) = entry_of(heard.body, false)?;
    if asks(&kind, me) {
        return settings
            .notifies(name, direct, true, now)
            .then_some(Why::Question);
    }
    if words.trim().is_empty() {
        return None;
    }
    if direct {
        return settings
            .notifies(name, true, false, now)
            .then_some(Why::Direct);
    }
    // Aimed at them: as a mention, through a mute and a level of Nothing.
    let why = if heard.to_theirs {
        Some(Why::Reply)
    } else if names_me(&kind, &words, me) {
        Some(Why::Mention)
    } else {
        None
    };
    match why {
        Some(why) => settings.notifies(name, false, true, now).then_some(why),
        None => settings
            .notifies(name, false, false, now)
            .then_some(Why::All),
    }
}

/// Whether a line of `kind`, saying `words`, names `me` as said to them, as Activity and the
/// phones count a mention: [`crate::names::names_person`], unless it only says back what
/// someone decided ([`zirc_proto::Event::says_a_choice`]), which names them for the record.
pub(super) fn names_me(kind: &EntryKind, words: &str, me: &str) -> bool {
    let record = matches!(kind, EntryKind::Zirc(message) if message.event.says_a_choice());
    !record && crate::names::names_person(words, me)
}

/// Whether `kind` is an agent's question to `me` still waiting on an answer.
fn asks(kind: &EntryKind, me: &str) -> bool {
    let EntryKind::Zirc(message) = kind else {
        return false;
    };
    zirc_proto::request::Request::of(&message.event).is_some_and(|asked| {
        asked.state == zirc_proto::request::State::Open && asked.by.eq_ignore_ascii_case(me)
    })
}

#[cfg(test)]
mod tests {
    use super::super::notify::Muted;
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn said<'a>(conversation: &'a str, body: &'a str) -> Heard<'a> {
        Heard {
            conversation,
            command: "PRIVMSG",
            body,
            reaction: None,
            to_theirs: false,
        }
    }

    fn settings(levels: &[(&str, Level)], muted: &[&str]) -> NotifySettings {
        NotifySettings {
            levels: levels.iter().map(|(n, l)| ((*n).to_owned(), *l)).collect(),
            muted: muted
                .iter()
                .map(|n| ((*n).to_owned(), Muted::Off))
                .collect(),
        }
    }

    #[test]
    fn pushes_what_activity_lists_and_direct_messages() {
        let none = NotifySettings::default();
        assert_eq!(
            why(&none, NOW, "ada", &said("#api", "ada: look?")),
            Some(Why::Mention)
        );
        assert_eq!(why(&none, NOW, "ada", &said("#api", "adam: look?")), None);
        assert_eq!(
            why(&none, NOW, "ada", &said("bob", "hi")),
            Some(Why::Direct)
        );
        let reply = Heard {
            to_theirs: true,
            ..said("#api", "agreed")
        };
        assert_eq!(why(&none, NOW, "ada", &reply), Some(Why::Reply));
        let reaction = Heard {
            command: "TAGMSG",
            body: "",
            reaction: Some("👍"),
            to_theirs: true,
            conversation: "#api",
        };
        assert_eq!(why(&none, NOW, "ada", &reaction), Some(Why::Reaction));
        let to_someone_else = Heard {
            to_theirs: false,
            ..reaction.clone()
        };
        assert_eq!(why(&none, NOW, "ada", &to_someone_else), None);
        let notice = Heard {
            command: "NOTICE",
            ..said("#api", "ada: deploy done")
        };
        assert_eq!(why(&none, NOW, "ada", &notice), None);
    }

    #[test]
    fn keeps_to_each_conversations_level_and_mute() {
        let all = settings(&[("#api", Level::All)], &[]);
        assert_eq!(
            why(&all, NOW, "ada", &said("#api", "deploying")),
            Some(Why::All)
        );
        let quiet = settings(
            &[("#api", Level::Nothing), ("bob", Level::Nothing)],
            &["#web"],
        );
        assert_eq!(
            why(&quiet, NOW, "ada", &said("#api", "ada: still named")),
            Some(Why::Mention),
            "naming them gets through a level of Nothing"
        );
        assert_eq!(
            why(&quiet, NOW, "ada", &said("#web", "ada: and a mute")),
            Some(Why::Mention)
        );
        assert_eq!(why(&quiet, NOW, "ada", &said("bob", "hi")), None);
        let reaction = Heard {
            command: "TAGMSG",
            body: "",
            reaction: Some("👍"),
            to_theirs: true,
            conversation: "#web",
        };
        assert_eq!(why(&quiet, NOW, "ada", &reaction), None);
        let for_an_hour = NotifySettings {
            muted: [("bob".to_owned(), Muted::Time { at: NOW + 60 })].into(),
            ..NotifySettings::default()
        };
        assert_eq!(why(&for_an_hour, NOW, "ada", &said("bob", "hi")), None);
        assert_eq!(
            why(&for_an_hour, NOW + 61, "ada", &said("bob", "hi")),
            Some(Why::Direct)
        );
    }

    /// An agent's open question to `by`, as zirc request posts it.
    fn question(by: &str) -> String {
        let asked = zirc_proto::request::Request::open(
            "ada-mbp/1",
            "cc-1",
            by,
            "Drop the old sessions table?",
            "migration 0042 drops it",
            "keeps writing both",
        );
        zirc_proto::encode_up_to(&zirc_proto::Message::new("cc-1", asked.event()), 4096).unwrap()
    }

    #[test]
    fn an_agents_question_to_them_gets_through_as_a_mention_does() {
        let quiet = settings(&[("#api", Level::Nothing)], &["#web"]);
        assert_eq!(
            why(&quiet, NOW, "ada", &said("#api", &question("ada"))),
            Some(Why::Question)
        );
        assert_eq!(
            why(&quiet, NOW, "ada", &said("#web", &question("ADA"))),
            Some(Why::Question),
            "through a mute, whatever the case of their nick"
        );
        assert_eq!(
            why(&quiet, NOW, "ada", &said("#api", &question("bob"))),
            None
        );
    }

    #[test]
    fn what_they_decided_said_back_isnt_pushed_again() {
        let asked = zirc_proto::request::Request::open(
            "ada-mbp/1",
            "cc-1",
            "ada",
            "Run the Mac smoke test?",
            "it runs once",
            "it doesn't",
        );
        let taken = zirc_proto::request::Request {
            state: zirc_proto::request::State::Answered {
                answer: zirc_proto::request::Answer::Yes,
                emoji: String::new(),
                at: "2026-11-18T18:57:15Z".into(),
                recommended: true,
            },
            ..asked
        };
        let body = zirc_proto::encode_up_to(&zirc_proto::Message::new("cc-1", taken.event()), 4096)
            .unwrap();
        assert!(body.contains("ada can still"), "it names them: {body}");
        assert_eq!(
            why(
                &NotifySettings::default(),
                NOW,
                "ada",
                &said("#decisions", &body)
            ),
            None
        );
    }

    #[test]
    fn reads_a_zirc_posts_words() {
        let post = zirc_proto::encode(&zirc_proto::Message::new(
            "cc-1",
            zirc_proto::Event::Text {
                body: "done: ada's login fix".into(),
            },
        ))
        .unwrap();
        assert_eq!(
            why(&NotifySettings::default(), NOW, "ada", &said("#api", &post)),
            Some(Why::Mention)
        );
    }
}
