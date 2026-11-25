//! The connection: it logs in as a machine's account, hears the queries sent to it, runs the
//! commands its config declares for those who may call them, and answers.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::process::{ChildStdout, Command as Process};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::{debug, info, warn};
use zirc_core::agents::{self, Sender};
use zirc_core::chat::{TYPING_EVERY, TYPING_TAG};
use zirc_irc::multiline::Batches;
use zirc_irc::{Client, Credentials, Line};
use zirc_proto::{Event, Message};

use crate::config::{self, Command, Config, Who};

/// The session this host's answers come from.
const SESSION: &str = "remote";

/// The most progress lines one call sends: a command that prints more is cut, so it can't
/// flood the asker.
const MAX_PROGRESS: usize = 20;

/// What a command prints at the start of a line to say how far it is.
const PROGRESS: &str = "progress: ";

/// Where to connect and who to be.
#[derive(Debug, Clone)]
pub struct Options {
    /// The IRC server's host name.
    pub server: String,
    /// Its port.
    pub port: u16,
    /// Whether to use TLS.
    pub tls: bool,
    /// The machine's account, such as `build-2300`.
    pub account: String,
    /// Its owner's handle, whom it answers.
    pub owner: String,
    /// The machine it runs on, such as a warm CI agent's `ci-1`, when it is another of its
    /// owner's: it says so in its real name (`mattias on ci-1`) and apps draw it under that.
    pub on: Option<String>,
    /// The account's passphrase.
    pub passphrase: String,
    /// The secrets it was started with, by name: a command that names one in its `secrets`
    /// gets it in its environment, and nothing else does.
    pub secrets: BTreeMap<String, String>,
    /// Whether a host says whose machine an account is on a server that isn't zirc.dev's
    /// (`<org>.irc.zirc.dev`, where it always does): only where its operator alone sets hosts
    /// and makes accounts ([`agents::standing`]).
    pub trust_hosts: bool,
}

/// How far a caller may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Standing {
    /// Anyone else: not answered at all, so a stranger learns nothing, not even that the
    /// host is here.
    Nobody,
    /// A machine of the owner, where their agents run.
    Mine,
    /// The owner's own account, or one the config allows.
    Owner,
}

impl Standing {
    fn may(self, who: Who) -> bool {
        match who {
            Who::Owner => self >= Self::Owner,
            Who::Mine => self >= Self::Mine,
        }
    }
}

/// What a connection needs to answer.
struct Host {
    config: Config,
    owner: String,
    secrets: BTreeMap<String, String>,
    /// One lock for each command whose calls don't overlap.
    locks: BTreeMap<String, Arc<Mutex<()>>>,
    /// Whether the server is one of zirc.dev's, where only zirc.dev makes accounts and sets
    /// hosts, so a host says whose machine an account is ([`agents::standing`]).
    named_by_zirc_dev: bool,
}

impl Host {
    fn new(
        config: Config,
        owner: String,
        secrets: BTreeMap<String, String>,
        named_by_zirc_dev: bool,
    ) -> Self {
        let locks = config
            .commands
            .iter()
            .filter(|(_, command)| !command.parallel)
            .map(|(name, _)| (name.clone(), Arc::new(Mutex::new(()))))
            .collect();
        Self {
            config,
            owner,
            secrets,
            locks,
            named_by_zirc_dev,
        }
    }

    /// Who a query's sender is here: the owner or an account the config allows; one of the
    /// owner's machines, where their agents run; or nobody. Whose an account is, is zirc's
    /// one rule ([`agents::standing`]), hosts counting only on a server of zirc.dev's.
    fn standing(&self, sender: Sender<'_>) -> Standing {
        let theirs = agents::standing(sender, Some(&self.owner), self.named_by_zirc_dev);
        let allowed = sender.account().is_some_and(|account| {
            self.config
                .allow
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(account))
        });
        match theirs {
            agents::Standing::Owner => Standing::Owner,
            _ if allowed => Standing::Owner,
            agents::Standing::Machine => Standing::Mine,
            agents::Standing::Member | agents::Standing::Nobody => Standing::Nobody,
        }
    }

    /// How far a person's plain message goes: the owner's own account, or their own client
    /// under another name, is answered; anyone else, the owner's machines included, is not.
    /// An agent asks with a query, which is answered as information; a plain reply to one
    /// would read as its user's own words.
    fn person(&self, sender: Sender<'_>) -> Standing {
        match agents::standing(sender, Some(&self.owner), self.named_by_zirc_dev) {
            agents::Standing::Owner => Standing::Owner,
            _ => Standing::Nobody,
        }
    }

    /// Who a query's sender counts as, and the request it makes. A trusted integration
    /// speaks for the owner with `as <handle> <request>`, and for no one else; every other
    /// sender speaks for itself.
    fn admit<'t>(&self, sender: Sender<'_>, text: &'t str) -> (Standing, &'t str) {
        let trusted = sender.account().is_some_and(|account| {
            self.config
                .trust
                .iter()
                .any(|trusted| trusted.eq_ignore_ascii_case(account))
        });
        if !trusted {
            return (self.standing(sender), text);
        }
        match text
            .strip_prefix("as ")
            .and_then(|rest| rest.split_once(' '))
        {
            Some((handle, request)) if handle.eq_ignore_ascii_case(&self.owner) => {
                (Standing::Owner, request.trim_start())
            }
            _ => (Standing::Nobody, text),
        }
    }
}

/// Connects and serves until the connection ends.
///
/// # Errors
///
/// When it can't connect or log in, or the connection ends.
pub async fn run(options: Options, config: Config) -> Result<()> {
    let owner = options.owner.to_lowercase();
    let mut connection = zirc_irc::Config::new(&options.server, options.port, &options.account);
    connection.tls = options.tls;
    // A machine gives its owner as its real name, as zircd does: apps know it's theirs by it.
    connection.realname = match options.on.as_deref().filter(|on| !on.is_empty()) {
        Some(on) => format!("{owner} on {on}"),
        None => owner.clone(),
    };
    connection.sasl = Some(Credentials {
        account: options.account.clone(),
        password: options.passphrase.clone(),
    });
    let (client, mut incoming) = zirc_irc::connect(&connection)
        .await
        .context("couldn't connect and log in")?;
    for channel in &config.channels {
        client
            .join(channel)
            .with_context(|| format!("couldn't join {channel}"))?;
    }
    info!(nick = client.nick(), %owner, "serving {} commands", config.commands.len());
    // An IRC server's host name, as zirc.dev names its servers (`<org>.irc.zirc.dev`).
    let named_by_zirc_dev =
        options.trust_hosts || agents::is_zirc_dev(&format!("https://{}", options.server));
    let host = Arc::new(Host::new(config, owner, options.secrets, named_by_zirc_dev));
    // A long query comes as a multiline batch, joined here into the one message it is.
    let mut batches = Batches::default();
    while let Some(line) = incoming.recv().await {
        let Some(line) = batches.feed(line) else {
            continue;
        };
        if line.command != "PRIVMSG" {
            continue;
        }
        let [target, body] = line.params.as_slice() else {
            continue;
        };
        // Only what's said to this nick alone.
        if !target.eq_ignore_ascii_case(client.nick()) {
            continue;
        }
        let Some(from) = line
            .source
            .as_deref()
            .and_then(|source| source.split('!').next())
        else {
            continue;
        };
        let sender = Sender::of(line.tag("account"), line.source.as_deref());
        // A query, from an agent or from a person; or a person's own message, as an app or
        // a plain IRC client sends it, which is a command to run: each is answered in kind.
        let (how, request, standing) = match zirc_proto::decode(body) {
            Ok(Some(message)) => match message.event {
                Event::Query { id, text, .. } => {
                    let (standing, request) = host.admit(sender, &text);
                    let asker = message.session;
                    (Reply::Answer { id, asker }, request.to_owned(), standing)
                }
                Event::Text { body } => (Reply::Chat, unaddressed(&body), host.person(sender)),
                _ => continue,
            },
            Ok(None) => (Reply::Chat, unaddressed(body), host.person(sender)),
            Err(_) => continue,
        };
        if standing == Standing::Nobody {
            debug!(from, "a message from someone who may not ask: not answered");
            continue;
        }
        let id = match &how {
            Reply::Answer { id, .. } => id.clone(),
            Reply::Chat => line.tag("msgid").unwrap_or("chat").to_owned(),
        };
        tokio::spawn(answer(
            client.clone(),
            Arc::clone(&host),
            from.to_owned(),
            how,
            id,
            request,
            standing,
        ));
    }
    bail!(
        "the connection ended{}",
        client
            .ended()
            .map(|why| format!(": {why}"))
            .unwrap_or_default()
    )
}

/// Answers what `from` asked, as `how` says: progress as it comes, then the reply; typing
/// meanwhile, for a person, as their app shows.
async fn answer(
    client: Client,
    host: Arc<Host>,
    from: String,
    how: Reply,
    id: String,
    request: String,
    standing: Standing,
) {
    let (progress, mut said) = unbounded_channel::<String>();
    let forward = async {
        while let Some(text) = said.recv().await {
            speak(&client, &from, &how, &format!("{PROGRESS}{text}"));
        }
    };
    let typing = matches!(how, Reply::Chat).then(|| {
        let (client, to) = (client.clone(), from.clone());
        tokio::spawn(async move {
            let every = TYPING_EVERY.to_std().unwrap_or(Duration::from_secs(3));
            loop {
                typing_now(&client, &to, "active");
                tokio::time::sleep(every).await;
            }
        })
    });
    let (reply, ()) = tokio::join!(
        respond(&host, standing, &from, &id, request.trim(), progress),
        forward
    );
    speak(&client, &from, &how, &reply);
    if let Some(typing) = typing {
        typing.abort();
        typing_now(&client, &from, "done");
    }
}

/// How a reply goes back.
enum Reply {
    /// As the answer to a query, which `asker`, the session that made it, hears: what an agent
    /// is given, and zirc shows as information, never as the person's own words.
    Answer { id: String, asker: String },
    /// As a plain message in a person's conversation with this host, which their app shows
    /// as it shows any message. Only the owner's own account is ever answered so.
    Chat,
}

/// A person's message without the session it's addressed to, as an app starts one written in
/// a session's conversation, `remote: help`: the host is that session.
fn unaddressed(text: &str) -> String {
    let text = text.trim_start();
    text.get(..SESSION.len() + 1)
        .filter(|head| head.eq_ignore_ascii_case(&format!("{SESSION}:")))
        .map_or(text, |head| text[head.len()..].trim_start())
        .to_owned()
}

/// Tells `to` this host is typing (`active`), or has stopped (`done`): IRCv3's `+typing`,
/// which the server keeps in no history.
fn typing_now(client: &Client, to: &str, state: &str) {
    let mut line = Line::new("TAGMSG", [to]);
    line.tags = vec![(TYPING_TAG.to_owned(), Some(state.to_owned()))];
    if let Err(error) = client.send(line) {
        debug!(%error, %to, "couldn't say it's typing");
    }
}

/// Sends `text` to `to` as `how` says: a long reply in parts, in order, each one whole.
fn speak(client: &Client, to: &str, how: &Reply, text: &str) {
    for part in zirc_core::send::in_parts(text, zirc_core::send::PART_BYTES) {
        // An answer is this host's, as `<machine>/remote`, to the agent that asked; a reply in
        // a person's conversation is the machine's own, without a session, so their app shows
        // it in the thread they wrote in, not in a session's conversation beside it.
        let (session, event) = match how {
            Reply::Answer { id, asker } => (
                SESSION,
                Event::Answer {
                    query: id.clone(),
                    to: Some(asker.clone()),
                    text: part,
                },
            ),
            Reply::Chat => (zirc_proto::NO_SESSION, Event::Text { body: part }),
        };
        if let Err(error) = zirc_core::send::send_message(client, to, &Message::new(session, event))
        {
            warn!(%error, %to, "couldn't answer");
            return;
        }
    }
}

/// The answer to one query from someone who may ask. A command that prints progress says it
/// on `progress` as it goes.
async fn respond(
    host: &Host,
    standing: Standing,
    from: &str,
    id: &str,
    text: &str,
    progress: UnboundedSender<String>,
) -> String {
    let Some(config::Request {
        command: name,
        named,
        rest,
    }) = config::request(text)
    else {
        return "say a command; `help` lists them".to_owned();
    };
    match name.as_str() {
        "ping" => return "pong".to_owned(),
        "version" => return zirc_proto::version::get().to_owned(),
        "help" => return help(host, standing),
        _ => {}
    }
    // What isn't a command goes, whole, to the one the config says, if it says one.
    let (name, named, rest) = match (host.config.commands.get(&name), &host.config.otherwise) {
        (None, Some(otherwise)) => (otherwise.clone(), Vec::new(), text.to_owned()),
        _ => (name, named, rest),
    };
    info!(%from, command = %name, "called");
    let Some(command) = host.config.commands.get(&name) else {
        return format!("no command `{name}`; `help` lists them");
    };
    if !standing.may(command.who) {
        return format!("refused: `{name}` is for the owner's own account");
    }
    let values = match command.bind(named, &rest) {
        Ok(values) => values,
        Err(why) => return format!("{name}: {why}"),
    };
    // Held while it runs: a second call meanwhile is told so, not queued behind it.
    let _running: Option<OwnedMutexGuard<()>> = match host.locks.get(&name) {
        Some(lock) => match Arc::clone(lock).try_lock_owned() {
            Ok(guard) => Some(guard),
            Err(_) => return format!("`{name}` is already running; try again when it's done"),
        },
        None => None,
    };
    match execute(host, command, &values, from, id, &progress).await {
        Ok(output) => output,
        Err(error) => format!("{name} failed: {error:#}"),
    }
}

/// What the caller may call.
fn help(host: &Host, standing: Standing) -> String {
    let mut lines = vec![
        "ping: answers pong".to_owned(),
        "version: this host's version".to_owned(),
    ];
    lines.extend(
        host.config
            .commands
            .iter()
            .filter(|(_, command)| standing.may(command.who))
            .map(|(name, command)| command.describe(name)),
    );
    lines.join("\n")
}

/// Runs the command, and returns its output or says why it failed.
async fn execute(
    host: &Host,
    command: &Command,
    values: &BTreeMap<String, String>,
    from: &str,
    id: &str,
    progress: &UnboundedSender<String>,
) -> Result<String> {
    let line = command.argv(values);
    let (program, rest) = line.split_first().context("it has nothing to run")?;
    let mut process = Process::new(program);
    process
        .args(rest)
        .env_clear()
        .env("ZIRC_FROM", from)
        .env("ZIRC_REQUEST", id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // The program is found as this host finds it; the rest of its environment is only what
    // the config lists.
    if let Some(path) = std::env::var_os("PATH") {
        process.env("PATH", path);
    }
    for name in &host.config.limits.env {
        if let Some(value) = std::env::var_os(name) {
            process.env(name, value);
        }
    }
    for (key, value) in values {
        process.env(
            format!("ZIRC_ARG_{}", key.to_uppercase().replace('-', "_")),
            value,
        );
    }
    // Its secrets, and no other command's: a missing one says which, never what.
    for (variable, secret) in &command.secrets {
        let value = host.secrets.get(secret).with_context(|| {
            format!("it needs the secret `{secret}`, which this host wasn't given")
        })?;
        process.env(variable, value);
    }
    let timeout = command.timeout(&host.config.limits)?;
    let limit = host.config.limits.output_bytes;
    let mut child = process.spawn().context("couldn't start")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let mut stderr = child.stderr.take().context("no stderr")?;
    let run = async {
        let mut err = Vec::new();
        let (out, _, status) = tokio::join!(
            read_output(stdout, progress),
            stderr.read_to_end(&mut err),
            child.wait()
        );
        (out, err, status)
    };
    let Ok((out, err, status)) = tokio::time::timeout(timeout, run).await else {
        bail!("it ran past {} and was stopped", seconds(timeout));
    };
    let out = out.context("couldn't read its output")?;
    let status = status.context("couldn't wait for it")?;
    if status.success() {
        let out = out.trim();
        return Ok(if out.is_empty() {
            "done".to_owned()
        } else {
            cut(out, limit)
        });
    }
    let err = String::from_utf8_lossy(&err);
    let why = err
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .or_else(|| out.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or("it said nothing");
    bail!(
        "exit {}: {}",
        status
            .code()
            .map_or_else(|| "by a signal".to_owned(), |code| code.to_string()),
        cut(why, limit)
    )
}

/// A time as people read it: `120s`, or `500ms` under a second.
fn seconds(time: std::time::Duration) -> String {
    if time.as_secs() == 0 {
        format!("{}ms", time.as_millis())
    } else {
        format!("{}s", time.as_secs())
    }
}

/// What a command prints, less its progress lines, which go on `progress` as they come, at
/// most [`MAX_PROGRESS`] of them.
async fn read_output(
    stdout: ChildStdout,
    progress: &UnboundedSender<String>,
) -> std::io::Result<String> {
    let mut reader = BufReader::new(stdout);
    let (mut kept, mut said, mut buffer) = (Vec::new(), 0, Vec::new());
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer).await? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buffer);
        let text = text.trim_end_matches(['\n', '\r']);
        match text.strip_prefix(PROGRESS) {
            Some(update) => {
                if said < MAX_PROGRESS {
                    // Nobody listening is no reason to stop the command.
                    let _ = progress.send(update.to_owned());
                    said += 1;
                }
            }
            None => kept.push(text.to_owned()),
        }
    }
    Ok(kept.join("\n"))
}

/// `text` in at most `limit` bytes, cut at a character and marked when it was.
fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (cut)", &text[..end])
}

#[cfg(test)]
mod tests {
    use zirc_proto::names::Hosted;

    use super::*;

    /// The sender logged in as `account`, a machine of `machine_of`'s when its host says so.
    fn sender<'a>(account: Option<&'a str>, machine_of: Option<&'a str>) -> Sender<'a> {
        Sender::new(account, machine_of.map(Hosted::Machine))
    }

    fn host(allow: &[&str], trust: &[&str]) -> Host {
        Host::new(
            Config {
                allow: allow.iter().map(|&a| a.to_owned()).collect(),
                trust: trust.iter().map(|&a| a.to_owned()).collect(),
                ..Config::default()
            },
            "mattias".to_owned(),
            BTreeMap::new(),
            true,
        )
    }

    #[test]
    fn a_host_says_nothing_on_a_server_not_zirc_dev_s() {
        let mut host = host(&[], &[]);
        host.named_by_zirc_dev = false;
        assert_eq!(
            host.standing(sender(Some("m3-mac"), Some("mattias"))),
            Standing::Nobody
        );
        let client = Sender::new(Some("mattias-irc"), Some(Hosted::Client("mattias")));
        assert_eq!(host.person(client), Standing::Nobody);
        assert_eq!(
            host.standing(sender(Some("mattias"), None)),
            Standing::Owner
        );
        assert!(agents::is_zirc_dev("https://acme.irc.zirc.dev"));
    }

    #[test]
    fn the_owner_and_their_machines_are_told_apart_from_everyone_else() {
        let host = host(&["zirc-ops"], &[]);
        assert_eq!(
            host.standing(sender(Some("mattias"), None)),
            Standing::Owner
        );
        assert_eq!(
            host.standing(sender(Some("Mattias"), None)),
            Standing::Owner
        );
        assert_eq!(
            host.standing(sender(Some("m3-mac"), Some("mattias"))),
            Standing::Mine
        );
        assert_eq!(
            host.standing(sender(Some("zirc-ops"), None)),
            Standing::Owner
        );
        // A name alone, or someone else's machine, isn't theirs.
        assert_eq!(
            host.standing(sender(Some("mattias-mbp"), None)),
            Standing::Nobody
        );
        assert_eq!(
            host.standing(sender(Some("m3-mac"), Some("bob"))),
            Standing::Nobody
        );
        assert_eq!(
            host.standing(sender(Some("someone"), None)),
            Standing::Nobody
        );
        assert_eq!(host.standing(sender(Some("*"), None)), Standing::Nobody);
        assert_eq!(host.standing(sender(None, None)), Standing::Nobody);
    }

    #[test]
    fn only_the_owners_own_account_is_answered_in_plain_messages() {
        let host = host(&["zirc-ops"], &["zirc-alerts"]);
        assert_eq!(host.person(sender(Some("mattias"), None)), Standing::Owner);
        assert_eq!(host.person(sender(Some("Mattias"), None)), Standing::Owner);
        // Their machines, accounts the config allows or trusts, and strangers ask with queries
        // or not at all.
        assert_eq!(
            host.person(sender(Some("mattias-mbp"), None)),
            Standing::Nobody
        );
        assert_eq!(
            host.person(sender(Some("zirc-ops"), None)),
            Standing::Nobody
        );
        assert_eq!(
            host.person(sender(Some("zirc-alerts"), None)),
            Standing::Nobody
        );
        assert_eq!(host.person(sender(Some("zed"), None)), Standing::Nobody);
        assert_eq!(host.person(sender(None, None)), Standing::Nobody);
        // Their own client under another name, as its host says, is them; a machine never is.
        let client = Sender::new(Some("mattias-irc"), Some(Hosted::Client("mattias")));
        assert_eq!(host.person(client), Standing::Owner);
        let machine = sender(Some("m3-mac"), Some("mattias"));
        assert_eq!(host.person(machine), Standing::Nobody);
    }

    #[test]
    fn a_machine_may_call_what_is_for_mine_and_not_what_is_for_the_owner() {
        assert!(Standing::Mine.may(Who::Mine));
        assert!(!Standing::Mine.may(Who::Owner));
        assert!(Standing::Owner.may(Who::Mine));
        assert!(Standing::Owner.may(Who::Owner));
    }

    #[test]
    fn a_trusted_integration_speaks_for_the_owner_and_no_one_else() {
        let host = host(&[], &["zirc-alerts"]);
        let admit = |account, text| host.admit(sender(Some(account), None), text);
        assert_eq!(
            admit("zirc-alerts", "as mattias kill"),
            (Standing::Owner, "kill")
        );
        assert_eq!(
            admit("ZIRC-ALERTS", "as Mattias  log lines=3"),
            (Standing::Owner, "log lines=3")
        );
        assert_eq!(
            admit("zirc-alerts", "as zed kill"),
            (Standing::Nobody, "as zed kill")
        );
        assert_eq!(admit("zirc-alerts", "kill").0, Standing::Nobody);
        // Nobody else may claim to speak for the owner.
        assert_eq!(admit("zed-box", "as mattias kill").0, Standing::Nobody);
        assert_eq!(
            host.admit(sender(Some("m3-mac"), Some("mattias")), "kill"),
            (Standing::Mine, "kill")
        );
        assert_eq!(
            host.admit(sender(None, None), "as mattias kill").0,
            Standing::Nobody
        );
    }

    #[test]
    fn a_message_addressed_to_the_hosts_session_is_read_without_the_address() {
        assert_eq!(unaddressed("remote: help"), "help");
        assert_eq!(unaddressed("Remote:status"), "status");
        assert_eq!(unaddressed("  help"), "help");
        assert_eq!(unaddressed("remotes: help"), "remotes: help");
        assert_eq!(unaddressed("é"), "é");
    }

    #[test]
    fn says_a_time_as_people_read_it() {
        assert_eq!(seconds(std::time::Duration::from_millis(300)), "300ms");
        assert_eq!(seconds(std::time::Duration::from_secs(120)), "120s");
    }

    #[test]
    fn cuts_at_a_character() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("åäö", 3), "å… (cut)");
    }
}
