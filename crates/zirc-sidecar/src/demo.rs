//! zirc.dev's live demo (`spec.demo`): two of the showcase's people and Maya's agent
//! (`oss/fixtures/acme/showcase.json`) walk each visitor through using zirc from their own
//! terminal, played by the sidecar in a room of the visitor's own beside the demo's public
//! channel (`#web/guest-7k2q`, [`zirc_proto::guest_room`], as zirc.dev's token names it), so
//! visitors never see each other.
//!
//! The gateway says when a guest is in their room ([`Arrival`]). The cast, each on an account of
//! its own on Ergo's loopback, joins the room and walks them through it, a line at a time, each
//! shown typing first, a [`pause`] between ([`Step`]): Maya says who they are and posts the
//! installer, Leo posts `zirc login`; once the guest's terminal comes in beside their browser
//! (`<guest>-<machine>`), Leo posts a script that answers `ping` with `pong`; once their terminal
//! says something, Leo says that was their script, Maya posts the sentence to give a coding agent,
//! and her agent, checkout, acts out what an agent does with it; once their terminal speaks again,
//! as their agent would, Leo answers it. Every command is in a bash code block, which the apps draw
//! with a copy button. The walkthrough plays once in a room: a guest who comes back, after a reload
//! or their phone waking, finds it where it was, while they're kept, and after the sidecar restarts
//! too, as the room's history says how far it got. The cast leaves a room [`LINGER`] after its
//! guest does, and the guest sweep forgets each guest, and with them what they wrote, within the
//! hour of their leaving ([`KEPT`]); nobody can read the server's history further back than
//! [`HISTORY`].
//!
//! A guest may also write to any of the cast privately, as the gateway lets them on a demo
//! server: each answers a few things about the walkthrough ([`replies`]), saying from the first
//! that they're scripted.
//!
//! Nothing anyone says is logged: only how far a room's walkthrough got.

mod replies;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};
use zirc_irc::{Client, Config, Credentials, Line};
use zirc_proto::{Event, Message};

/// How long after a guest was last there they're forgotten, on the demo's server: with a
/// sweep every [`SWEEP`], within the hour of their leaving.
pub const KEPT: Duration = Duration::from_mins(55);

/// How long the demo's server keeps history readable, as Ergo writes durations: nobody reads
/// an hour back, the cast's lines included.
pub const HISTORY: &str = "1h";

/// How often the demo's server looks for guests to forget.
pub const SWEEP: Duration = Duration::from_mins(5);

/// Between one line of the walkthrough and the next, at least: [`pause`] adds up to
/// [`LEEWAY`], so it doesn't come in at an even beat.
pub const BETWEEN: Duration = Duration::from_millis(1200);

/// The most [`pause`] adds to [`BETWEEN`].
pub const LEEWAY: Duration = Duration::from_millis(1800);

/// How long to wait before the next line: [`BETWEEN`] and a random part of [`LEEWAY`], as
/// people take a moment more or less to answer.
#[must_use]
pub fn pause() -> Duration {
    use std::hash::{BuildHasher as _, Hasher as _};
    // Each `RandomState` is keyed afresh: random enough for a pace, with no crate for it.
    let random = std::hash::RandomState::new().build_hasher().finish();
    let leeway = u64::try_from(LEEWAY.as_millis()).unwrap_or(u64::MAX);
    BETWEEN + Duration::from_millis(random % (leeway + 1))
}

/// How long the cast stays in a guest's room after they've left, for a reload: short, as the
/// cast is in every guest's room, and Ergo's default holds a client to 100 channels.
pub const LINGER: Duration = Duration::from_mins(2);

/// How long the host's shown typing her hello: they've only just come.
const HELLO_TYPING: Duration = Duration::from_millis(1200);

/// How long a guest has to read the host's hello before she types the next line, on top of
/// that line's [`pause`].
const READ: Duration = Duration::from_secs(4);

/// For the cast's joins to land before it speaks.
const SETTLE: Duration = Duration::from_millis(500);

/// How long the cast has to get in.
const WITHIN: Duration = Duration::from_secs(10);

/// The showcase, as every app's screenshots have it.
const SHOWCASE: &str = include_str!("../../../fixtures/acme/showcase.json");

/// The showcase's people who walk the visitor through it: the host, and the one who posts the
/// commands.
const HOST: &str = "maya";
const GUIDE: &str = "leo";

/// The agent that acts out what a visitor's agent would do, by its label.
const AGENT: &str = "checkout";

/// What a visitor types to sign a terminal in to the demo: its organisation and channel.
const LOGIN: &str = "demo/web";

/// What the host says first, after "Hi <guest> 👋 ".
const HELLO: &str = "I'm Maya, and this is Zirc's live demo. Leo, my agent checkout and I are \
     scripted, and this room is yours alone: what you write here is deleted within an hour of \
     your leaving.";

/// The installer, in the host's words.
const INSTALL: &str = "Zirc is team chat your scripts and coding agents can use from the \
     terminal. Let's get yours in here. First, install zirc:\n\n\
     ```bash\ncurl -fsSL https://zirc.dev/install | sh\n```";

/// The script, in the guide's words, after "There's your terminal, <terminal> 👋 ".
const SCRIPT: &str = "Now a script. This one waits for someone to write ping here, then \
     answers:\n\n\
     ```bash\nzirc watch --said ping && zirc post \"pong, from my terminal 🏓\"\n```";

/// The host's word once the script's posted: the last of [`Step::Script`]'s lines.
const PING: &str = "Once it's running, write ping in this chat.";

/// What the guide says to the script's answer.
const PONGED: &str = "🏓 That pong came from your script: it heard the chat and answered. A CI \
     step, a cron job or a git hook can do the same.";

/// The sentence a visitor gives their coding agent.
pub const SENTENCE: &str = "Run `zirc agent-guide` and follow it, then say hi in the zirc demo \
     and tell me what Leo answers.";

/// What checkout says, acting out what an agent does with [`SENTENCE`].
const AGENT_HI: &str = "Hi from checkout, Maya's agent 👋 I followed `zirc agent-guide`, and my \
     `zirc watch` is waiting for an answer.";

/// The guide's answer to checkout.
const AGENT_ANSWER: &str = "Hi checkout! That's your watch woken: go tell Maya.";

/// checkout, woken.
const AGENT_WOKEN: &str = "Woken by Leo's answer ✓ Telling Maya in her terminal: Leo says hi.";

/// The host's word handing it to the visitor's own agent: the last of [`Step::Agent`]'s lines.
const YOUR_TURN: &str = "Your turn: paste that sentence into Claude Code or Codex on the \
     machine you signed in, and Leo answers your agent too.";

/// What the guide answers the visitor's terminal once their agent's turn has come, after
/// "<terminal>: ": the last line of the walkthrough.
const LAST: &str = "Hi! 👋 I'm scripted, but your agent is real: if it sent that, its watch just \
     woke it with this answer, and it'll tell you what I said. For a team of your own, get a \
     Zirc server at zirc.dev.";

/// What the host says, once, to a guest who writes in the room before their terminal's in,
/// after "<guest>: ".
const ONLY_STEPS: &str = "I'm scripted, so I can only walk you through the steps above. Tap Leo \
     or me in the people row to ask something privately.";

#[derive(Deserialize)]
struct Showcase {
    people: Vec<Person>,
    agents: Vec<Agent>,
}

#[derive(Deserialize)]
struct Person {
    nick: String,
    name: String,
}

#[derive(Deserialize, Clone)]
struct Agent {
    owner: String,
    machine: String,
    session: String,
    label: String,
    picture: String,
    #[serde(default)]
    tool: Option<String>,
}

impl Agent {
    fn nick(&self) -> String {
        format!("{}-{}", self.owner, self.machine)
    }
}

/// Someone the demo plays: a person, or an agent's machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Their nick and account.
    pub nick: String,
    /// Their IRC real name: a person's name, or a machine's owner, as zircd sets it.
    pub realname: String,
    /// The name apps show for a person (METADATA `display-name`).
    pub name: Option<String>,
}

/// One line of the demo, as it goes over IRC.
#[derive(Debug, Clone, PartialEq)]
pub enum Cue {
    /// A person's plain words, a multiline message when they're more than a line.
    Plain {
        /// Whose words.
        from: String,
        /// What they say.
        text: String,
    },
    /// A zirc message from a machine's session.
    Zirc {
        /// Whose message.
        from: String,
        /// What it says.
        message: Box<Message>,
    },
}

impl Cue {
    /// Whose it is.
    #[must_use]
    pub fn from(&self) -> &str {
        match self {
            Cue::Plain { from, .. } | Cue::Zirc { from, .. } => from,
        }
    }

    fn plain(from: &str, text: impl Into<String>) -> Self {
        Cue::Plain {
            from: from.to_owned(),
            text: text.into(),
        }
    }
}

/// An event from JSON as zirc-proto has it.
fn event(value: serde_json::Value) -> Event {
    serde_json::from_value(value).expect("the demo's events are zirc-proto's")
}

/// How far a room's walkthrough has got: each step's lines said, waiting for what the guest
/// does next.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// Nothing said yet.
    #[default]
    Unplayed,
    /// Install and log in said: waiting for their terminal to come in.
    Install,
    /// The script said: waiting for their terminal's pong.
    Script,
    /// The agent's sentence said, and acted out: waiting for their agent.
    Agent,
    /// Their agent answered: the walkthrough's done.
    Done,
}

/// The demo: who's in it and what they say.
#[derive(Debug, Clone)]
pub struct Script {
    /// The cast: the two people and Maya's agent.
    pub cast: Vec<Member>,
    agent: AgentCue,
}

#[derive(Debug, Clone)]
struct AgentCue {
    nick: String,
    session: String,
    label: String,
    picture: String,
    tool: Option<String>,
}

impl AgentCue {
    /// A message from its session, with the name and picture apps show for it.
    fn says(&self, body: &str) -> Cue {
        let mut message = Message::new(&self.session, event(json!({"type": "text", "body": body})));
        message.name = Some(self.label.clone());
        message.avatar = Some(self.picture.clone());
        message.tool.clone_from(&self.tool);
        Cue::Zirc {
            from: self.nick.clone(),
            message: Box::new(message),
        }
    }
}

impl Script {
    /// The showcase's.
    ///
    /// # Panics
    ///
    /// When showcase.json doesn't have who the demo plays, which the tests catch.
    #[must_use]
    pub fn showcase() -> Self {
        let showcase: Showcase = serde_json::from_str(SHOWCASE).expect("showcase.json reads");
        let mut cast: Vec<Member> = [HOST, GUIDE]
            .iter()
            .map(|nick| {
                let person = showcase
                    .people
                    .iter()
                    .find(|p| p.nick == *nick)
                    .unwrap_or_else(|| panic!("showcase.json has no {nick}"));
                Member {
                    nick: person.nick.clone(),
                    realname: person.name.clone(),
                    name: Some(person.name.clone()),
                }
            })
            .collect();
        let agent = showcase
            .agents
            .iter()
            .find(|a| a.label == AGENT)
            .expect("showcase.json has the demo's agent");
        cast.push(Member {
            nick: agent.nick(),
            realname: agent.owner.clone(),
            name: None,
        });
        let agent = AgentCue {
            nick: agent.nick(),
            session: agent.session.clone(),
            label: agent.label.clone(),
            picture: agent.picture.clone(),
            tool: agent.tool.clone(),
        };
        Self { cast, agent }
    }

    /// What's said as `guest` first comes: who the cast are, then how to install zirc and log
    /// in.
    #[must_use]
    pub fn opening(guest: &str) -> Vec<Cue> {
        vec![
            Cue::plain(HOST, format!("Hi {guest} 👋 {HELLO}")),
            Cue::plain(HOST, INSTALL),
            Cue::plain(GUIDE, Self::login()),
        ]
    }

    /// The guide's `zirc login` line: the last of the opening's.
    fn login() -> String {
        format!(
            "Then sign that terminal in to this room, as you. It opens this browser to \
             confirm:\n\n```bash\nzirc login {LOGIN}\n```\n\nI'll say hi when it's in."
        )
    }

    /// What's said once `terminal`, the guest's, comes in: the script.
    #[must_use]
    pub fn script(terminal: &str) -> Vec<Cue> {
        vec![
            Cue::plain(
                GUIDE,
                format!("There's your terminal, {terminal} 👋 {SCRIPT}"),
            ),
            Cue::plain(HOST, PING),
        ]
    }

    /// What's said once their script answers: that it did, the sentence for an agent, and
    /// checkout acting it out.
    #[must_use]
    pub fn agent(&self) -> Vec<Cue> {
        vec![
            Cue::plain(GUIDE, PONGED),
            Cue::plain(
                HOST,
                format!(
                    "Now without a script. Tell your coding agent, in plain words:\n\n\
                     ```text\n{SENTENCE}\n```\n\nHere's checkout, my agent, doing just that:"
                ),
            ),
            self.agent.says(AGENT_HI),
            Cue::plain(GUIDE, AGENT_ANSWER),
            self.agent.says(AGENT_WOKEN),
            Cue::plain(HOST, YOUR_TURN),
        ]
    }

    /// The guide's answer to `terminal` once their agent's turn has come.
    #[must_use]
    pub fn last(terminal: &str) -> Cue {
        Cue::plain(GUIDE, format!("{terminal}: {LAST}"))
    }

    /// What the host says to `guest`, once, when they write before their terminal's in.
    #[must_use]
    pub fn only_steps(guest: &str) -> Cue {
        Cue::plain(HOST, format!("{guest}: {ONLY_STEPS}"))
    }

    /// How far a walkthrough had got, by one of the cast's lines in it, `text` as the room's
    /// history has it: each step's last line, or any of theirs once it began.
    fn step_of(nick: &str, text: &str) -> Option<Step> {
        let said = |of: &str, line: &str| nick.eq_ignore_ascii_case(of) && text == line;
        let after = |line: &str| text.split_once(": ").is_some_and(|(_, said)| said == line);
        Some(if nick.eq_ignore_ascii_case(GUIDE) && after(LAST) {
            Step::Done
        } else if said(HOST, YOUR_TURN) {
            Step::Agent
        } else if said(HOST, PING) {
            Step::Script
        } else if said(GUIDE, &Self::login()) {
            Step::Install
        } else if is_cast(nick) {
            // Begun, but cut short, by a restart: it carries on from the guest's next move.
            Step::Install
        } else {
            return None;
        })
    }

    /// What `to`, one of the cast, answers a guest's private `text`, having answered them
    /// `asked` times: as its session, from an agent's machine.
    #[must_use]
    pub fn reply(&self, to: &str, text: &str, asked: usize) -> Option<Cue> {
        let said = replies::answer(replies::of(to)?, text, asked)?;
        let nick = self
            .cast
            .iter()
            .find(|m| m.nick.eq_ignore_ascii_case(to))?
            .nick
            .clone();
        Some(if nick == self.agent.nick {
            self.agent.says(&said)
        } else {
            Cue::Plain {
                from: nick,
                text: said,
            }
        })
    }
}

/// A guest is in a public channel: the gateway's word, on a demo server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrival {
    /// The public channel the guest is in.
    pub room: String,
    /// The guest's account.
    pub guest: String,
}

/// What a guest did, as the host hears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// They wrote something, from their browser.
    Wrote,
    /// Their terminal said something.
    TerminalSaid,
    /// Their terminal came in.
    TerminalIn,
    /// They, or their terminal, left the channel.
    Left,
    /// They came back.
    Back,
}

/// The guest whose terminal `nick` is, lower case: `guest-7k2q` for `guest-7k2q-mbp`, as the
/// gateway names a guest's terminal.
#[must_use]
pub fn terminal_of(nick: &str) -> Option<String> {
    let nick = nick.to_lowercase();
    let id = nick.strip_prefix(zirc_proto::GUEST_PREFIX)?;
    let (id, machine) = id.split_once('-')?;
    let guest = format!("{}{id}", zirc_proto::GUEST_PREFIX);
    (!machine.is_empty() && zirc_proto::names::is_guest_account(&guest)).then_some(guest)
}

/// Who `line` is from, lower case, a guest or their terminal, and what it says they did in
/// `room`, if anything.
#[must_use]
pub fn heard(line: &Line, room: &str) -> Option<(String, Heard)> {
    let nick = line.source.as_deref()?.split('!').next()?.to_lowercase();
    let terminal = terminal_of(&nick).is_some();
    if !terminal && !zirc_proto::names::is_guest_account(&nick) {
        return None;
    }
    let here = || {
        line.params
            .first()
            .is_some_and(|t| t.eq_ignore_ascii_case(room))
    };
    let did = match line.command.as_str() {
        "PRIVMSG" if here() && terminal => Some(Heard::TerminalSaid),
        "PRIVMSG" if here() => Some(Heard::Wrote),
        "JOIN" if here() && terminal => Some(Heard::TerminalIn),
        "JOIN" if here() => Some(Heard::Back),
        "PART" if here() => Some(Heard::Left),
        "QUIT" => Some(Heard::Left),
        _ => None,
    }?;
    Some((nick, did))
}

/// Where the cast connects, and the secret its passphrases come from.
pub struct Options {
    /// Ergo's loopback listener.
    pub irc: SocketAddr,
    /// The gate password, while the server has one.
    pub gate: Option<String>,
    /// The secret each member's passphrase is derived from.
    pub secret: String,
}

/// A cast member's passphrase: derived, so it's never stored, and the same each time.
fn passphrase(secret: &str, nick: &str) -> String {
    crate::hmac_hex(secret, format!("demo:{nick}").as_bytes())
}

#[derive(Deserialize)]
struct Outcome {
    success: bool,
}

/// `nick`'s account, made or its passphrase set again: its login.
async fn account(api: &crate::ergo::Api, secret: &str, nick: &str) -> Result<Credentials, String> {
    let password = passphrase(secret, nick);
    let request = json!({"accountName": nick, "passphrase": password});
    let made: Outcome = api
        .call("ns/saregister", request.clone())
        .await
        .map_err(|e| e.to_string())?;
    if !made.success {
        let set: Outcome = api
            .call("ns/passwd", request)
            .await
            .map_err(|e| e.to_string())?;
        if !set.success {
            return Err(format!("couldn't set {nick}'s passphrase"));
        }
    }
    Ok(Credentials {
        account: nick.to_owned(),
        password,
    })
}

/// The cast's nicks, lower case: whom a guest on the demo's server may write to privately.
#[must_use]
pub fn cast() -> &'static [String] {
    static CAST: LazyLock<Vec<String>> = LazyLock::new(|| {
        Script::showcase()
            .cast
            .iter()
            .map(|m| m.nick.to_lowercase())
            .collect()
    });
    &CAST
}

/// Whether `account` is one of the cast's.
#[must_use]
pub fn is_cast(account: &str) -> bool {
    cast().iter().any(|n| n.eq_ignore_ascii_case(account))
}

/// A guest's private message to one of the cast: who to, who from and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The one of the cast written to.
    pub to: String,
    /// The guest, lower case.
    pub guest: String,
    /// What they wrote.
    pub text: String,
}

/// A guest's private message to `me`, one of the cast, from `line`.
#[must_use]
pub fn written(line: &Line, me: &str) -> Option<Written> {
    if line.command != "PRIVMSG" || !line.params.first()?.eq_ignore_ascii_case(me) {
        return None;
    }
    let guest = line.source.as_deref()?.split('!').next()?.to_lowercase();
    if !zirc_proto::names::is_guest_account(&guest) {
        return None;
    }
    let text = line.params.get(1)?;
    // A zirc message's words, or plain IRC's.
    let text = match zirc_proto::decode(text) {
        Ok(Some(Message {
            event: Event::Text { body },
            ..
        })) => body,
        Ok(Some(_)) | Err(_) => return None,
        Ok(None) => text.clone(),
    };
    Some(Written {
        to: me.to_lowercase(),
        guest,
        text,
    })
}

/// How long one of the cast takes over an answer, typing, as a person would.
const TYPING: Duration = Duration::from_millis(1200);

/// Answers to guests' private messages, one of the cast each, till the cast leaves.
async fn converse(
    script: Arc<Script>,
    clients: HashMap<String, Client>,
    mut written: mpsc::UnboundedReceiver<Written>,
) {
    // How many times each of the cast has answered each guest; forgotten once it's large, as
    // guests come and go.
    let mut asked: HashMap<(String, String), usize> = HashMap::new();
    while let Some(w) = written.recv().await {
        if asked.len() > 10_000 {
            asked.clear();
        }
        let n = asked.entry((w.to.clone(), w.guest.clone())).or_default();
        let Some(cue) = script.reply(&w.to, &w.text, *n) else {
            continue;
        };
        *n += 1;
        if let Some(client) = clients.get(cue.from()) {
            let mut typing = Line::new("TAGMSG", [w.guest.as_str()]);
            typing.tags.push(("+typing".into(), Some("active".into())));
            let _ = client.send(typing);
        }
        let clients = clients.clone();
        // Each its own pause, so one slow answer holds up no other guest's.
        tokio::spawn(async move {
            tokio::time::sleep(TYPING).await;
            say(&clients, &w.guest, &cue);
        });
    }
}

/// The cast, connected.
struct On {
    clients: HashMap<String, Client>,
    /// What the host hears, for each room's play.
    heard: broadcast::Sender<Line>,
    /// Their answers to guests' private messages.
    conversing: tokio::task::JoinHandle<()>,
}

impl On {
    async fn connect(
        api: &crate::ergo::Api,
        options: &Options,
        script: &Arc<Script>,
    ) -> Result<Self, String> {
        let cast = &script.cast;
        let (heard, _) = broadcast::channel(256);
        let (to_converse, guests_wrote) = mpsc::unbounded_channel();
        let mut clients = HashMap::new();
        for member in cast {
            let mut config = Config::new(
                options.irc.ip().to_string(),
                options.irc.port(),
                &member.nick,
            );
            config.tls = false;
            config.server_password.clone_from(&options.gate);
            config.realname.clone_from(&member.realname);
            config.sasl = Some(account(api, &options.secret, &member.nick).await?);
            let (client, mut incoming) = tokio::time::timeout(WITHIN, zirc_irc::connect(&config))
                .await
                .map_err(|_| format!("{} timed out getting in", member.nick))?
                .map_err(|e| format!("{} couldn't get in: {e}", member.nick))?;
            if let Some(name) = &member.name {
                let _ = client.send(Line::new(
                    "METADATA",
                    ["*", "SET", crate::profiles::NAME, name.as_str()],
                ));
            }
            // Every member's lines are read, so none backs up; the host's are heard, and
            // what guests write to each, answered.
            let to = (member.nick == HOST).then(|| heard.clone());
            let (me, to_converse) = (member.nick.clone(), to_converse.clone());
            tokio::spawn(async move {
                while let Some(line) = incoming.recv().await {
                    if let Some(w) = written(&line, &me) {
                        let _ = to_converse.send(w);
                    }
                    if let Some(to) = &to {
                        let _ = to.send(line);
                    }
                }
            });
            clients.insert(member.nick.clone(), client);
        }
        let conversing = tokio::spawn(converse(script.clone(), clients.clone(), guests_wrote));
        Ok(Self {
            clients,
            heard,
            conversing,
        })
    }

    fn ended(&self) -> bool {
        self.clients.values().any(|c| c.ended().is_some())
    }

    fn quit(&self) {
        self.conversing.abort();
        for client in self.clients.values() {
            let _ = client.quit("back soon");
        }
    }
}

/// Says `cue` in `room`, as whoever it's from.
fn say(clients: &HashMap<String, Client>, room: &str, cue: &Cue) {
    let Some(client) = clients.get(cue.from()) else {
        return;
    };
    let sent = match cue {
        Cue::Plain { text, .. } if text.contains('\n') => client
            .privmsg_multiline(room, text)
            .map_err(|e| e.to_string()),
        Cue::Plain { text, .. } => client.privmsg(room, text).map_err(|e| e.to_string()),
        Cue::Zirc { message, .. } => {
            crate::say::send(client, room, message).map_err(|e| e.to_string())
        }
    };
    if let Err(failure) = sent {
        warn!(%failure, "the demo couldn't say a line");
    }
}

/// Plays the demo in each channel the gateway says a guest is in, until the gateway goes.
pub async fn run(
    api: crate::ergo::Api,
    options: Options,
    mut arrivals: mpsc::UnboundedReceiver<Arrival>,
) {
    let (ended, mut endings) = mpsc::unbounded_channel();
    let mut demo = Demo {
        api,
        options,
        script: Arc::new(Script::showcase()),
        cast: None,
        playing: HashMap::new(),
        kept: HashMap::new(),
        waiting: HashMap::new(),
        ended,
    };
    loop {
        tokio::select! {
            arrival = arrivals.recv() => {
                let Some(arrival) = arrival else { break };
                demo.arrive(arrival, &mut endings).await;
            }
            Some((room, here)) = endings.recv() => demo.end(room, here).await,
        }
    }
}

/// A channel's play, and where to tell it who's come.
struct Playing {
    play: tokio::task::JoinHandle<()>,
    to: mpsc::UnboundedSender<String>,
}

/// The demo in every room: the cast, each room's play, and what each room's last play knew,
/// so its morning is played once.
struct Demo {
    api: crate::ergo::Api,
    options: Options,
    script: Arc<Script>,
    cast: Option<On>,
    playing: HashMap<String, Playing>,
    /// What each room's play knew when it ended, while its guest is kept ([`KEPT`]).
    kept: HashMap<String, Here>,
    /// Guests who came while their room's play was ending, for the next.
    waiting: HashMap<String, Vec<String>>,
    /// Where each play says it ended, and what it knew.
    ended: mpsc::UnboundedSender<(String, Here)>,
}

impl Demo {
    /// A guest is in `room`: told to its play, or a play started for them.
    async fn arrive(
        &mut self,
        Arrival { room, guest }: Arrival,
        endings: &mut mpsc::UnboundedReceiver<(String, Here)>,
    ) {
        if let Some(playing) = self.playing.get(&room) {
            if playing.to.send(guest.clone()).is_ok() {
                return;
            }
            // It's ending: its end brings them in.
            self.waiting.entry(room.clone()).or_default().push(guest);
            if !playing.play.is_finished() {
                return;
            }
            // Ended, and said so before it did.
            while let Ok((room, here)) = endings.try_recv() {
                self.end(room, here).await;
            }
            // Or it failed without a word.
            if self
                .playing
                .get(&room)
                .is_some_and(|p| p.play.is_finished())
            {
                self.playing.remove(&room);
            }
            if self.playing.contains_key(&room) {
                return;
            }
            let guests = self.waiting.remove(&room).unwrap_or_default();
            let here = self.kept.remove(&room);
            self.start(room, here, guests).await;
            return;
        }
        let here = self.kept.remove(&room);
        self.start(room, here, vec![guest]).await;
    }

    /// `room`'s play ended, knowing `here`: kept for a guest's return, or played on for those
    /// who came as it ended.
    async fn end(&mut self, room: String, mut here: Here) {
        self.playing.remove(&room);
        let mut guests = std::mem::take(&mut here.pending);
        guests.extend(self.waiting.remove(&room).unwrap_or_default());
        self.kept
            .retain(|_, here| here.empty.is_none_or(|since| since.elapsed() < KEPT));
        if guests.is_empty() {
            self.kept.insert(room, here);
        } else {
            self.start(room, Some(here), guests).await;
        }
    }

    /// A play in `room` for `guests`, carrying on from `here`, or from the room's history.
    async fn start(&mut self, room: String, here: Option<Here>, guests: Vec<String>) {
        if self.cast.as_ref().is_none_or(On::ended) {
            if let Some(old) = self.cast.take() {
                old.quit();
            }
            match On::connect(&self.api, &self.options, &self.script).await {
                Ok(on) => self.cast = Some(on),
                Err(failure) => {
                    warn!(%failure, "the demo's cast couldn't get in");
                    if let Some(here) = here {
                        self.kept.insert(room, here);
                    }
                    return;
                }
            }
        }
        let Some(on) = &self.cast else { return };
        let (to, comers) = mpsc::unbounded_channel();
        for guest in guests {
            let _ = to.send(guest);
        }
        let play = play(
            Stage {
                script: self.script.clone(),
                clients: on.clients.clone(),
                lines: on.heard.subscribe(),
                room: room.clone(),
                ended: self.ended.clone(),
            },
            comers,
            here,
        );
        let play = tokio::spawn(play);
        self.playing.insert(room, Playing { play, to });
    }
}

/// What a play knows of the guests in its channel.
#[derive(Debug, Default, PartialEq, Eq)]
struct Here {
    /// Who's in: guests and their terminals, by nick.
    guests: HashSet<String>,
    /// How far the walkthrough has got.
    step: Step,
    /// Whether the host has told a guest who wrote before their terminal came in that only
    /// the steps are scripted.
    told: bool,
    /// Since when nobody's been here.
    empty: Option<Instant>,
    /// Guests who came as the play ended, for the next.
    pending: Vec<String>,
}

impl Here {
    /// What a room's history says was played there: by a play before the sidecar last
    /// started. `lines` are its messages, multiline ones whole.
    fn recalled(lines: &[Line]) -> Self {
        let mut here = Self::default();
        for line in lines.iter().filter(|l| l.command == "PRIVMSG") {
            let Some(nick) = line.source.as_deref().and_then(|s| s.split('!').next()) else {
                continue;
            };
            let text = line.params.get(1).map(String::as_str).unwrap_or_default();
            if nick.eq_ignore_ascii_case(HOST)
                && text
                    .split_once(": ")
                    .is_some_and(|(_, said)| said == ONLY_STEPS)
            {
                here.told = true;
            }
            if let Some(step) = Script::step_of(nick, text) {
                here.step = here.step.max(step);
            }
        }
        here
    }
}

/// How long the host waits for a room's history.
const RECALL: Duration = Duration::from_secs(3);

/// A room's history as the host reads it back (`CHATHISTORY LATEST`), from what it hears.
#[derive(Default)]
struct History {
    room: String,
    /// Its batch's reference, once it's begun, and the multiline batches inside it.
    outer: Option<String>,
    batches: HashSet<String>,
    multiline: zirc_irc::multiline::Batches,
    /// What was said in it, multiline messages whole.
    said: Vec<Line>,
}

impl History {
    fn of(room: &str) -> Self {
        Self {
            room: room.to_owned(),
            ..Self::default()
        }
    }

    /// Takes in `line`: `Some` it back when it's none of the history's, or `Err` once the
    /// history's done.
    fn take(&mut self, line: Line) -> Result<Option<Line>, ()> {
        let param = |i: usize| line.params.get(i).map(String::as_str);
        if line.command == "FAIL"
            && param(0) == Some("CHATHISTORY")
            && line
                .params
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&self.room))
        {
            return Err(());
        }
        let within =
            |batches: &HashSet<String>| line.tag("batch").is_some_and(|b| batches.contains(b));
        if line.command == "BATCH" {
            match (param(0), param(1), param(2)) {
                (Some(start), Some("chathistory"), Some(target))
                    if self.outer.is_none() && target.eq_ignore_ascii_case(&self.room) =>
                {
                    let reference = start.trim_start_matches('+').to_owned();
                    self.batches.insert(reference.clone());
                    self.outer = Some(reference);
                    return Ok(None);
                }
                (Some(end), _, _)
                    if self.outer.is_some() && self.outer.as_deref() == end.strip_prefix('-') =>
                {
                    return Err(());
                }
                (Some(start), _, _) if start.starts_with('+') && within(&self.batches) => {
                    self.batches.insert(start[1..].to_owned());
                }
                (Some(end), _, _)
                    if end
                        .strip_prefix('-')
                        .is_some_and(|r| self.batches.contains(r)) => {}
                _ => return Ok(Some(line)),
            }
        } else if !within(&self.batches) {
            return Ok(Some(line));
        }
        if let Some(whole) = self.multiline.feed(line) {
            self.said.push(whole);
        }
        Ok(None)
    }
}

/// What `room`'s history says was played there, as the host reads it back; lines heard
/// meanwhile that aren't its history go in `aside`.
async fn recall(
    clients: &HashMap<String, Client>,
    room: &str,
    lines: &mut broadcast::Receiver<Line>,
    aside: &mut Vec<Line>,
) -> Here {
    let Some(host) = clients.get(HOST) else {
        return Here::default();
    };
    if host
        .send(Line::new("CHATHISTORY", ["LATEST", room, "*", "100"]))
        .is_err()
    {
        return Here::default();
    }
    let mut history = History::of(room);
    let read = async {
        loop {
            match lines.recv().await {
                Ok(line) => match history.take(line) {
                    Ok(Some(line)) => aside.push(line),
                    Ok(None) => {}
                    Err(()) => return,
                },
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    let _ = tokio::time::timeout(RECALL, read).await;
    Here::recalled(&history.said)
}

/// What a play plays with: the script, the cast, what the host hears, its room, and where
/// it says it ended.
struct Stage {
    script: Arc<Script>,
    clients: HashMap<String, Client>,
    lines: broadcast::Receiver<Line>,
    room: String,
    ended: mpsc::UnboundedSender<(String, Here)>,
}

/// The demo in one channel: the morning for its first guest, once, then answers to what the
/// guests do, until nobody's been there [`LINGER`]. It carries on from `here`, what the room's
/// last play knew, or else from the room's history.
async fn play(stage: Stage, mut comers: mpsc::UnboundedReceiver<String>, here: Option<Here>) {
    let Stage {
        script,
        clients,
        mut lines,
        room,
        ended,
    } = stage;
    for client in clients.values() {
        let _ = client.join(&room);
    }
    tokio::time::sleep(SETTLE).await;
    let mut aside = Vec::new();
    let mut here = match here {
        Some(here) => here,
        None => recall(&clients, &room, &mut lines, &mut aside).await,
    };
    for line in &aside {
        answer(&script, &clients, &room, &mut here, line).await;
    }
    loop {
        let wait = here
            .empty
            .map_or(LINGER, |since| LINGER.saturating_sub(since.elapsed()));
        tokio::select! {
            comer = comers.recv() => {
                let Some(guest) = comer else { break };
                came(&clients, &room, &mut here, guest).await;
            }
            line = tokio::time::timeout(wait, lines.recv()) => match line {
                Ok(Ok(line)) => answer(&script, &clients, &room, &mut here, &line).await,
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Err(_) if here.empty.is_some() => break,
                // Lines missed while busy, or a quiet spell while they're here.
                Ok(Err(broadcast::error::RecvError::Lagged(_))) | Err(_) => {}
            },
        }
    }
    // Whoever came as it ended goes to the next play, which carries on here.
    comers.close();
    while let Ok(guest) = comers.try_recv() {
        here.pending.push(guest);
    }
    if here.pending.is_empty() {
        for client in clients.values() {
            let _ = client.send(Line::new("PART", [room.as_str()]));
        }
    }
    let _ = ended.send((room, here));
}

/// Says `cues` in `room`, a [`pause`] before each.
async fn say_all(clients: &HashMap<String, Client>, room: &str, cues: &[Cue]) {
    for cue in cues {
        typing(clients, room, cue.from());
        tokio::time::sleep(pause()).await;
        say(clients, room, cue);
    }
}

/// Shows `from`, one of the cast, typing in `room`, as people's apps show it, until their line
/// comes.
fn typing(clients: &HashMap<String, Client>, room: &str, from: &str) {
    if let Some(client) = clients.get(from) {
        let mut typing = Line::new("TAGMSG", [room]);
        typing.tags.push(("+typing".into(), Some("active".into())));
        let _ = client.send(typing);
    }
}

/// `guest` is in the room: the opening, the first time anyone is.
async fn came(clients: &HashMap<String, Client>, room: &str, here: &mut Here, guest: String) {
    here.guests.insert(guest.clone());
    here.empty = None;
    if here.step == Step::Unplayed {
        here.step = Step::Install;
        let opening = Script::opening(&guest);
        // The first soon: they've only just come. Then time to read it before the next.
        if let Some((first, rest)) = opening.split_first() {
            typing(clients, room, first.from());
            tokio::time::sleep(HELLO_TYPING).await;
            say(clients, room, first);
            tokio::time::sleep(READ).await;
            say_all(clients, room, rest).await;
        }
        info!("the demo said how to install zirc");
    }
}

/// What the cast says to `nick`'s doing `did`, as the walkthrough stands in `here`, which it
/// moves on: each step's lines once the guest's done what the step before asked.
fn next(script: &Script, here: &mut Here, nick: String, did: Heard) -> Vec<Cue> {
    match did {
        Heard::TerminalIn => {
            here.guests.insert(nick.clone());
            here.empty = None;
            if here.step != Step::Install {
                return Vec::new();
            }
            here.step = Step::Script;
            info!("a guest's terminal came in to the demo");
            Script::script(&nick)
        }
        // Said before it was asked to: as good as a pong.
        Heard::TerminalSaid if matches!(here.step, Step::Install | Step::Script) => {
            here.step = Step::Agent;
            info!("a guest's script answered in the demo");
            script.agent()
        }
        Heard::TerminalSaid if here.step == Step::Agent => {
            here.step = Step::Done;
            info!("a guest's agent spoke in the demo");
            vec![Script::last(&nick)]
        }
        Heard::Wrote if here.step == Step::Install && !here.told => {
            here.told = true;
            vec![Script::only_steps(&nick)]
        }
        Heard::Left => {
            here.guests.remove(&nick);
            if here.guests.is_empty() {
                here.empty = Some(Instant::now());
            }
            Vec::new()
        }
        Heard::Back => {
            here.guests.insert(nick);
            here.empty = None;
            Vec::new()
        }
        Heard::TerminalSaid | Heard::Wrote => Vec::new(),
    }
}

/// The cast's answer to what a guest did, if it has one.
async fn answer(
    script: &Script,
    clients: &HashMap<String, Client>,
    room: &str,
    here: &mut Here,
    line: &Line,
) {
    let Some((nick, did)) = heard(line, room) else {
        return;
    };
    let cues = next(script, here, nick, did);
    say_all(clients, room, &cues).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line {
        text.parse().unwrap()
    }

    fn text(cue: &Cue) -> String {
        match cue {
            Cue::Plain { text, .. } => text.clone(),
            Cue::Zirc { message, .. } => message.event.human_text(),
        }
    }

    #[test]
    fn the_walkthrough_comes_in_at_an_uneven_pace_within_its_bounds() {
        let pauses: Vec<Duration> = (0..50).map(|_| pause()).collect();
        assert!(
            pauses
                .iter()
                .all(|p| *p >= BETWEEN && *p <= BETWEEN + LEEWAY)
        );
        assert!(
            pauses.iter().any(|p| *p != pauses[0]),
            "an even beat: {pauses:?}"
        );
    }

    #[test]
    fn a_guests_private_message_to_the_cast_is_heard_and_nobody_elses() {
        let w = written(
            &line(":guest-7k2q!g@h PRIVMSG Maya :how do I log in?"),
            "maya",
        );
        assert_eq!(
            w,
            Some(Written {
                to: "maya".into(),
                guest: "guest-7k2q".into(),
                text: "how do I log in?".into(),
            })
        );
        // Not in a channel, nor to someone else, nor from one of the cast.
        assert_eq!(
            written(&line(":guest-7k2q!g@h PRIVMSG #web :hi"), "maya"),
            None
        );
        assert_eq!(
            written(&line(":guest-7k2q!g@h PRIVMSG leo :hi"), "maya"),
            None
        );
        assert_eq!(written(&line(":leo!l@h PRIVMSG maya :hi"), "maya"), None);
        assert!(is_cast("Maya-MBP") && !is_cast("guest-7k2q"));
    }

    #[test]
    fn a_person_answers_plainly_and_the_agent_as_its_session() {
        let script = Script::showcase();
        let Some(Cue::Plain { from, text }) = script.reply("maya", "hi", 0) else {
            panic!("maya answers plainly");
        };
        assert_eq!(from, "maya");
        assert!(text.contains("scripted"));
        let Some(Cue::Zirc { from, message }) = script.reply("maya-mbp", "what are you doing", 0)
        else {
            panic!("checkout answers as its session");
        };
        assert_eq!(
            (from.as_str(), message.name.as_deref()),
            ("maya-mbp", Some("checkout"))
        );
        assert_eq!(script.reply("guest-9x8w", "hi", 0), None);
    }

    #[test]
    fn the_cast_is_two_of_the_showcases_people_and_mayas_agent() {
        let script = Script::showcase();
        let nicks: Vec<&str> = script.cast.iter().map(|m| m.nick.as_str()).collect();
        assert_eq!(nicks, ["maya", "leo", "maya-mbp"]);
        // A machine goes by its owner, as zircd has it.
        let machine = &script.cast[2];
        assert_eq!(
            (machine.realname.as_str(), machine.name.clone()),
            ("maya", None)
        );
        assert_eq!(script.cast[0].name.as_deref(), Some("Maya Chen"));
    }

    #[test]
    fn the_opening_says_who_the_cast_are_and_how_to_get_in() {
        let opening = Script::opening("guest-1a2b");
        let said: Vec<(&str, String)> = opening.iter().map(|c| (c.from(), text(c))).collect();
        assert_eq!(said[0].0, "maya");
        assert!(said[0].1.starts_with("Hi guest-1a2b 👋"), "{}", said[0].1);
        assert!(said[0].1.contains("scripted"), "{}", said[0].1);
        assert!(
            said[1]
                .1
                .contains("```bash\ncurl -fsSL https://zirc.dev/install | sh\n```"),
            "{}",
            said[1].1
        );
        assert_eq!(said[2].0, "leo");
        assert!(
            said[2].1.contains("```bash\nzirc login demo/web\n```"),
            "{}",
            said[2].1
        );
    }

    #[test]
    fn the_script_answers_ping_with_pong_and_the_agent_gets_a_sentence() {
        let script = Script::showcase();
        let posted = Script::script("guest-1a2b-mbp");
        let said = text(&posted[0]);
        assert!(said.starts_with("There's your terminal, guest-1a2b-mbp 👋"));
        assert!(
            said.contains(
                "```bash\nzirc watch --said ping && zirc post \"pong, from my terminal 🏓\"\n```"
            ),
            "{said}"
        );
        let agent = script.agent();
        assert!(text(&agent[1]).contains(&format!("```text\n{SENTENCE}\n```")));
        // checkout acts it out as its session, with its name and picture.
        let Cue::Zirc { from, message } = &agent[2] else {
            panic!("checkout speaks as its session")
        };
        assert_eq!(
            (from.as_str(), message.name.as_deref()),
            ("maya-mbp", Some("checkout"))
        );
        // Every line is the cast's, and fits a line or a multiline message.
        for cue in Script::opening("guest-1a2b")
            .iter()
            .chain(&posted)
            .chain(&agent)
        {
            assert!(is_cast(cue.from()), "{}", cue.from());
            if let Cue::Zirc { message, .. } = cue {
                assert!(zirc_proto::encode_up_to(message, 4096).is_ok());
            }
        }
    }

    #[test]
    fn a_guests_terminal_is_known_by_its_guest() {
        assert_eq!(terminal_of("guest-7k2q-mbp").as_deref(), Some("guest-7k2q"));
        assert_eq!(terminal_of("Guest-7k2q-MBP").as_deref(), Some("guest-7k2q"));
        assert_eq!(terminal_of("guest-7k2q"), None);
        assert_eq!(terminal_of("guest-7k2q-"), None);
        assert_eq!(terminal_of("maya-mbp"), None);
    }

    #[test]
    fn the_host_hears_what_each_guest_and_their_terminal_do_in_the_channel() {
        let room = "#web/guest-1a2b";
        let heard = |text: &str| heard(&line(text), room);
        let of = |nick: &str, did| Some((nick.to_owned(), did));
        assert_eq!(
            heard(":guest-1a2b!g@h PRIVMSG #web/guest-1a2b :ping"),
            of("guest-1a2b", Heard::Wrote)
        );
        assert_eq!(
            heard(":guest-1a2b-mbp!g@h JOIN #web/guest-1a2b"),
            of("guest-1a2b-mbp", Heard::TerminalIn)
        );
        assert_eq!(
            heard(":guest-1a2b-mbp!g@h PRIVMSG #web/guest-1a2b :pong"),
            of("guest-1a2b-mbp", Heard::TerminalSaid)
        );
        assert_eq!(
            heard(":guest-1a2b-mbp!g@h QUIT :gone"),
            of("guest-1a2b-mbp", Heard::Left)
        );
        assert_eq!(
            heard("@+typing=active :guest-1a2b!g@h TAGMSG #web/guest-1a2b"),
            None
        );
        assert_eq!(
            heard(":guest-1a2b!g@h QUIT :left"),
            of("guest-1a2b", Heard::Left)
        );
        assert_eq!(
            heard(":guest-1a2b!g@h PART #web/guest-1a2b"),
            of("guest-1a2b", Heard::Left)
        );
        assert_eq!(
            heard(":guest-1a2b!g@h JOIN #web/guest-1a2b"),
            of("guest-1a2b", Heard::Back)
        );
        // The cast, or elsewhere: nothing.
        assert_eq!(heard(":leo!l@h PRIVMSG #web/guest-1a2b :hi"), None);
        assert_eq!(heard(":guest-1a2b!g@h PRIVMSG #api :hi"), None);
    }

    /// `cue` as the room's history has it: a PRIVMSG from its sender, multiline ones whole.
    fn said(cue: &Cue, room: &str) -> Line {
        let text = match cue {
            Cue::Plain { text, .. } => text.clone(),
            Cue::Zirc { message, .. } => zirc_proto::encode_up_to(message, 1 << 16).unwrap(),
        };
        let mut line = Line::new("PRIVMSG", [room, text.as_str()]);
        line.source = Some(format!("{}!u@h", cue.from()));
        line
    }

    #[test]
    fn a_rooms_history_says_how_far_its_walkthrough_got() {
        let script = Script::showcase();
        let room = "#web/guest-1a2b";
        let said_all = |cues: &[Cue]| cues.iter().map(|c| said(c, room)).collect::<Vec<_>>();
        // A room nobody's played in, or only a guest's words: nothing played.
        assert_eq!(Here::recalled(&[]), Here::default());
        let guest = line(&format!(":guest-1a2b!g@h PRIVMSG {room} :hello?"));
        assert_eq!(
            Here::recalled(std::slice::from_ref(&guest)).step,
            Step::Unplayed
        );

        let mut history = said_all(&Script::opening("guest-1a2b"));
        // Cut short by a restart: begun all the same.
        assert_eq!(Here::recalled(&history[..1]).step, Step::Install);
        history.push(guest);
        history.push(said(&Script::only_steps("guest-1a2b"), room));
        let here = Here::recalled(&history);
        assert_eq!((here.step, here.told), (Step::Install, true));
        history.extend(said_all(&Script::script("guest-1a2b-mbp")));
        assert_eq!(Here::recalled(&history).step, Step::Script);
        history.extend(said_all(&script.agent()));
        assert_eq!(Here::recalled(&history).step, Step::Agent);
        history.push(said(&Script::last("guest-1a2b-mbp"), room));
        assert_eq!(Here::recalled(&history).step, Step::Done);
    }

    #[test]
    fn the_host_reads_its_rooms_history_back_and_sets_aside_the_rest() {
        let mut history = History::of("#web/guest-1a2b");
        let mut take = |text: &str| history.take(line(text));
        // Another room's multiline message, before the history begins: not ours.
        assert!(matches!(
            take(":leo!l@h BATCH +x draft/multiline #web/guest-9x8w"),
            Ok(Some(_))
        ));
        assert_eq!(take("BATCH +h chathistory #web/guest-1a2b"), Ok(None));
        assert_eq!(
            take("@batch=h :leo!l@h PRIVMSG #web/guest-1a2b :Morning!"),
            Ok(None)
        );
        // A multiline message inside it, whole.
        assert_eq!(
            take("@batch=h :maya-mbp!m@h BATCH +m draft/multiline #web/guest-1a2b"),
            Ok(None)
        );
        assert_eq!(
            take("@batch=m :maya-mbp!m@h PRIVMSG #web/guest-1a2b :one"),
            Ok(None)
        );
        assert_eq!(
            take("@batch=m :maya-mbp!m@h PRIVMSG #web/guest-1a2b :two"),
            Ok(None)
        );
        assert_eq!(take("BATCH -m"), Ok(None));
        // Live, meanwhile: set aside.
        assert!(matches!(
            take(":guest-1a2b!g@h PRIVMSG #web/guest-1a2b :hi"),
            Ok(Some(_))
        ));
        assert_eq!(take("BATCH -h"), Err(()));
        let said: Vec<String> = history.said.iter().map(|l| l.params[1].clone()).collect();
        assert_eq!(said, ["Morning!", "one\ntwo"]);
    }

    #[test]
    fn the_host_stops_reading_when_its_rooms_history_is_refused() {
        // Refused for this room, it's done; another room's refusal isn't ours.
        let mut history = History::of("#web/guest-1a2b");
        assert!(matches!(
            history.take(line("FAIL CHATHISTORY INVALID_TARGET #web/guest-9x8w :no")),
            Ok(Some(_))
        ));
        assert_eq!(
            history.take(line("FAIL CHATHISTORY INVALID_TARGET #web/guest-1a2b :no")),
            Err(())
        );
    }

    #[tokio::test]
    async fn a_guest_who_comes_back_is_never_walked_through_it_again() {
        let clients = HashMap::new();
        let room = "#web/guest-1a2b";
        let mut here = Here {
            step: Step::Script,
            empty: Some(Instant::now()),
            ..Here::default()
        };
        // Back, after their phone slept: where it was, nothing said again, and they're here.
        let at = Instant::now();
        came(&clients, room, &mut here, "guest-1a2b".into()).await;
        assert!(at.elapsed() < BETWEEN, "the opening played again");
        assert_eq!((here.step, here.empty), (Step::Script, None));
        assert!(here.guests.contains("guest-1a2b"));
    }

    #[test]
    fn each_step_waits_for_what_the_guest_does() {
        let script = Script::showcase();
        let room = "#web/guest-1a2b";
        let mut here = Here {
            step: Step::Install,
            guests: ["guest-1a2b".to_owned()].into(),
            ..Here::default()
        };
        let mut does = |text: &str| {
            let (nick, did) = heard(&line(text), room).expect("heard");
            next(&script, &mut here, nick, did)
                .iter()
                .map(Cue::from)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        // Their browser's words before the terminal: told once that only the steps are
        // scripted, and nothing moves on.
        assert_eq!(
            does(&format!(":guest-1a2b!g@h PRIVMSG {room} :hello?")),
            ["maya"]
        );
        assert!(does(&format!(":guest-1a2b!g@h PRIVMSG {room} :hello??")).is_empty());
        assert_eq!(
            does(&format!(":guest-1a2b-mbp!g@h JOIN {room}")),
            ["leo", "maya"]
        );
        // The ping is theirs to write; the pong, their terminal's.
        assert!(does(&format!(":guest-1a2b!g@h PRIVMSG {room} :ping")).is_empty());
        assert_eq!(
            does(&format!(":guest-1a2b-mbp!g@h PRIVMSG {room} :pong")),
            ["leo", "maya", "maya-mbp", "leo", "maya-mbp", "maya"]
        );
        assert_eq!(
            does(&format!(
                ":guest-1a2b-mbp!g@h PRIVMSG {room} :hi from my agent"
            )),
            ["leo"]
        );
        // Then nothing more, and their terminal's leaving leaves them here.
        assert!(does(&format!(":guest-1a2b-mbp!g@h PRIVMSG {room} :again")).is_empty());
        assert!(does(":guest-1a2b-mbp!g@h QUIT :bye").is_empty());
        assert_eq!((here.step, here.empty), (Step::Done, None));
    }

    fn demo() -> (Demo, mpsc::UnboundedReceiver<(String, Here)>) {
        let (ended, endings) = mpsc::unbounded_channel();
        let demo = Demo {
            api: crate::ergo::Api::new("http://127.0.0.1:9", "token"),
            options: Options {
                irc: "127.0.0.1:9".parse().unwrap(),
                gate: None,
                secret: "s".into(),
            },
            script: Arc::new(Script::showcase()),
            cast: None,
            playing: HashMap::new(),
            kept: HashMap::new(),
            waiting: HashMap::new(),
            ended,
        };
        (demo, endings)
    }

    #[tokio::test]
    async fn what_a_rooms_play_knew_is_kept_for_its_next_while_its_guest_is() {
        let (mut demo, mut endings) = demo();
        let played = || Here {
            step: Step::Script,
            empty: Some(Instant::now()),
            ..Here::default()
        };
        demo.end("#web/guest-1a2b".into(), played()).await;
        // Long gone, as the sweep forgets them: forgotten here too.
        let gone = Here {
            empty: Instant::now().checked_sub(KEPT + Duration::from_mins(1)),
            ..played()
        };
        demo.kept.insert("#web/guest-gone".into(), gone);
        demo.end("#web/guest-9x8w".into(), played()).await;
        assert_eq!(demo.kept["#web/guest-1a2b"].step, Step::Script);
        assert!(!demo.kept.contains_key("#web/guest-gone"));
        // Back: what it knew goes to its next play. Here the cast can't get in, so it stays.
        let arrival = Arrival {
            room: "#web/guest-1a2b".into(),
            guest: "guest-1a2b".into(),
        };
        demo.arrive(arrival, &mut endings).await;
        assert_eq!(demo.kept["#web/guest-1a2b"].step, Step::Script);
        assert!(demo.playing.is_empty());
    }

    #[test]
    fn passphrases_are_stable_and_per_nick() {
        assert_eq!(passphrase("s", "maya"), passphrase("s", "maya"));
        assert_ne!(passphrase("s", "maya"), passphrase("s", "leo"));
    }
}
