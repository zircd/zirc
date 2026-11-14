//! Sending a zirc event to a channel, the one way zircd and zirc's bots do it.
//!
//! An event that fits an IRC line goes as one. One too long for a line, or a post of
//! several lines, goes whole as one multiline message, where the server takes them: a
//! post's lines then show as lines in any client. Otherwise it's split or shortened into
//! events that each fit a line ([`fitting`]), a post of several lines into one each, as
//! servers without multiline need. Readers that predate multiline can't read the whole
//! message, but they get its lines.

use zirc_irc::Client;
use zirc_proto::{Event, Message};

/// How an event goes to IRC: its `PRIVMSG` bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bodies {
    /// One multiline message holding the whole event.
    Multiline(String),
    /// Lines that each fit, in order: one for an event that fits a line.
    Lines(Vec<String>),
}

/// How `session`'s `event` goes to a server that takes multiline messages up to `multiline`
/// bytes, or none at all.
#[must_use]
pub fn bodies(
    session: &str,
    event: Event,
    multiline: Option<zirc_irc::multiline::Limits>,
) -> Bodies {
    bodies_of(&Message::new(session, event), multiline)
}

/// The same for a whole `message`, as one an integration sends with its `app` label.
#[must_use]
pub fn bodies_of(message: &Message, multiline: Option<zirc_irc::multiline::Limits>) -> Bodies {
    let lines = matches!(&message.event, Event::Text { body } if body.trim().contains('\n'));
    // An integration's event of several lines, CI's details below its line: its lines as
    // lines too where multiline is taken, but whole otherwise, as its data goes on one part.
    let custom_lines =
        matches!(&message.event, Event::Custom { text, .. } if text.trim().contains('\n'));
    if let Some(limits) = multiline {
        let body = if lines || custom_lines {
            zirc_proto::encode_lines_up_to(message, limits.max_bytes)
        } else if zirc_proto::encode(message).is_err() {
            zirc_proto::encode_up_to(message, limits.max_bytes)
        } else {
            Err(zirc_proto::Error::TooLong(0))
        };
        // Within the server's lines too, as the batch would be sent.
        if let Ok(body) = body
            && zirc_irc::multiline::batch("b", "#c", &body, limits).is_some()
        {
            return Bodies::Multiline(body);
        }
    }
    match &message.event {
        // A message a line, without multiline: each reads as itself.
        Event::Text { body } if lines => Bodies::Lines(
            body.trim()
                .lines()
                .filter(|line| !line.trim().is_empty())
                .flat_map(|line| {
                    let line = Event::Text {
                        body: line.trim_end().to_owned(),
                    };
                    encode_as(message, line)
                })
                .collect(),
        ),
        event => Bodies::Lines(encode_as(message, event.clone())),
    }
}

/// Sends `session`'s `event` to `channel` as [`bodies`] says for `client`'s server.
///
/// # Errors
///
/// As [`Client::privmsg`].
pub fn send(
    client: &Client,
    channel: &str,
    session: &str,
    event: Event,
) -> Result<(), zirc_irc::Error> {
    send_message(client, channel, &Message::new(session, event))
}

/// The same for a whole `message`. zirc-test-ergo's `stage` module has a shorter copy of
/// this routing for other languages' test harnesses, as it can't link this crate: change
/// the two together. One that's debug, only true now
/// ([`Message::is_live`]), such as a working set, a lease or CI saying the Apple apps compile,
/// goes as a `TAGMSG`, out of the channel's history ([`crate::presence::working_line`]).
///
/// # Errors
///
/// As [`Client::privmsg`].
pub fn send_message(
    client: &Client,
    channel: &str,
    message: &Message,
) -> Result<(), zirc_irc::Error> {
    send_reply(client, channel, message, None)
}

/// The same, as a reply to the message `reply` names by its id (`+draft/reply`), when it
/// does: the tag goes on the first line, or the multiline batch's opening one, as apps send
/// a reply. A live message is never a reply.
///
/// # Errors
///
/// As [`Client::privmsg`].
pub fn send_reply(
    client: &Client,
    channel: &str,
    message: &Message,
    reply: Option<&str>,
) -> Result<(), zirc_irc::Error> {
    if message.is_live() {
        return crate::presence::working_line(channel, message)
            .map_or(Ok(()), |line| client.send(line));
    }
    let tags = || {
        reply
            .map(|id| vec![("+draft/reply".to_owned(), Some(id.to_owned()))])
            .unwrap_or_default()
    };
    match bodies_of(message, client.multiline()) {
        Bodies::Multiline(body) => client.privmsg_multiline_tagged(channel, &body, tags()),
        Bodies::Lines(lines) => lines.iter().enumerate().try_for_each(|(i, body)| {
            let tags = if i == 0 { tags() } else { Vec::new() };
            client.privmsg_tagged(channel, body, tags)
        }),
    }
}

/// The PRIVMSG bodies for an event. IRC lines are short, so an event too long for one is
/// split or shortened rather than dropped: a working set or a post across several lines,
/// a push down to its newest commit, subjects and summaries cut short with `…`.
#[must_use]
pub fn encode(session: &str, event: Event) -> Vec<String> {
    encode_as(&Message::new(session, event.clone()), event)
}

/// The same for `event` sent as `sender`'s, with its session and `app` label.
fn encode_as(sender: &Message, event: Event) -> Vec<String> {
    fitting_as(sender, event)
        .into_iter()
        .filter_map(|event| zirc_proto::encode(&from(sender, event)).ok())
        .collect()
}

/// `event` as `sender`'s.
fn from(sender: &Message, event: Event) -> Message {
    Message {
        event,
        ..sender.clone()
    }
}

/// `event` as events that each fit a line: itself, or split or shortened as [`encode`]
/// says.
#[must_use]
pub fn fitting(session: &str, event: Event) -> Vec<Event> {
    fitting_as(&Message::new(session, event.clone()), event)
}

/// The same, for `event` sent as `sender` says, with its name and app label.
#[must_use]
pub fn fitting_as(sender: &Message, event: Event) -> Vec<Event> {
    match zirc_proto::encode(&from(sender, event.clone())) {
        Ok(_) => vec![event],
        Err(err) => match smaller(sender, event) {
            Smaller::Split(parts) => parts
                .into_iter()
                .flat_map(|part| fitting_as(sender, part))
                .collect(),
            Smaller::Shorter(event) => fitting_as(sender, event),
            Smaller::Nothing => {
                tracing::warn!("dropping an event: {err}");
                Vec::new()
            }
        },
    }
}

/// A way to make an event that's too long for a line fit.
enum Smaller {
    /// Several events, in order, that together say the same.
    Split(Vec<Event>),
    /// The same event, saying less.
    Shorter(Event),
    /// Nothing left to cut.
    Nothing,
}

/// The shortest text worth keeping before giving up on cutting it further.
const SHORTEST: usize = 16;

/// A working set made smaller: its paths and committed paths halved as one list, the paths
/// first, or its one path shortened.
fn smaller_working(event: Event) -> Smaller {
    match event {
        Event::Working {
            paths,
            continued,
            worktree,
            branch,
            part,
            base,
            committed,
        } if paths.len() + committed.len() > 1 => {
            let half = paths.len().midpoint(committed.len());
            let (first, second) = paths.split_at(half.min(paths.len()));
            let (first_committed, second_committed) =
                committed.split_at(half.saturating_sub(paths.len()));
            Smaller::Split(vec![
                Event::Working {
                    paths: first.to_vec(),
                    continued,
                    worktree: worktree.clone(),
                    branch: branch.clone(),
                    part,
                    base: base.clone(),
                    committed: first_committed.to_vec(),
                },
                Event::Working {
                    paths: second.to_vec(),
                    continued: true,
                    worktree,
                    branch,
                    part,
                    base,
                    committed: second_committed.to_vec(),
                },
            ])
        }
        // One path too long for a line beside its worktree's and branch's names: its
        // directory, which still overlaps whatever is changed in it. Dropping it would leave
        // its set a part short, which readers ignore whole.
        Event::Working {
            mut paths,
            continued,
            worktree,
            branch,
            part,
            base,
            mut committed,
        } if paths
            .iter()
            .chain(&committed)
            .any(|path| path.trim_end_matches('/').contains('/')) =>
        {
            for path in paths.iter_mut().chain(&mut committed) {
                let whole = path.trim_end_matches('/');
                *path = format!("{}/", &whole[..whole.rfind('/').unwrap_or_default()]);
            }
            Smaller::Shorter(Event::Working {
                paths,
                continued,
                worktree,
                branch,
                part,
                base,
                committed,
            })
        }
        _ => Smaller::Nothing,
    }
}

fn smaller(sender: &Message, event: Event) -> Smaller {
    match event {
        working @ Event::Working { .. } => smaller_working(working),
        Event::Text { body } if body.chars().count() > 1 => {
            let (first, second) = halves(&body);
            Smaller::Split(vec![
                Event::Text { body: first },
                Event::Text { body: second },
            ])
        }
        Event::Answer { query, to, text } if text.chars().count() > 1 => {
            smaller_answer(query, to, &text)
        }
        push @ Event::Push { .. } => smaller_push(push),
        // A commit's paths go first, as a push's do, then its subject is shortened.
        Event::Commit {
            branch,
            sha,
            subject,
            paths,
            repo,
        } if !paths.is_empty() => Smaller::Shorter(Event::Commit {
            branch,
            sha,
            subject,
            paths: Vec::new(),
            repo,
        }),
        Event::Commit {
            branch,
            sha,
            subject,
            paths,
            repo,
        } if subject.chars().count() > SHORTEST => Smaller::Shorter(Event::Commit {
            branch,
            sha,
            subject: shortened(&subject),
            paths,
            repo,
        }),
        // The link last: without it, apps still show the commit.
        Event::Commit {
            branch,
            sha,
            subject,
            paths,
            repo: Some(_),
        } => Smaller::Shorter(Event::Commit {
            branch,
            sha,
            subject,
            paths,
            repo: None,
        }),
        Event::File(file) => smaller_file(file),
        custom @ Event::Custom { .. } => smaller_custom(sender, custom),
        intent @ Event::Intent { .. } => smaller_intent(intent),
        _ => Smaller::Nothing,
    }
}

/// A push saying less: without its lines, then its paths, which are only for telling who may
/// have a conflict ahead or need to rebase; then with fewer and shorter subjects.
fn smaller_push(mut push: Event) -> Smaller {
    let Event::Push {
        subjects,
        paths,
        lines,
        repo,
        ..
    } = &mut push
    else {
        return Smaller::Nothing;
    };
    if !lines.is_empty() {
        lines.clear();
    } else if !paths.is_empty() {
        paths.clear();
    } else if subjects.len() > 1 {
        subjects.drain(..subjects.len() - 1);
    } else if let Some(subject) = subjects.first_mut()
        && subject.chars().count() > SHORTEST
    {
        *subject = shortened(subject);
    } else if !subjects.is_empty() {
        subjects.clear();
    } else if repo.take().is_none() {
        return Smaller::Nothing;
    }
    Smaller::Shorter(push)
}

/// A way to make a shared file fit. Its name is only for people: it's shortened, keeping its
/// extension. Still too long, with a long organisation's name or media type, it goes as its
/// link, which every client shows, rather than not at all.
fn smaller_file(mut file: zirc_proto::File) -> Smaller {
    if file.name.chars().count() > SHORTEST {
        file.name = shortened_name(&file.name);
        return Smaller::Shorter(Event::File(file));
    }
    Smaller::Shorter(Event::Text {
        body: Event::File(file).human_text(),
    })
}

/// A way to make an intent fit. Its paths are what others check theirs against: its
/// summary goes first.
fn smaller_intent(event: Event) -> Smaller {
    match event {
        Event::Intent {
            summary,
            paths,
            expires,
            lasts,
        } if summary.chars().count() > SHORTEST => Smaller::Shorter(Event::Intent {
            summary: shortened(&summary),
            paths,
            expires,
            lasts,
        }),
        Event::Intent {
            summary,
            mut paths,
            expires,
            lasts,
        } if !paths.is_empty() => {
            paths.truncate(paths.len() / 2);
            Smaller::Shorter(Event::Intent {
                summary,
                paths,
                expires,
                lasts,
            })
        }
        _ => Smaller::Nothing,
    }
}

/// A way to make a custom event fit. The text is what people and mentions read, so it's
/// kept whole without the data when that fits; otherwise it's halved, the data with the
/// first half while it fits there, then nowhere.
fn smaller_custom(sender: &Message, event: Event) -> Smaller {
    let Event::Custom { kind, data, text } = event else {
        return Smaller::Nothing;
    };
    let bare = Event::Custom {
        kind: kind.clone(),
        data: serde_json::Value::Null,
        text: text.clone(),
    };
    if !data.is_null() && zirc_proto::encode(&from(sender, bare.clone())).is_ok() {
        return Smaller::Shorter(bare);
    }
    if text.chars().count() > 1 {
        let (first, second) = halves(&text);
        return Smaller::Split(vec![
            Event::Custom {
                kind: kind.clone(),
                data,
                text: first,
            },
            Event::Custom {
                kind,
                data: serde_json::Value::Null,
                text: second,
            },
        ]);
    }
    if data.is_null() {
        Smaller::Nothing
    } else {
        Smaller::Shorter(bare)
    }
}

/// `text` cut to two thirds and marked as cut.
fn shortened(text: &str) -> String {
    let keep = text.chars().count() * 2 / 3;
    let mut cut: String = text.chars().take(keep).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// A file's name shortened, as [`shortened`], keeping what follows its last dot when it's
/// short enough to be an extension.
fn shortened_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty() && extension.chars().count() <= 5 && !extension.contains(' ') =>
        {
            format!("{}.{extension}", shortened(stem))
        }
        _ => shortened(name),
    }
}

/// `text` in two, at the space nearest its middle, or at its middle without one.
/// The most text one message of several carries ([`in_parts`]). A zirc message says its text
/// twice, in its data and for plain IRC clients, so this keeps each within one multiline batch
/// of Ergo's 4096 bytes.
pub const PART_BYTES: usize = 1400;

/// `text` in parts of at most `limit` bytes, in order, broken between lines where it can, else
/// at a character: a long reply sent as several messages, each one whole.
#[must_use]
pub fn in_parts(text: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    for line in text.split_inclusive('\n') {
        if part.len() + line.len() > limit && !part.is_empty() {
            parts.push(std::mem::take(&mut part).trim_end().to_owned());
        }
        let mut line = line;
        while line.len() > limit {
            let mut at = limit;
            while !line.is_char_boundary(at) {
                at -= 1;
            }
            parts.push(line[..at].to_owned());
            line = &line[at..];
        }
        part.push_str(line);
    }
    if !part.trim().is_empty() || parts.is_empty() {
        parts.push(part.trim_end().to_owned());
    }
    parts
}

/// An answer in parts, each answering the same query, as a command's progress does: dropped,
/// its asker would wait for it in vain.
fn smaller_answer(query: String, to: Option<String>, text: &str) -> Smaller {
    let (first, second) = halves(text);
    Smaller::Split(vec![
        Event::Answer {
            query: query.clone(),
            to: to.clone(),
            text: first,
        },
        Event::Answer {
            query,
            to,
            text: second,
        },
    ])
}

fn halves(text: &str) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let middle = chars.len() / 2;
    let split = (0..middle)
        .flat_map(|offset| [middle - offset, middle + offset])
        .find(|&at| at > 0 && at < chars.len() && chars[at] == ' ')
        .unwrap_or(middle);
    let first: String = chars[..split].iter().collect();
    let second: String = chars[split..].iter().collect();
    (first.trim_end().to_owned(), second.trim_start().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_file_names_are_shortened_keeping_their_extension() {
        let file = |name: &str, mime: &str| {
            Event::File(zirc_proto::File {
                url: "https://staging.zirc.dev/m/a-longer-organisation/7Hq2xV9kLmN3pR8".into(),
                name: name.into(),
                mime: mime.into(),
                size: 2_345_678,
                width: None,
                height: None,
                duration_ms: None,
            })
        };
        let name = "a quarterly report with a very long name, final version 3 (copy).docx";
        let events = decoded(&encode("desktop", file(name, "application/pdf")));
        let [Event::File(sent)] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert!(sent.name.ends_with("….docx"), "{}", sent.name);
        assert!(sent.name.starts_with("a quarterly"), "{}", sent.name);
        let docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        let events = decoded(&encode("desktop", file(name, docx)));
        let [Event::Text { body }] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert!(body.ends_with("/7Hq2xV9kLmN3pR8"), "{body}");
    }

    #[test]
    fn a_long_text_goes_in_parts_between_lines_each_within_the_limit() {
        let text = format!("{}\n{}\n{}", "a".repeat(8), "b".repeat(8), "c".repeat(25));
        assert_eq!(
            in_parts(&text, 20),
            ["aaaaaaaa\nbbbbbbbb", "cccccccccccccccccccc", "ccccc"]
        );
        assert_eq!(in_parts("short", 20), ["short"]);
        assert_eq!(in_parts("", 20), [""]);
        let wide = "é".repeat(15);
        assert!(in_parts(&wide, 7).iter().all(|part| part.len() <= 7));
        assert_eq!(in_parts(&wide, 7).concat(), wide);
    }

    #[test]
    fn an_answer_too_long_for_one_line_goes_in_parts_answering_the_same_query() {
        let text = "the landing is testing ".repeat(60);
        let events = decoded(&encode(
            "remote",
            Event::Answer {
                query: "q1".into(),
                to: Some("cc-1".into()),
                text: text.clone(),
            },
        ));
        assert!(events.len() > 1, "{events:?}");
        let mut said = Vec::new();
        for event in events {
            let Event::Answer { query, to, text } = event else {
                panic!("not an answer");
            };
            assert_eq!((query.as_str(), to.as_deref()), ("q1", Some("cc-1")));
            said.push(text);
        }
        assert_eq!(said.join(" ").trim_end(), text.trim_end());
    }

    #[test]
    fn splits_working_sets_too_long_for_one_line() {
        let paths: Vec<String> = (0..40).map(|i| format!("src/module_{i}/file.rs")).collect();
        let bodies = encode(
            "watcher",
            Event::Working {
                paths: paths.clone(),
                continued: false,
                worktree: None,
                branch: None,
                part: None,
                base: None,
                committed: Vec::new(),
            },
        );
        assert!(bodies.len() > 1);
        let mut decoded = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            let message = zirc_proto::decode(body).unwrap().unwrap();
            let Event::Working {
                paths, continued, ..
            } = message.event
            else {
                panic!("not a working set")
            };
            // Only the first part replaces; the rest add to it.
            assert_eq!(continued, i > 0);
            decoded.extend(paths);
        }
        assert_eq!(decoded, paths);
    }

    #[test]
    fn splits_a_working_sets_committed_paths_with_its_base_on_every_part() {
        let paths: Vec<String> = (0..3).map(|i| format!("src/dirty_{i}.rs")).collect();
        let committed: Vec<String> = (0..40).map(|i| format!("src/module_{i}/file.rs")).collect();
        let events = decoded(&encode(
            "cc-1",
            Event::Working {
                paths: paths.clone(),
                continued: false,
                worktree: Some("one".into()),
                branch: Some("login".into()),
                part: None,
                base: Some("b".repeat(40)),
                committed: committed.clone(),
            },
        ));
        assert!(events.len() > 1);
        let (mut all_paths, mut all_committed) = (Vec::new(), Vec::new());
        for (i, event) in events.into_iter().enumerate() {
            let Event::Working {
                paths,
                continued,
                base,
                committed,
                ..
            } = event
            else {
                panic!("not a working set")
            };
            assert_eq!(continued, i > 0);
            assert_eq!(base, Some("b".repeat(40)));
            all_paths.extend(paths);
            all_committed.extend(committed);
        }
        assert_eq!((all_paths, all_committed), (paths, committed));
    }

    #[test]
    fn shortens_a_path_too_long_for_a_line_to_its_directory() {
        let long = format!(
            "apps/android/app/src/main/{}/Screen.kt",
            "kotlin/dev/zirc".repeat(12)
        );
        let bodies = encode(
            "cc-android",
            Event::Working {
                paths: vec![long.clone()],
                continued: true,
                worktree: Some("wizardly-bell-f60983".into()),
                branch: Some("android-polish".into()),
                part: Some([2, 3]),
                base: None,
                committed: Vec::new(),
            },
        );
        let events = decoded(&bodies);
        let [Event::Working { paths, part, .. }] = events.as_slice() else {
            panic!("expected one working set, got {events:?}")
        };
        assert_eq!(*part, Some([2, 3]));
        // A directory the file is in, so it still overlaps with the file.
        assert!(
            paths[0].ends_with('/') && long.starts_with(&paths[0]),
            "{paths:?}"
        );
        assert!(crate::presence::touches(&paths[0], &long));
    }

    fn decoded(bodies: &[String]) -> Vec<Event> {
        bodies
            .iter()
            .map(|body| zirc_proto::decode(body).unwrap().unwrap().event)
            .collect()
    }

    #[test]
    fn shortens_pushes_and_commits_rather_than_dropping_them() {
        let long = "a commit subject that goes on and on ".repeat(6);
        let push = Event::Push {
            branch: "main".into(),
            head: "f".repeat(40),
            forced: false,
            subjects: vec![long.clone(), "Fix login".into(), long.clone()],
            paths: vec!["src/login.rs".into()],
            base: Some("e".repeat(40)),
            lines: [("src/login.rs".to_owned(), vec![[3, 2]])].into(),
            repo: None,
        };
        let events = decoded(&encode("watcher", push));
        let [
            Event::Push {
                subjects,
                paths,
                lines,
                ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one push, got {events:?}")
        };
        // The lines and paths go before any subject does.
        assert!(lines.is_empty());
        assert!(paths.is_empty());
        assert_eq!(subjects.len(), 1);
        assert!(subjects[0].ends_with('…'));

        let commit = Event::Commit {
            paths: Vec::new(),
            branch: "main".into(),
            sha: "f".repeat(40),
            subject: long,
            repo: None,
        };
        let events = decoded(&encode("watcher", commit));
        assert!(matches!(&events[..], [Event::Commit { subject, .. }] if subject.ends_with('…')));
    }

    #[test]
    fn splits_long_posts_into_several_in_order() {
        let body = (1..=60)
            .map(|n| format!("word{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let events = decoded(&encode("cc-1", Event::Text { body: body.clone() }));
        assert!(events.len() > 1);
        let rejoined: Vec<String> = events
            .into_iter()
            .map(|event| match event {
                Event::Text { body } => body,
                other => panic!("not a post: {other:?}"),
            })
            .collect();
        assert_eq!(rejoined.join(" "), body);
    }

    #[test]
    fn splits_long_custom_events_with_their_data_first() {
        let text = (1..=40)
            .map(|n| format!("word{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let data = serde_json::json!({"build": "123"});
        let event = Event::Custom {
            kind: zirc_proto::ci::LANDING_KIND.into(),
            data: data.clone(),
            text: text.clone(),
        };
        let events = decoded(&encode("ci", event));
        assert!(events.len() > 1);
        let mut rejoined = Vec::new();
        for (i, event) in events.into_iter().enumerate() {
            let Event::Custom {
                data: got, text, ..
            } = event
            else {
                panic!("not a custom event: {event:?}")
            };
            assert_eq!(
                got,
                if i == 0 {
                    data.clone()
                } else {
                    serde_json::Value::Null
                }
            );
            rejoined.push(text);
        }
        assert_eq!(rejoined.join(" "), text);
    }

    #[test]
    fn keeps_a_custom_events_text_whole_rather_than_its_data() {
        let text = "cc-012345: approve https://buildkite.com/zirc/monorepo/builds/123456: web";
        let event = Event::Custom {
            kind: zirc_proto::ci::DEPLOY_KIND.into(),
            data: serde_json::json!({"pipeline": "monorepo", "build": "123456", "more": "x".repeat(40)}),
            text: text.into(),
        };
        let events = decoded(&encode("ci", event));
        let [
            Event::Custom {
                text: said, data, ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one custom event, got {events:?}")
        };
        assert_eq!(said, text);
        assert!(data.is_null());
    }

    #[test]
    fn sends_too_long_events_whole_where_multiline_is_taken() {
        let event = Event::Text {
            body: "word ".repeat(200),
        };
        let limits = zirc_irc::multiline::Limits {
            max_bytes: 4096,
            max_lines: 100,
        };
        let Bodies::Multiline(body) = bodies("cc-1", event.clone(), Some(limits)) else {
            panic!("not one multiline message")
        };
        assert_eq!(zirc_proto::decode(&body).unwrap().unwrap().event, event);
        assert!(matches!(bodies("cc-1", event, None), Bodies::Lines(lines) if lines.len() > 1));
        let short = Event::Text { body: "hi".into() };
        assert!(
            matches!(bodies("cc-1", short, Some(limits)), Bodies::Lines(lines) if lines.len() == 1)
        );
    }

    #[test]
    fn sends_a_post_of_several_lines_as_one_multiline_message_or_a_message_a_line() {
        let event = Event::Text {
            body: "done: the release\n\n```\ncargo test\n```\n".into(),
        };
        let limits = zirc_irc::multiline::Limits {
            max_bytes: 16384,
            max_lines: 200,
        };
        let Bodies::Multiline(body) = bodies("cc-1", event.clone(), Some(limits)) else {
            panic!("not one multiline message")
        };
        // Its lines as lines, the payload on the first.
        assert_eq!(body.lines().count(), 5, "{body:?}");
        assert_eq!(zirc_proto::decode(&body).unwrap().unwrap().event, event);

        // Without multiline: a message each, blank lines left out.
        let Bodies::Lines(lines) = bodies("cc-1", event.clone(), None) else {
            panic!("not lines")
        };
        assert_eq!(
            decoded(&lines),
            ["done: the release", "```", "cargo test", "```"]
                .map(|body| Event::Text { body: body.into() })
        );
        // Or more lines than the server takes in one.
        let few = zirc_irc::multiline::Limits {
            max_bytes: 16384,
            max_lines: 2,
        };
        assert!(
            matches!(bodies("cc-1", event, Some(few)), Bodies::Lines(lines) if lines.len() == 4)
        );
    }

    #[test]
    fn an_integrations_event_of_several_lines_keeps_them_where_multiline_is_taken() {
        let event = Event::Custom {
            kind: zirc_proto::ci::LANDING_KIND.into(),
            data: serde_json::json!({"build": "7"}),
            text: "cc-1: build 7 failed https://b.test/7: //web:e2e\n- failed: `//web:e2e`".into(),
        };
        let limits = zirc_irc::multiline::Limits {
            max_bytes: 16384,
            max_lines: 200,
        };
        let Bodies::Multiline(body) = bodies("ci", event.clone(), Some(limits)) else {
            panic!("not one multiline message")
        };
        assert_eq!(body.lines().count(), 2, "{body:?}");
        assert_eq!(zirc_proto::decode(&body).unwrap().unwrap().event, event);
        // Without multiline, whole on one line, its data with it.
        let Bodies::Lines(lines) = bodies("ci", event.clone(), None) else {
            panic!("not lines")
        };
        assert_eq!(decoded(&lines), [event]);
    }

    #[test]
    fn an_integrations_label_goes_with_each_part() {
        let message = Message {
            app: Some("ci".into()),
            ..Message::new(
                "deploy",
                Event::Text {
                    body: "word ".repeat(200),
                },
            )
        };
        let Bodies::Lines(lines) = bodies_of(&message, None) else {
            panic!("not lines")
        };
        assert!(lines.len() > 1);
        for line in lines {
            let part = zirc_proto::decode(&line).unwrap().unwrap();
            assert_eq!(part.app.as_deref(), Some("ci"));
        }
    }
}
