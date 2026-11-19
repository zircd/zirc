//! The core path every app's end-to-end test walks, with the teammate's side of it and what
//! the server heard said once, here, so no app's driver writes a line of IRC or of zirc's
//! framing. A driver only works its app, and asks for the rest by name
//! ([`crate::control`], over `serve`'s control port):
//!
//! 1. The person signs in through the stand-in zirc.dev ([`crate::signin`]).
//! 2. They open [`Words::channel`] and see what was said before they came (the step
//!    `history`): a teammate's message, and a push by the teammate's machine.
//! 3. They send a message, which the server heard ([`HeardLog::heard`]).
//! 4. The teammate answers (the step `reply`).
//! 5. They react to that answer, which the server heard too.
//! 6. The server restarts ([`Scenario::restart`]), the app reconnects by itself and is back
//!    in the channel ([`HeardLog::joined`]), and the teammate says something more (the step
//!    `back`).
//!
//! The teammate, [`Words::teammate`], and their machine, [`Words::machine`], are plain
//! connections to the real Ergo. What they say that's zirc's is framed by [`Stager`], and
//! what's heard from the app is read back by zirc-proto.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{Ipv4Addr, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use zirc_line::Line;
use zirc_line::multiline::{self, Batches};
use zirc_line::registration::{Identity, Registration, Step};

use crate::Ergo;
use crate::stage::Stager;

/// How long the server gets to answer a teammate.
const WITHIN: Duration = Duration::from_secs(20);

/// How long [`HeardLog::heard`] waits for what it's asked, when the ask doesn't say.
const HEARD_WITHIN: u64 = 20;

/// IRCv3's reaction to a message, the emoji as the tag's value, on a `TAGMSG` that names the
/// message with [`REPLY`]; and the one taking it back. zirc-chat's `REACT` and `UNREACT`,
/// said again because this crate is linked into every crate's tests and zirc-chat is not to
/// be: change them together.
const REACT: &str = "+draft/react";
const UNREACT: &str = "+draft/unreact";
/// IRCv3's tag naming the message a line answers or reacts to.
const REPLY: &str = "+draft/reply";

/// The steps a driver can ask for, in the order the path plays them.
pub const STEPS: [&str; 3] = ["history", "reply", "back"];

/// The session the teammate's machine speaks as.
const SESSION: &str = "cc-7f3e";

/// The path's fixed names and words: what a driver looks for on its app's screen. A driver
/// asks for them (`GET /words`), so none is typed twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Words {
    /// The channel the path is walked in.
    pub channel: &'static str,
    /// The organisation's own channel, which someone signing in for the first time is put in.
    pub general: &'static str,
    /// The teammate's nick.
    pub teammate: &'static str,
    /// The teammate's machine's nick.
    pub machine: &'static str,
    /// What the teammate said before the person came.
    pub history: &'static str,
    /// What the teammate's machine said it was about to change before the person came: its
    /// routine work, which every app shows as one brief line. An intent, not a push: a commit
    /// or a push shows in a channel only under Show: Everything (19 Nov 2026).
    pub intended: &'static str,
    /// What the teammate answers the person.
    pub reply: &'static str,
    /// What the teammate says once the server is back.
    pub back: &'static str,
}

/// The path's words.
pub const WORDS: Words = Words {
    channel: "#web",
    general: "#general",
    teammate: "bob",
    machine: "bob-laptop",
    history: "morning! the login page is up for review",
    intended: "sign in with a code",
    reply: "welcome aboard, good to have you here",
    back: "and the server is back, as if nothing happened",
};

impl Words {
    /// The words as a shell reads them, a line each: `E2E_CHANNEL='#web'`. For a driver in
    /// shell, which `eval`s them (`GET /words?sh`).
    #[must_use]
    pub fn shell(&self) -> String {
        let fields = serde_json::to_value(self).unwrap_or_default();
        let mut lines = String::new();
        for (name, value) in fields.as_object().into_iter().flatten() {
            let value = value.as_str().unwrap_or_default().replace('\'', "'\\''");
            let _ = writeln!(lines, "E2E_{}='{value}'", name.to_uppercase());
        }
        lines
    }
}

/// One message: what a teammate said in a step, or what the server heard someone say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Said {
    /// The server's id for it (`msgid`).
    pub id: String,
    /// Who said it, by nick.
    pub from: String,
    /// Its text as people read it: a zirc event's is zirc-proto's own words for it.
    pub text: String,
    /// The message it answers, by id, if it's a reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// A reaction that stands: one given and not taken back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reaction {
    /// Who reacted, by nick.
    pub from: String,
    /// The emoji.
    pub emoji: String,
    /// The message reacted to, by id.
    pub to: String,
}

/// What the server heard from someone in a channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Heard {
    /// Their messages, oldest first.
    pub messages: Vec<Said>,
    /// Their reactions that stand, oldest first.
    pub reactions: Vec<Reaction>,
}

/// What to ask [`HeardLog::heard`]: everything `from` said in `channel`, once it includes a
/// message containing `text` and a reaction with `react`, where those are given.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    /// Whose lines, by nick: the person the app signed in as.
    pub from: String,
    /// The channel they were said in.
    pub channel: String,
    /// What one of their messages must contain, if anything.
    #[serde(default)]
    pub text: Option<String>,
    /// The emoji one of their standing reactions must be, if any.
    #[serde(default)]
    pub react: Option<String>,
    /// How many seconds to wait for those: 20 when left out.
    #[serde(default)]
    pub within: Option<u64>,
}

/// Every whole line a connection heard, a multiline message as the one line it is.
type Log = Arc<Mutex<Vec<Line>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The nick a line came from.
fn nick_of(line: &Line) -> &str {
    line.source
        .as_deref()
        .and_then(|source| source.split('!').next())
        .unwrap_or_default()
}

/// An emoji without its variation selector, which one app sends and another leaves out.
fn bare(emoji: &str) -> String {
    emoji.chars().filter(|c| *c != '\u{fe0f}').collect()
}

/// A `PRIVMSG` as the message it is: a zirc event's text is what zirc-proto reads from it.
fn said(line: &Line) -> Said {
    let raw = line.params.get(1).cloned().unwrap_or_default();
    let text = match zirc_proto::decode(&raw) {
        Ok(Some(message)) => message.event.human_text(),
        _ => raw,
    };
    Said {
        id: line.tag("msgid").unwrap_or_default().to_owned(),
        from: nick_of(line).to_owned(),
        text,
        reply_to: line.tag(REPLY).map(str::to_owned),
    }
}

/// What the teammate's connection heard, which a driver asks about while the scenario is
/// busy or idle alike.
#[derive(Clone)]
pub struct HeardLog {
    lines: Log,
    /// How much had been heard when the teammate last arrived: what's after it was heard
    /// from the server as it runs now.
    arrived: Arc<Mutex<usize>>,
}

/// What to ask [`HeardLog::joined`]: whether `nick` is in `channel`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Whether {
    /// The person the app signed in as, by nick.
    pub nick: String,
    /// The channel.
    pub channel: String,
    /// How many seconds to wait for them: 20 when left out.
    #[serde(default)]
    pub within: Option<u64>,
}

impl HeardLog {
    /// Whether `ask.nick` is in `ask.channel` on the server as it runs now, as the teammate
    /// there saw: they joined since the teammate arrived, or were among its names then.
    /// After a restart, that the app is back.
    ///
    /// # Errors
    ///
    /// When they still aren't after `ask.within` seconds.
    pub fn joined(&self, ask: &Whether) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(ask.within.unwrap_or(HEARD_WITHIN));
        let here = |line: &Line, at: usize| {
            line.params
                .get(at)
                .is_some_and(|channel| channel.eq_ignore_ascii_case(&ask.channel))
        };
        loop {
            let arrived = *lock(&self.arrived);
            let there =
                lock(&self.lines)
                    .iter()
                    .skip(arrived)
                    .any(|line| match line.command.as_str() {
                        "JOIN" => here(line, 0) && nick_of(line).eq_ignore_ascii_case(&ask.nick),
                        // The channel's names, each with the marks of what they are there.
                        "353" => {
                            here(line, 2)
                                && line.params.get(3).is_some_and(|names| {
                                    names.split(' ').any(|name| {
                                        name.trim_start_matches(['~', '&', '@', '%', '+'])
                                            .eq_ignore_ascii_case(&ask.nick)
                                    })
                                })
                        }
                        _ => false,
                    });
            if there {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("{} isn't in {}", ask.nick, ask.channel));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// What `ask.from` said in `ask.channel` so far.
    fn so_far(&self, ask: &Ask) -> Heard {
        let mut heard = Heard::default();
        for line in lock(&self.lines).iter() {
            let here = line
                .params
                .first()
                .is_some_and(|target| target.eq_ignore_ascii_case(&ask.channel));
            if !here || !nick_of(line).eq_ignore_ascii_case(&ask.from) {
                continue;
            }
            let to = line.tag(REPLY).unwrap_or_default();
            match line.command.as_str() {
                "PRIVMSG" => heard.messages.push(said(line)),
                "TAGMSG" => {
                    if let Some(emoji) = line.tag(REACT) {
                        heard.reactions.push(Reaction {
                            from: nick_of(line).to_owned(),
                            emoji: emoji.to_owned(),
                            to: to.to_owned(),
                        });
                    } else if let Some(emoji) = line.tag(UNREACT) {
                        heard.reactions.retain(|reaction| {
                            reaction.to != to || bare(&reaction.emoji) != bare(emoji)
                        });
                    }
                }
                _ => {}
            }
        }
        heard
    }

    /// What `ask.from` said in `ask.channel`, once it has what `ask` wants.
    ///
    /// # Errors
    ///
    /// What they said so far, when it still lacks that after `ask.within` seconds.
    pub fn heard(&self, ask: &Ask) -> Result<Heard, Heard> {
        let deadline = Instant::now() + Duration::from_secs(ask.within.unwrap_or(HEARD_WITHIN));
        loop {
            let heard = self.so_far(ask);
            let text = ask.text.as_deref().is_none_or(|text| {
                heard
                    .messages
                    .iter()
                    .any(|message| message.text.contains(text))
            });
            let react = ask.react.as_deref().is_none_or(|emoji| {
                heard
                    .reactions
                    .iter()
                    .any(|reaction| bare(&reaction.emoji) == bare(emoji))
            });
            if text && react {
                return Ok(heard);
            }
            if Instant::now() >= deadline {
                return Err(heard);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A teammate's connection: a thread keeps what it hears in its log and answers the server's
/// pings, however long an app takes.
struct Talker {
    nick: String,
    write: TcpStream,
    log: Log,
}

impl Talker {
    /// Connects to the server on `port` as `nick`, whose real name is `realname` (a machine's
    /// is its owner), with the capabilities a zirc client has, hearing into `log`.
    fn connect(port: u16, nick: &str, realname: &str, log: &Log) -> Result<Self, String> {
        let failed = |error: std::io::Error| format!("{nick} connecting: {error}");
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).map_err(failed)?;
        stream.set_read_timeout(Some(WITHIN)).map_err(failed)?;
        let mut write = stream.try_clone().map_err(failed)?;
        let caps = [
            "batch",
            "echo-message",
            "message-tags",
            "server-time",
            multiline::CAP,
        ]
        .map(String::from);
        let mut identity = Identity::new(nick, caps.to_vec());
        realname.clone_into(&mut identity.realname);
        let mut registration = Registration::new(identity);
        let send = |write: &mut TcpStream, lines: &[Line]| {
            lines
                .iter()
                .try_for_each(|line| write!(write, "{line}\r\n"))
        };
        send(&mut write, &registration.opening()).map_err(failed)?;
        let mut read = BufReader::new(stream);
        let registered = loop {
            let mut text = String::new();
            if read.read_line(&mut text).map_err(failed)? == 0 {
                return Err(format!("{nick}: the server closed the connection"));
            }
            let Ok(line) = text.parse::<Line>() else {
                continue;
            };
            match registration
                .step(&line)
                .map_err(|error| format!("{nick} registering: {error}"))?
            {
                Step::Reply(lines) => send(&mut write, &lines).map_err(failed)?,
                Step::Registered(registered) => break registered,
            }
        };
        read.get_ref().set_read_timeout(None).map_err(failed)?;
        let mut pong = write.try_clone().map_err(failed)?;
        let heard = Arc::clone(log);
        std::thread::spawn(move || {
            let mut batches = Batches::default();
            for text in read.lines() {
                let Ok(text) = text else { break };
                let Ok(line) = text.parse::<Line>() else {
                    continue;
                };
                if line.command == "PING" {
                    let _ = write!(pong, "{}\r\n", Line::new("PONG", line.params.clone()));
                    continue;
                }
                if let Some(whole) = batches.feed(line) {
                    lock(&heard).push(whole);
                }
            }
        });
        Ok(Self {
            nick: registered.nick,
            write,
            log: Arc::clone(log),
        })
    }

    fn send(&mut self, line: &Line) -> Result<(), String> {
        write!(self.write, "{line}\r\n").map_err(|error| format!("{} sending: {error}", self.nick))
    }

    /// How much this connection has heard: where to look from for an answer to what's sent
    /// next.
    fn mark(&self) -> usize {
        lock(&self.log).len()
    }

    /// The first line heard since `mark` that `wanted` says yes to.
    fn hears(
        &self,
        mark: usize,
        what: &str,
        wanted: impl Fn(&Line) -> bool,
    ) -> Result<Line, String> {
        let deadline = Instant::now() + WITHIN;
        loop {
            if let Some(line) = lock(&self.log).iter().skip(mark).find(|line| wanted(line)) {
                return Ok(line.clone());
            }
            if Instant::now() >= deadline {
                return Err(format!("{}: {what} never came", self.nick));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Joins `channel`, and is in it once this returns.
    fn join(&mut self, channel: &str) -> Result<(), String> {
        let mark = self.mark();
        self.send(&Line::new("JOIN", [channel]))?;
        // The end of its names: the server's last word on a join.
        self.hears(mark, &format!("joining {channel}"), |line| {
            line.command == "366"
                && line
                    .params
                    .get(1)
                    .is_some_and(|joined| joined.eq_ignore_ascii_case(channel))
        })?;
        Ok(())
    }

    /// Says `lines`, one message, to `target`, and gives it as the server echoed it: with
    /// its id.
    fn says(&mut self, target: &str, lines: &[Line]) -> Result<Said, String> {
        let mark = self.mark();
        for line in lines {
            self.send(line)?;
        }
        let nick = self.nick.clone();
        let echo = self.hears(mark, "its own message's echo", |line| {
            line.command == "PRIVMSG"
                && nick_of(line) == nick
                && line.params.first().is_some_and(|to| to == target)
        })?;
        Ok(said(&echo))
    }
}

/// The path's teammates on one throwaway Ergo, which it owns: see the [module](self).
pub struct Scenario {
    /// The server, until [`Self::stop`].
    ergo: Option<Ergo>,
    port: u16,
    ws_port: u16,
    stager: Stager,
    teammate: Option<Talker>,
    machine: Option<Talker>,
    /// What the teammate's connections heard, this one and those before a restart.
    heard: Log,
    /// How much of it there was when the teammate last arrived.
    arrived: Arc<Mutex<usize>>,
    /// What the machine's heard, which is the same and so isn't asked about.
    machine_heard: Log,
    /// What each step played said, for a driver that asks for it again.
    played: BTreeMap<&'static str, Vec<Said>>,
}

impl Scenario {
    /// The path on `ergo`. Nobody connects until the first step is asked for.
    #[must_use]
    pub fn new(ergo: Ergo) -> Self {
        let (port, ws_port) = (ergo.port(), ergo.ws_port());
        Self {
            ergo: Some(ergo),
            port,
            ws_port,
            stager: Stager::for_server(port),
            teammate: None,
            machine: None,
            heard: Log::default(),
            arrived: Arc::default(),
            machine_heard: Log::default(),
            played: BTreeMap::new(),
        }
    }

    /// The server's plaintext IRC port on 127.0.0.1.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The server's WebSocket port on 127.0.0.1.
    #[must_use]
    pub fn ws_port(&self) -> u16 {
        self.ws_port
    }

    /// What the server heard, to ask about without holding the scenario.
    #[must_use]
    pub fn heard(&self) -> HeardLog {
        HeardLog {
            lines: Arc::clone(&self.heard),
            arrived: Arc::clone(&self.arrived),
        }
    }

    /// Stops the server, as dropping the scenario does: for a process about to end with the
    /// scenario still shared.
    pub fn stop(&mut self) {
        self.teammate = None;
        self.machine = None;
        self.ergo = None;
    }

    /// Restarts the server, as one restarting for an upgrade does ([`Ergo::restart`]): every
    /// connection ends, the app's too, and the teammates who were there come back to their
    /// channels. Safe to ask again: it restarts again.
    ///
    /// # Errors
    ///
    /// When the server was stopped, or a teammate can't come back.
    pub fn restart(&mut self) -> Result<(), String> {
        let ergo = self.ergo.as_mut().ok_or("the server was stopped")?;
        ergo.restart();
        if self.teammate.take().is_some() {
            self.arrive()?;
        }
        Ok(())
    }

    /// The teammate and their machine, connected and in their channels.
    fn arrive(&mut self) -> Result<(), String> {
        *lock(&self.arrived) = lock(&self.heard).len();
        let mut teammate = Talker::connect(self.port, WORDS.teammate, WORDS.teammate, &self.heard)?;
        teammate.join(WORDS.general)?;
        teammate.join(WORDS.channel)?;
        // A machine's real name is its owner.
        let mut machine = Talker::connect(
            self.port,
            WORDS.machine,
            WORDS.teammate,
            &self.machine_heard,
        )?;
        machine.join(WORDS.channel)?;
        self.teammate = Some(teammate);
        self.machine = Some(machine);
        Ok(())
    }

    /// Plays the step `name`, one of [`STEPS`], and gives what the teammates said in it,
    /// oldest first. A step is played once: asked for again, it answers what it said then.
    ///
    /// - `history`: the teammate and their machine arrive, and in [`Words::channel`] the
    ///   teammate says [`Words::history`] and the machine says it's about to change
    ///   [`Words::intended`].
    /// - `reply`: the teammate says [`Words::reply`].
    /// - `back`: the teammate says [`Words::back`].
    ///
    /// # Errors
    ///
    /// A step by another name, one asked for before `history`, or the server not answering.
    pub fn play(&mut self, name: &str) -> Result<Vec<Said>, String> {
        let Some(step) = STEPS.into_iter().find(|step| *step == name) else {
            return Err(format!(
                "no step called {name}: there are {}",
                STEPS.join(", ")
            ));
        };
        if let Some(said) = self.played.get(step) {
            return Ok(said.clone());
        }
        let said = match step {
            "history" => self.history()?,
            "reply" => vec![self.teammate_says(WORDS.reply)?],
            _ => vec![self.teammate_says(WORDS.back)?],
        };
        self.played.insert(step, said.clone());
        Ok(said)
    }

    fn history(&mut self) -> Result<Vec<Said>, String> {
        self.arrive()?;
        let topic = Line::new("TOPIC", [WORDS.channel, "The web app"]);
        if let Some(teammate) = &mut self.teammate {
            teammate.send(&topic)?;
        }
        let morning = self.teammate_says(WORDS.history)?;
        let intent = json!({ "type": "intent", "summary": WORDS.intended, "paths": [] });
        let lines = self.stager.staged(WORDS.channel, SESSION, &intent)?;
        let machine = self.machine.as_mut().ok_or("the machine isn't here")?;
        let intended = machine.says(WORDS.channel, &lines)?;
        Ok(vec![morning, intended])
    }

    fn teammate_says(&mut self, text: &str) -> Result<Said, String> {
        let teammate = self
            .teammate
            .as_mut()
            .ok_or("the teammate isn't here yet: play history first")?;
        teammate.says(
            WORDS.channel,
            &[Line::new("PRIVMSG", [WORDS.channel, text])],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(lines: &[&str]) -> HeardLog {
        HeardLog {
            lines: Arc::new(Mutex::new(
                lines.iter().map(|line| line.parse().unwrap()).collect(),
            )),
            arrived: Arc::default(),
        }
    }

    fn ask(text: Option<&str>, react: Option<&str>) -> Ask {
        Ask {
            from: "dana".to_owned(),
            channel: "#web".to_owned(),
            text: text.map(str::to_owned),
            react: react.map(str::to_owned),
            within: Some(0),
        }
    }

    #[test]
    fn hears_one_persons_messages_in_one_channel() {
        let log = heard(&[
            "@msgid=a :dana!d@h PRIVMSG #web :hello there",
            "@msgid=b :bob!b@h PRIVMSG #web :hello dana",
            "@msgid=c :dana!d@h PRIVMSG #api :elsewhere",
            "@msgid=d;+draft/reply=b :Dana!d@h PRIVMSG #WEB :thanks",
        ]);
        let all = log.heard(&ask(None, None)).unwrap();
        let texts: Vec<&str> = all.messages.iter().map(|said| said.text.as_str()).collect();
        assert_eq!(texts, ["hello there", "thanks"]);
        assert_eq!(all.messages[1].reply_to.as_deref(), Some("b"));
        assert!(log.heard(&ask(Some("hello"), None)).is_ok());
        // What was never said is answered with what was.
        let lacking = log.heard(&ask(Some("goodbye"), None)).unwrap_err();
        assert_eq!(lacking.messages.len(), 2);
    }

    #[test]
    fn a_reaction_stands_until_its_taken_back() {
        let react = "@+draft/react=👍;+draft/reply=b :dana!d@h TAGMSG #web";
        let log = heard(&[react]);
        let stands = log.heard(&ask(None, Some("👍\u{fe0f}"))).unwrap();
        assert_eq!(
            stands.reactions,
            [Reaction {
                from: "dana".to_owned(),
                emoji: "👍".to_owned(),
                to: "b".to_owned()
            }]
        );
        let log = heard(&[
            react,
            "@+draft/unreact=👍;+draft/reply=b :dana!d@h TAGMSG #web",
        ]);
        assert!(log.heard(&ask(None, Some("👍"))).is_err());
    }

    #[test]
    fn someone_is_in_a_channel_by_their_join_or_its_names() {
        let whether = |nick: &str| Whether {
            nick: nick.to_owned(),
            channel: "#web".to_owned(),
            within: Some(0),
        };
        let log = heard(&[
            ":irc.test 353 bob = #web :@carol +dana",
            ":erin!e@h JOIN #web",
            ":frank!f@h JOIN #api",
        ]);
        assert!(log.joined(&whether("dana")).is_ok());
        assert!(log.joined(&whether("Erin")).is_ok());
        assert!(log.joined(&whether("frank")).is_err());
        // What was heard before the teammate last arrived was another server's word.
        *lock(&log.arrived) = 3;
        assert!(log.joined(&whether("dana")).is_err());
    }

    #[test]
    fn a_zirc_events_text_is_zirc_protos() {
        let lines = Stager::with_limits(multiline::Limits {
            max_bytes: 4096,
            max_lines: 100,
        })
        .staged(
            "#web",
            "cc-1",
            &json!({ "type": "text", "body": "the form posts twice" }),
        )
        .unwrap();
        let mut line = lines[0].clone();
        line.source = Some("dana!d@h".to_owned());
        assert_eq!(said(&line).text, "the form posts twice");
    }

    #[test]
    fn a_shell_reads_the_words() {
        let shell = WORDS.shell();
        assert!(shell.contains("E2E_CHANNEL='#web'\n"), "{shell}");
        assert!(shell.contains("E2E_TEAMMATE='bob'\n"), "{shell}");
        assert_eq!(shell.lines().count(), 8);
    }
}
