//! The server's workflows ([`zirc_proto::workflow`]): each runs as a user of its own, the nick
//! its file names, which keeps its runs' cards up to date in its channel.
//!
//! Everything comes from what the nicks hear, as it happens, as any IRC client hears it: a
//! workflow's file deployed to `#workflows` ([`zirc_proto::workflow::Deploy`]), heard by the
//! host, `zirc-workflows`; and, by each workflow's nick in its channels, the states said
//! ([`zirc_proto::state`]), what workers say of a step ([`zirc_proto::run::StepAction`]) and
//! do, mentions of it, 👍 or 👎 on an approval, and the machines' sessions and their roles.
//! A nick listens where its file says ([`Workflow::channels`]) and wherever anyone in the
//! organisation invites it (kept beside the history, [`KEPT`]); kicked, it stays away.
//! Nothing runs on a timer: the engines are woken by what's heard and at their next time
//! ([`Hosted::next_due`]).
//!
//! Joining a channel, a nick catches up on its history since [`WINDOW`] ago (`#workflows`
//! whole) over IRC ([`connection`]): its workflow's own run events put its engine
//! ([`zirc_workflows::Engine`]) where it was, and what came after its read marker there, if
//! not more than [`MISSED`] ago, is acted on; the host keeps the markers ([`KEPT`]). A nick with no marker in a channel acts on
//! nothing before it joined. A run started before the window is forgotten, and a deploy the
//! history no longer keeps is gone: a stopgap while workflows are tried out.
//!
//! Each workflow's nick has an account made when it's deployed, with a passphrase derived from
//! the endpoint's token, as an integration's is ([`crate::say`]), and one connection to Ergo's
//! loopback, which says what its engine says. A nick that's already someone
//! else's account is refused, never taken over: its passphrase isn't the derived one, so its
//! login fails, and the deploy is answered with why, as `zirc-workflows`
//! ([`zirc_proto::workflow::HOST`]).
//!
//! Anyone in the organisation may deploy a workflow, from their own account or one of their
//! machines; guests, integrations and workflows may not. Nothing anyone says is logged: only
//! which workflow did what.
//!
//! Workflows are off unless zirc.dev turns them on for the organisation (`spec.workflows`, as
//! [`crate::gateway::Public::workflows`]): until then nothing connects, and `zirc workflow
//! deploy`, finding no host, sends nothing. On, the host is in `#workflows` alone, and each
//! workflow's nick connects once its file is.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{info, warn};
use zirc_proto::run::{Act, Change, RunEvent, StepAction};
use zirc_proto::state::State;
use zirc_proto::workflow::{self, Deploy, Listed, Refused, Workflow};
use zirc_proto::{Event, Message};
use zirc_workflows::{Actor, Did, Engine, Input, Said};

use crate::exporter::{PRIVMSG, Said as Item, TAGMSG};
use crate::machines::Machines;

/// The nicks the server's workflows run as now: zirc's own accounts, always connected, which
/// count as nobody (`crate::accounts::sessions`).
static NICKS: std::sync::LazyLock<std::sync::RwLock<std::collections::BTreeSet<String>>> =
    std::sync::LazyLock::new(std::sync::RwLock::default);

/// Whether `account` is zirc's own, not a person's or a machine's: the recorder, the host of
/// the workflows, or a workflow's nick.
#[must_use]
pub fn is_zircs_own(account: &str) -> bool {
    let account = account.to_lowercase();
    account == zirc_proto::activity::RECORDER
        || account == workflow::HOST
        || NICKS
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&account)
}

/// The workflows the server runs now, as its host last had them ([`Hosted::listed`]): what
/// the account endpoint lists, which the host keeps as its runs move.
static LISTED: std::sync::LazyLock<std::sync::RwLock<(Vec<Listed>, Vec<Refused>)>> =
    std::sync::LazyLock::new(std::sync::RwLock::default);

/// The workflows the server runs now with their runs that aren't finished, and the deploys
/// it didn't take ([`Hosted::refusals`]); none while workflows are off.
#[must_use]
pub fn listed() -> (Vec<Listed>, Vec<Refused>) {
    LISTED
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn list(listed: Vec<Listed>, refused: Vec<Refused>) {
    *LISTED
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = (listed, refused);
}

/// How long a nick's connection waits before connecting again, at first and at most.
const RETRY: Duration = Duration::from_secs(5);
const RETRY_MOST: Duration = Duration::from_mins(5);

/// How long connecting may take.
const WITHIN: Duration = Duration::from_secs(8);

/// How many messages the host remembers the words of, for reactions to them.
const RECENT: usize = 2000;

/// How far back a nick catches up on a channel's history when it joins, but `#workflows`,
/// which it reads whole.
pub const WINDOW: Duration = Duration::from_hours(24);

/// A message in a channel's history.
pub(crate) struct Row {
    /// Its id on the server.
    pub msgid: String,
    /// The channel, as the history keeps it.
    pub channel: String,
    /// When, in unix seconds, as the server says.
    pub at: u64,
    /// The message.
    pub item: Item,
}

/// Who an account is, for a workflow: a person, a machine's session, or nobody who may act.
pub type Who<'a> = &'a dyn Fn(&str, Option<&str>) -> Option<Actor>;

/// What the host does, as its workflows say.
#[derive(Debug)]
pub enum Out {
    /// Says `message` in `channel` as `nick`.
    Post {
        /// The workflow's nick, or [`workflow::HOST`].
        nick: String,
        /// Where.
        channel: String,
        /// What.
        message: Message,
    },
    /// Keeps `nick` connected, in `channels` (and where it was invited).
    Bot {
        /// The workflow's nick.
        nick: String,
        /// Where its file says it listens ([`Workflow::channels`]).
        channels: Vec<String>,
    },
    /// A workflow deployed just now, to install once its nick's account is surely its own.
    Check(Candidate),
}

/// A workflow deployed just now.
#[derive(Debug)]
pub struct Candidate {
    workflow: Workflow,
    version: String,
    /// The file, as deployed.
    source: String,
    /// The person who deployed it: themselves, or from one of their machines.
    by: String,
}

/// A workflow running.
struct Live {
    engine: Engine,
    /// Its file, as deployed, and which of its files that is.
    source: String,
    version: String,
}

/// The server's workflows, as the history says: no I/O.
#[derive(Default)]
pub struct Hosted {
    live: BTreeMap<String, Live>,
    /// Each workflow's changes, oldest first, to rebuild it when it's deployed again: every
    /// change of a run going on, and only the last of one that's finished, which numbers the
    /// next run after it.
    logs: BTreeMap<String, Vec<Kept>>,
    /// The newest of each state.
    states: BTreeMap<String, State>,
    /// The messages about runs going on, by id, with the workflow and the run: a 👍 or 👎 on
    /// any of them answers the run's approval, if that's where it stands.
    messages: BTreeMap<String, (String, String)>,
    /// What each machine in the organisation's channel last said of its live sessions,
    /// with their roles, by its nick.
    machines: BTreeMap<String, zirc_proto::session::Live>,
    /// When each role's member was last given a step, by `machine/session`.
    given: BTreeMap<String, u64>,
    /// Who first deployed each workflow: they, and the organisation's owners and admins, may
    /// replace it ([`Hosted::deploy`]).
    deployers: BTreeMap<String, String>,
    /// Deploys read back when the sidecar started, by name and version, not installed until
    /// the host's own word that it deployed them is read back too ([`Hosted::deployed`]): one
    /// it refused when it was said stays refused.
    read_back: BTreeMap<(String, String), Candidate>,
    /// What was said lately where the workflows listen, by message id, oldest first in
    /// `recent_order`: its words, who said them and where, for a `reaction` trigger to give
    /// its run. The newest [`RECENT`] only, and never written anywhere.
    recent: HashMap<String, (String, String, String)>,
    recent_order: std::collections::VecDeque<String>,
    /// The last deploy of each name it didn't take, heard live, until one of that name is
    /// installed: what the dashboard shows whoever deployed it there.
    refused: BTreeMap<String, Refused>,
    /// The organisation's owners and admins, and whose each machine is, as the server was
    /// last told ([`Hosted::know`]).
    admins: BTreeSet<String>,
    owners: BTreeMap<String, String>,
}

/// A run's change, kept without its card.
struct Kept {
    run: String,
    at: u64,
    change: Change,
}

impl Hosted {
    /// How many workflows run.
    #[must_use]
    pub fn count(&self) -> usize {
        self.live.len()
    }

    /// The workflows running and their runs that aren't finished, by name: what
    /// [`zirc_proto::workflow::LIST_PATH`] lists.
    #[must_use]
    pub fn listed(&self) -> Vec<Listed> {
        self.live
            .iter()
            .map(|(name, live)| {
                let workflow = live.engine.workflow();
                Listed {
                    name: name.clone(),
                    nick: workflow.nick.clone(),
                    description: workflow.description.clone(),
                    channel: workflow.channel.clone(),
                    version: live.version.clone(),
                    by: self.deployers.get(name).cloned(),
                    source: live.source.clone(),
                    runs: live.engine.going(),
                }
            })
            .collect()
    }

    /// The deploys it didn't take, by name.
    #[must_use]
    pub fn refusals(&self) -> Vec<Refused> {
        self.refused.values().cloned().collect()
    }

    /// Notes that `candidate` wasn't taken, and why, as the host says it ([`Out::Post`]).
    pub fn refuse(&mut self, candidate: &Candidate, said: &Out) {
        if let Out::Post { message, .. } = said
            && let Event::Text { body } = &message.event
        {
            self.refused.insert(
                candidate.workflow.name.clone(),
                Refused {
                    name: candidate.workflow.name.clone(),
                    version: candidate.version.clone(),
                    why: body.clone(),
                },
            );
        }
    }

    /// Notes that `nick` has read back `channel`'s history: its workflow's own channel read,
    /// it knows its runs, and may start one when a time comes round.
    pub fn caught(&mut self, nick: &str, channel: &str) {
        if let Some(name) = self.named(nick)
            && let Some(live) = self.live.get_mut(&name)
            && live.engine.workflow().channel.eq_ignore_ascii_case(channel)
        {
            live.engine.caught_up();
        }
    }

    /// Keeps the words of a message heard, for a reaction to it later ([`Hosted::recent`]).
    fn remember(&mut self, row: &Row, message: Option<&Message>, account: &str, channel: &str) {
        if row.msgid.is_empty() || self.recent.contains_key(&row.msgid) {
            return;
        }
        let text = message.map_or_else(|| row.item.text.clone(), |m| m.event.human_text());
        self.recent.insert(
            row.msgid.clone(),
            (text, account.to_owned(), channel.to_owned()),
        );
        self.recent_order.push_back(row.msgid.clone());
        if self.recent_order.len() > RECENT
            && let Some(old) = self.recent_order.pop_front()
        {
            self.recent.remove(&old);
        }
    }

    /// Whether the workflow `w` follows what's said and reacted to in `channel`: its own
    /// channel and those its file follows.
    fn listens(w: &Workflow, channel: &str) -> bool {
        std::iter::once(&w.channel)
            .chain(&w.follow)
            .any(|c| c.eq_ignore_ascii_case(channel))
    }

    /// What `by`'s words in `channel` start, of the workflows listening there: a `said`
    /// trigger's run, unless they were to the workflow's own nick, which a mention answers.
    fn said(&mut self, channel: &str, by: &str, text: &str, id: &str, now: u64) -> Vec<Out> {
        let mut outs = Vec::new();
        for running in self.live.values_mut() {
            let w = running.engine.workflow();
            if !Self::listens(w, channel) || addressed(text, &w.nick).is_some() {
                continue;
            }
            let input = Input::Said {
                by: by.to_owned(),
                channel: channel.to_owned(),
                id: id.to_owned(),
                text: text.to_owned(),
            };
            let said = running.engine.handle(input, now);
            outs.extend(posts(running.engine.workflow(), said));
        }
        outs
    }

    /// What a person's `reaction` to the message `id` starts: a `reaction` trigger's run,
    /// given the message's words if they're still known. Only a person's own account counts.
    fn reacted(
        &mut self,
        row: &Row,
        reaction: &str,
        id: &str,
        account: &str,
        who: Who<'_>,
        now: u64,
    ) -> Vec<Out> {
        let Some(by) = who(account, None).filter(|by| by.person) else {
            return Vec::new();
        };
        let channel = row.channel.to_lowercase();
        let known = self.recent.get(id).cloned();
        let mut outs = Vec::new();
        for running in self.live.values_mut() {
            if !Self::listens(running.engine.workflow(), &channel) {
                continue;
            }
            let input = Input::Reacted {
                by: by.name(),
                reaction: reaction.to_owned(),
                channel: channel.clone(),
                id: id.to_owned(),
                text: known.as_ref().map(|(text, ..)| text.clone()),
                author: known.as_ref().map(|(_, author, _)| author.clone()),
            };
            let said = running.engine.handle(input, now);
            outs.extend(posts(running.engine.workflow(), said));
        }
        outs
    }

    /// What a landing CI says passed starts: a `landed` trigger's run, in every workflow.
    fn landed(&mut self, data: &zirc_proto::ci::Data, now: u64) -> Vec<Out> {
        if data.state.as_deref() != Some("passed") {
            return Vec::new();
        }
        let landing: BTreeMap<String, String> = [
            ("commit", &data.commit),
            ("branch", &data.branch),
            ("session", &data.session),
            ("build", &data.build),
            ("link", &data.url),
            ("subject", &data.subject),
        ]
        .into_iter()
        .filter_map(|(field, value)| Some((field.to_owned(), value.clone()?)))
        .collect();
        let mut outs = Vec::new();
        for running in self.live.values_mut() {
            let input = Input::Landed {
                landing: landing.clone(),
                changed: data.changed.clone(),
            };
            let said = running.engine.handle(input, now);
            outs.extend(posts(running.engine.workflow(), said));
        }
        outs
    }

    /// The workflow `nick` runs, if one does.
    fn named(&self, nick: &str) -> Option<String> {
        self.live
            .iter()
            .find(|(_, l)| l.engine.workflow().nick.eq_ignore_ascii_case(nick))
            .map(|(name, _)| name.clone())
    }

    /// Tells it the organisation's owners and admins, and each machine's owner, which say
    /// who may replace a workflow.
    pub fn know(&mut self, admins: BTreeSet<String>, owners: BTreeMap<String, String>) {
        self.admins = admins;
        self.owners = owners;
    }

    /// The person behind `account`: a machine's owner, whose word its deploy is, or else the
    /// account itself.
    fn person(&self, account: &str) -> String {
        self.owners
            .get(account)
            .map_or_else(|| account.to_owned(), |owner| owner.to_lowercase())
    }

    /// Why `person` can't replace the workflow `name` now, if it runs already: it's its
    /// deployer's and the owners' and admins' to replace, and not under a run that's going,
    /// which would carry on by other steps than it started with (an approval taken out, say).
    fn kept_from(&self, name: &str, person: &str) -> Option<String> {
        let running = self.live.get(name)?;
        let deployer = self.deployers.get(name);
        if deployer.is_some_and(|by| by != person) && !self.admins.contains(person) {
            return Some(format!(
                "`{name}` is @{}'s: they, an owner or an admin can replace it",
                deployer.map_or("", String::as_str)
            ));
        }
        let going = running.engine.running();
        (!going.is_empty()).then(|| {
            format!(
                "`{name}` has {} going: cancel it or let it finish, then deploy again",
                going.join(", ")
            )
        })
    }

    /// The host's own word, read back, that it deployed something: the deploy read back
    /// before it is installed now.
    fn acknowledged(&mut self, message: Option<&Message>) -> Vec<Out> {
        let Some(Event::Text { body }) = message.map(|m| &m.event) else {
            return Vec::new();
        };
        let Some((name, version)) = deployed_in(body) else {
            return Vec::new();
        };
        match self.read_back.remove(&(name, version)) {
            Some(candidate) => self.install(candidate).into_iter().collect(),
            None => Vec::new(),
        }
    }

    /// What `row` does, said `live` (rather than read back when the sidecar started), at
    /// `now`, knowing accounts by `who`.
    pub(crate) fn take(&mut self, row: &Row, live: bool, who: Who<'_>, now: u64) -> Vec<Out> {
        let account = row.item.account.to_lowercase();
        if account.is_empty() || account == "*" {
            return Vec::new();
        }
        if row.item.kind == TAGMSG {
            return if live && self.named(&account).is_none() {
                self.reaction(row, &account, who, now)
            } else {
                Vec::new()
            };
        }
        if row.item.kind != PRIVMSG {
            return Vec::new();
        }
        let Ok(message) = zirc_proto::decode(&row.item.text) else {
            return Vec::new();
        };
        if account == workflow::HOST {
            // The host's own: read back, what it says it deployed.
            return if live {
                Vec::new()
            } else {
                self.acknowledged(message.as_ref())
            };
        }
        if let Some(name) = self.named(&account) {
            // A workflow's own: its run events, read back or heard again.
            if let Some(event) = message.and_then(|m| RunEvent::of(&m.event)) {
                self.ran(&name, event, &row.msgid, live);
            }
            return Vec::new();
        }
        let mut outs = match seen(message.as_ref(), &account, who) {
            Some(seen) => self.activity(&seen, live, row.at, now),
            None => Vec::new(),
        };
        outs.extend(self.route(row, message, live, who, now));
        outs
    }

    /// What `row`'s `message` (none for a plain IRC line) asks of the workflows: a deploy, a
    /// state, a worker's step, or a mention.
    fn route(
        &mut self,
        row: &Row,
        message: Option<Message>,
        live: bool,
        who: Who<'_>,
        now: u64,
    ) -> Vec<Out> {
        let (channel, account) = (
            &row.channel.to_lowercase(),
            &row.item.account.to_lowercase(),
        );
        self.remember(row, message.as_ref(), account, channel);
        let Some(message) = message else {
            if !live {
                return Vec::new();
            }
            let text = &row.item.text;
            let mut outs = self.mention(channel, account, None, text, who, now);
            // A person's words; a machine's plain lines are nobody's.
            if let Some(by) = who(account, None).filter(|by| by.person) {
                outs.extend(self.said(channel, &by.name(), text, &row.msgid, now));
            }
            return outs;
        };
        if let Some(deploy) = Deploy::of(&message.event) {
            if channel == workflow::CHANNEL {
                return self.deploy(&deploy, account, live, who);
            }
            return Vec::new();
        }
        if let Some(state) = State::of(&message.event) {
            // How something stands is the team's and its CI's to say. A guest's word for it,
            // which the gateway passes on as it does any line of theirs, starts no run and
            // moves none on.
            let may = who(account, Some(&message.session)).is_some()
                || zirc_proto::names::is_integration_nick(account);
            return if may {
                self.state(&state, live, now)
            } else {
                Vec::new()
            };
        }
        if !live {
            return Vec::new();
        }
        if let Some(action) = StepAction::of(&message.event) {
            return self.act(channel, account, &message.session, action, who, now);
        }
        match &message.event {
            Event::Text { body } => {
                let session = Some(message.session.as_str());
                let mut outs = self.mention(channel, account, session, body, who, now);
                // A person's words from one of their apps; an agent session's start nothing.
                if let Some(by) = who(account, session).filter(|by| by.person) {
                    outs.extend(self.said(channel, &by.name(), body, &row.msgid, now));
                }
                outs
            }
            // CI landed something: what a `landed` trigger starts a run on.
            Event::Custom { kind, data, .. }
                if kind == zirc_proto::ci::LANDING_KIND && account == zirc_proto::ci::NICK =>
            {
                self.landed(&zirc_proto::ci::Data::of(data), now)
            }
            // An integration, such as CI saying a test was flaky, starts a run by naming the
            // workflow in its event's text, as `@flake_bot run test=//web:unit`: it's the
            // team's own, as its states are, though no person or machine holds it.
            Event::Custom { text, .. } if zirc_proto::names::is_integration_nick(account) => {
                let integration = |account: &str, _: Option<&str>| Some(Actor::person(account));
                let mut outs = self.mention(channel, account, None, text, &integration, now);
                outs.extend(self.said(channel, account, text, &row.msgid, now));
                outs
            }
            _ => Vec::new(),
        }
    }

    /// What `seen` says someone did, said `live`: what moves their tasks on; read back, only
    /// that they weren't quiet at `at`.
    fn activity(&mut self, seen: &Seen, live: bool, at: u64, now: u64) -> Vec<Out> {
        let mut outs = Vec::new();
        for running in self.live.values_mut() {
            if live {
                let input = Input::Activity {
                    by: seen.by.clone(),
                    did: seen.did,
                    detail: seen.detail.clone(),
                    branch: seen.branch.clone(),
                    text: seen.text.clone(),
                };
                let said = running.engine.handle(input, now);
                outs.extend(posts(running.engine.workflow(), said));
            } else {
                running.engine.restore_activity(&seen.by, at);
            }
        }
        outs
    }

    fn deploy(&mut self, deploy: &Deploy, account: &str, live: bool, who: Who<'_>) -> Vec<Out> {
        // zirc.dev's dashboard says a deploy for the person signed in there, as zirc's own
        // account, which nobody else posts as: theirs, then. From anyone else, `by` is ignored.
        let by = if account == zirc_proto::names::SYSTEM {
            let Some(by) = &deploy.by else {
                return Vec::new();
            };
            by.to_lowercase()
        } else if who(account, None).is_some() {
            self.person(account)
        } else {
            return Vec::new();
        };
        let checked = zirc_workflows::parse(&deploy.source).and_then(|workflow| {
            if workflow.name != deploy.name || workflow.nick != deploy.nick {
                return Err("its name or nick isn't what the deploy says".to_owned());
            }
            if let Some(why) = self.kept_from(&workflow.name, &by).filter(|_| live) {
                return Err(why);
            }
            match self.named(&workflow.nick) {
                Some(other) if other != workflow.name => {
                    Err(format!("@{} runs `{other}` already", workflow.nick))
                }
                _ => Ok(workflow),
            }
        });
        let version = zirc_workflows::version(&deploy.source);
        match (checked, live) {
            (Ok(workflow), true) => vec![Out::Check(Candidate {
                workflow,
                version,
                source: deploy.source.clone(),
                by,
            })],
            (Ok(workflow), false) => {
                // Not installed yet: only once the host's word that it was is read back.
                let key = (workflow.name.clone(), version.clone());
                let candidate = Candidate {
                    workflow,
                    version,
                    source: deploy.source.clone(),
                    by,
                };
                self.read_back.insert(key, candidate);
                Vec::new()
            }
            (Err(why), true) => {
                self.refused.insert(
                    deploy.name.clone(),
                    Refused {
                        name: deploy.name.clone(),
                        version,
                        why: why.clone(),
                    },
                );
                vec![host(format!(
                    "{by}: couldn't deploy `{}`: {why}",
                    deploy.name
                ))]
            }
            (Err(_), false) => Vec::new(),
        }
    }

    /// Runs `candidate`, in place of the workflow of its name, carrying on its runs: what to
    /// keep connected.
    pub fn install(&mut self, candidate: Candidate) -> Option<Out> {
        let Candidate {
            workflow,
            version,
            source,
            by,
        } = candidate;
        self.refused.remove(&workflow.name);
        // Whoever deployed it first keeps it, whoever replaces it since.
        self.deployers.entry(workflow.name.clone()).or_insert(by);
        let mut engine = Engine::new(workflow.clone(), version.clone());
        // Deployed again, it knows what the one before it did of the times that came round.
        if let Some(before) = self.live.get(&workflow.name) {
            engine.restore_fired(&before.engine.fired());
            if before.engine.is_caught_up() {
                engine.caught_up();
            }
        }
        for state in self.states.values() {
            engine.restore_state(state);
        }
        for kept in self.logs.get(&workflow.name).into_iter().flatten() {
            engine.restore_change(&kept.run, &kept.change, kept.at);
        }
        let out = Out::Bot {
            nick: workflow.nick.to_lowercase(),
            channels: workflow.channels(),
        };
        self.live.insert(
            workflow.name,
            Live {
                engine,
                source,
                version,
            },
        );
        *NICKS
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self
            .live
            .values()
            .map(|l| l.engine.workflow().nick.to_lowercase())
            .collect();
        Some(out)
    }

    /// What `zirc-workflows` says when it couldn't tell whose `candidate`'s nick is.
    #[must_use]
    pub fn unchecked(candidate: &Candidate) -> Out {
        host(format!(
            "couldn't deploy `{}`: couldn't check @{}'s account just now; deploy it again",
            candidate.workflow.name, candidate.workflow.nick
        ))
    }

    /// What `zirc-workflows` says of `candidate` once it's installed.
    #[must_use]
    pub fn deployed(candidate: &Candidate) -> Out {
        host(format!(
            "deployed `{}` ({}), run by @{} in {}",
            candidate.workflow.name,
            candidate.version,
            candidate.workflow.nick,
            candidate.workflow.channel
        ))
    }

    /// What `zirc-workflows` says when `candidate`'s nick is someone else's.
    #[must_use]
    pub fn refused(candidate: &Candidate) -> Out {
        host(format!(
            "couldn't deploy `{}`: @{} is someone else's account; name another nick",
            candidate.workflow.name, candidate.workflow.nick
        ))
    }

    fn ran(&mut self, name: &str, event: RunEvent, msgid: &str, live: bool) {
        let Some(running) = self.live.get_mut(name) else {
            return;
        };
        if !live {
            running.engine.restore(&event);
        }
        let RunEvent {
            run, at, change, ..
        } = event;
        let log = self.logs.entry(name.to_owned()).or_default();
        if matches!(change, Change::Finished { .. }) {
            // A part of a step given to several (`master-3.1`) is kept whole while the run
            // it's part of goes on, which reads how each part ended; and goes with it.
            let parts = format!("{run}.");
            if !run.contains('.') {
                log.retain(|kept| kept.run != run && !kept.run.starts_with(&parts));
            }
            self.messages.retain(|_, (_, of)| *of != run);
        } else {
            self.messages
                .insert(msgid.to_owned(), (name.to_owned(), run.clone()));
        }
        if let Change::Assigned { to, .. } = &change
            && let [member] = to.as_slice()
        {
            self.given.insert(member.clone(), at);
        }
        log.push(Kept { run, at, change });
    }

    /// What a nick heard live in the organisation's channel: a machine's live sessions and
    /// their roles, or a machine leaving, whose sessions then aren't.
    pub(crate) fn heard(&mut self, line: &zirc_irc::Line) {
        let Some(nick) = line
            .source
            .as_deref()
            .and_then(|source| source.split('!').next())
            .map(str::to_lowercase)
        else {
            return;
        };
        match line.command.as_str() {
            // A nick's own join, as when its connection came back: every machine says its list
            // again to whoever joins, so what was heard before, those gone since among it, goes.
            "JOIN"
                if self.runs_as(&nick)
                    && line
                        .params
                        .first()
                        .is_some_and(|c| c.eq_ignore_ascii_case(zirc_proto::ORG_CHANNEL)) =>
            {
                self.machines.clear();
            }
            "QUIT" | "PART" | "KICK" => {
                self.machines.remove(&nick);
            }
            "TAGMSG" | "PRIVMSG" => {
                let Some(said) = line
                    .tag(zirc_proto::EVENT_TAG)
                    .and_then(|payload| zirc_proto::from_payload(payload).ok())
                    .and_then(|message| zirc_proto::session::Live::of(&message.event))
                else {
                    return;
                };
                self.machines.insert(nick, said);
            }
            _ => {}
        }
    }

    /// Each role's live members now, how busy each is across the workflows, and when each
    /// was last given a step.
    fn members(&self) -> BTreeMap<String, Vec<zirc_workflows::Member>> {
        let mut busy: BTreeMap<String, usize> = BTreeMap::new();
        for running in self.live.values() {
            for holder in running.engine.holders() {
                *busy.entry(holder).or_default() += 1;
            }
        }
        let mut members: BTreeMap<String, Vec<zirc_workflows::Member>> = BTreeMap::new();
        for (machine, live) in &self.machines {
            for (session, roles) in &live.roles {
                if !live.sessions.contains(session) {
                    continue;
                }
                let name = format!("{machine}/{session}");
                for role in roles {
                    members
                        .entry(role.clone())
                        .or_default()
                        .push(zirc_workflows::Member {
                            name: name.clone(),
                            busy: busy.get(&name).copied().unwrap_or_default(),
                            last: self.given.get(&name).copied().unwrap_or_default(),
                        });
                }
            }
        }
        members
    }

    /// Whether `nick` is one of the workflows' running.
    #[must_use]
    pub fn runs_as(&self, nick: &str) -> bool {
        self.named(nick).is_some()
    }

    fn state(&mut self, state: &State, live: bool, now: u64) -> Vec<Out> {
        self.states.insert(state.name.clone(), state.clone());
        let mut outs = Vec::new();
        for running in self.live.values_mut() {
            if live {
                let said = running.engine.handle(Input::State(state.clone()), now);
                outs.extend(posts(running.engine.workflow(), said));
            } else {
                running.engine.restore_state(state);
            }
        }
        outs
    }

    fn act(
        &mut self,
        channel: &str,
        account: &str,
        session: &str,
        action: StepAction,
        who: Who<'_>,
        now: u64,
    ) -> Vec<Out> {
        let Some(by) = who(account, Some(session)) else {
            return Vec::new();
        };
        let Some(running) = self.live.values_mut().find(|l| {
            let w = l.engine.workflow();
            w.channel.eq_ignore_ascii_case(channel)
                && action
                    .run
                    .strip_prefix(&w.name)
                    .is_some_and(|n| n.starts_with('-'))
        }) else {
            return Vec::new();
        };
        let said = running.engine.handle(Input::Action { by, action }, now);
        posts(running.engine.workflow(), said)
    }

    fn mention(
        &mut self,
        channel: &str,
        account: &str,
        session: Option<&str>,
        text: &str,
        who: Who<'_>,
        now: u64,
    ) -> Vec<Out> {
        let mut outs = Vec::new();
        for running in self.live.values_mut() {
            let w = running.engine.workflow();
            if !w.channel.eq_ignore_ascii_case(channel) {
                continue;
            }
            let Some(rest) = addressed(text, &w.nick) else {
                continue;
            };
            let Some(by) = who(account, session) else {
                continue;
            };
            let said = running.engine.handle(
                Input::Mention {
                    by,
                    text: rest.to_owned(),
                },
                now,
            );
            outs.extend(posts(running.engine.workflow(), said));
        }
        outs
    }

    fn reaction(&mut self, row: &Row, account: &str, who: Who<'_>, now: u64) -> Vec<Out> {
        let tag = |name: &str| row.item.tags.get(name).cloned();
        let (Some(reaction), Some(id)) = (tag("+draft/react"), tag("+draft/reply")) else {
            return Vec::new();
        };
        let Some((name, run)) = self.messages.get(&id).cloned() else {
            // On anything but a run's own messages: what a `reaction` trigger starts a run on.
            return self.reacted(row, &reaction, &id, account, who, now);
        };
        let yes = match reaction.as_str() {
            "👍" => true,
            "👎" => false,
            _ => return Vec::new(),
        };
        let Some(running) = self.live.get_mut(&name) else {
            return Vec::new();
        };
        let (Some(step), Some(by)) = (running.engine.approval(&run), who(account, None)) else {
            return Vec::new();
        };
        let action = StepAction {
            run,
            step,
            act: Act::Answer { yes },
        };
        let said = running.engine.handle(Input::Action { by, action }, now);
        posts(running.engine.workflow(), said)
    }

    /// When the time passing next moves a workflow's run on, after `now`
    /// ([`Engine::next_due`]): `None` while nothing waits on the time.
    #[must_use]
    pub fn next_due(&self, now: u64) -> Option<u64> {
        self.live
            .values()
            .filter_map(|running| running.engine.next_due(now))
            .min()
    }

    /// What the time passing does, at `now`.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        let mut outs = Vec::new();
        for name in self.live.keys().cloned().collect::<Vec<_>>() {
            // Who's busy changes as each workflow gives out steps: asked again for each.
            let members = self.members();
            let Some(running) = self.live.get_mut(&name) else {
                continue;
            };
            running.engine.set_members(members);
            let said = running.engine.handle(Input::Tick, now);
            outs.extend(posts(running.engine.workflow(), said));
        }
        outs
    }
}

/// What `message` (none for a plain IRC line) from `account` shows of someone working, that
/// a workflow follows a task by: what they say, their intents and pushes, and what CI says of
/// their landings, naming their session.
fn seen(message: Option<&Message>, account: &str, who: Who<'_>) -> Option<Seen> {
    let Some(message) = message else {
        return who(account, None).map(|by| Seen::of(by, Did::Said));
    };
    let mine = || who(account, Some(&message.session));
    match &message.event {
        // Their words, so a session given several runs moves the one it names.
        Event::Text { body } => mine().map(|by| Seen {
            text: Some(body.clone()),
            ..Seen::of(by, Did::Said)
        }),
        Event::Intent { summary, .. } => mine().map(|by| Seen {
            text: Some(summary.clone()),
            ..Seen::of(by, Did::Intent)
        }),
        Event::Push { branch, .. } => mine().map(|by| Seen {
            detail: Some(branch.clone()),
            branch: Some(branch.clone()),
            ..Seen::of(by, Did::Push)
        }),
        Event::Custom { kind, data, .. }
            if kind == zirc_proto::ci::LANDING_KIND && account == zirc_proto::ci::NICK =>
        {
            let data = zirc_proto::ci::Data::of(data);
            let did = match data.state.as_deref()? {
                "pending" | "running" => Did::LandingQueued,
                "passed" => Did::Landed,
                "failed" | "cancelled" => Did::LandingFailed,
                _ => return None,
            };
            Some(Seen {
                detail: data.build.map(|build| format!("build {build}")),
                branch: data.branch,
                ..Seen::of(Actor::named_session(&data.session?), did)
            })
        }
        _ => None,
    }
}

/// Something someone did that a workflow follows a task by.
struct Seen {
    by: Actor,
    did: Did,
    detail: Option<String>,
    branch: Option<String>,
    text: Option<String>,
}

impl Seen {
    fn of(by: Actor, did: Did) -> Self {
        Self {
            by,
            did,
            detail: None,
            branch: None,
            text: None,
        }
    }
}

/// What follows `nick` in `text`, if `text` is to it: `@nick …`, `nick: …` or `nick, …`.
fn addressed<'a>(text: &'a str, nick: &str) -> Option<&'a str> {
    let text = text.trim_start();
    let rest = text.strip_prefix('@').unwrap_or(text);
    if !rest.get(..nick.len())?.eq_ignore_ascii_case(nick) {
        return None;
    }
    let after = &rest[nick.len()..];
    let after = after
        .strip_prefix(':')
        .or_else(|| after.strip_prefix(','))
        .unwrap_or(after);
    (after.is_empty() || after.starts_with(' ')).then(|| after.trim())
}

/// What `workflow`'s nick says, as posts in its channel.
fn posts(workflow: &Workflow, said: Vec<Said>) -> Vec<Out> {
    said.into_iter()
        .map(|said| {
            let event = match said {
                Said::Run(event) => event,
                Said::Reply(body) => Event::Text { body },
            };
            Out::Post {
                nick: workflow.nick.clone(),
                channel: workflow.channel.to_lowercase(),
                message: Message::new(workflow.nick.clone(), event),
            }
        })
        .collect()
}

/// The name and version in the host's word that it deployed a workflow
/// ([`Hosted::deployed`], whose words these are).
fn deployed_in(body: &str) -> Option<(String, String)> {
    let (name, rest) = body.strip_prefix("deployed `")?.split_once("` (")?;
    let (version, _) = rest.split_once("), run by @")?;
    Some((name.to_owned(), version.to_owned()))
}

/// What `zirc-workflows` says in `#workflows`.
fn host(body: String) -> Out {
    Out::Post {
        nick: workflow::HOST.to_owned(),
        channel: workflow::CHANNEL.to_owned(),
        message: Message::new(workflow::HOST, Event::Text { body }),
    }
}

/// Who `account` is, for a workflow, as `machines` says: a person's own client is that
/// person; a machine is its session; guests and zirc's own may not act.
#[must_use]
pub fn actor(machines: &Machines, account: &str, session: Option<&str>) -> Option<Actor> {
    if account.starts_with("zirc") || zirc_proto::names::is_guest_account(account) {
        return None;
    }
    if machines.clients().contains(account) {
        return machines.owner(account).map(|owner| Actor::person(&owner));
    }
    if machines.owner(account).is_some() {
        return Some(Actor {
            account: account.to_owned(),
            session: session.map(str::to_owned),
            person: false,
        });
    }
    Some(Actor::person(account))
}

/// What the host needs to connect its workflows' nicks: Ergo's loopback address, its gate
/// password, the endpoint's token their passphrases are derived from, where to keep where
/// they were invited, the server's machines, and whether it runs workflows at all.
pub struct Host {
    /// Ergo's loopback address.
    pub host: String,
    /// Its IRC port there.
    pub port: u16,
    /// The server's gate password, if it has one.
    pub gate: Option<String>,
    /// The endpoint's token.
    pub token: String,
    /// Ergo's history, beside which where each workflow's nick was invited and what it has
    /// read are kept ([`KEPT`]).
    pub history: PathBuf,
    /// The server's machines, to tell a person from a machine's session.
    pub machines: Machines,
    /// What the controller says, which turns workflows on and off.
    pub gateway: std::sync::Arc<crate::gateway::Gateway>,
    /// The organisation's owners and admins now, who may replace anyone's workflow.
    pub admins: Admins,
}

/// Asks who the organisation's owners and admins are ([`Host::admins`]).
pub type Admins = Box<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = BTreeSet<String>> + Send>>
        + Send
        + Sync,
>;

/// What the host keeps beside Ergo's history ([`Memory`]).
pub const KEPT: &str = "workflows.json";

/// What the host keeps of its nicks, by nick: where each was invited, and how far it has read
/// in each channel. Nothing anyone said.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Memory {
    #[serde(default)]
    invited: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    read: BTreeMap<String, BTreeMap<String, connection::Mark>>,
}

/// How old a message a nick hadn't seen may be and still be acted on when it catches up:
/// older, it only tells where things stand, so a long time away doesn't start stale runs.
pub const MISSED: Duration = Duration::from_hours(1);

/// How many message ids the host remembers, to count one heard by two of its nicks once.
const REMEMBERED: usize = 10_000;

/// Each nick's connection: the channels it's in, and what it's to do.
type Links = HashMap<String, (BTreeSet<String>, mpsc::UnboundedSender<connection::Do>)>;

impl Host {
    /// Runs the server's workflows while the controller says it runs them
    /// ([`crate::gateway::Public::workflows`]), until the process ends. Off, nothing connects,
    /// so an organisation zirc.dev hasn't turned them on for never sees them. `account` makes
    /// sure a nick has an account, made with the derived passphrase if it has none, never
    /// changing one that exists.
    pub async fn run<F, Fut>(self, account: F)
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let mut on = self.gateway.runs_workflows();
        loop {
            if on.wait_for(|on| *on).await.is_err() {
                return;
            }
            info!("workflows are on");
            self.serve(&account, &mut on).await;
            list(Vec::new(), Vec::new());
            info!("workflows were turned off");
        }
    }

    /// Runs the workflows, caught up afresh, until they're turned off: every nick's
    /// connection ends with it. Nothing happens but on what's heard, and when an engine's next
    /// time comes ([`Hosted::next_due`]).
    async fn serve<F, Fut>(&self, account: &F, on: &mut tokio::sync::watch::Receiver<bool>)
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        while !account(workflow::HOST.to_owned()).await {
            tokio::select! {
                () = tokio::time::sleep(RETRY) => {}
                _ = on.wait_for(|on| !*on) => return,
            }
        }
        let mut hosted = Hosted::default();
        let mut links = Links::new();
        let mut ids = Ids::default();
        let mut kept = self.kept();
        let (hear, mut heard) = mpsc::unbounded_channel();
        // In #workflows alone, for what's deployed and to say how that went.
        self.link(
            &mut links,
            workflow::HOST,
            [workflow::CHANNEL.to_owned()].into(),
            &kept,
            &hear,
        );
        let mut due: Option<u64> = None;
        loop {
            let wait = due.map(|at| Duration::from_secs(at.saturating_sub(connection::now())));
            let mut outs = Vec::new();
            let mut got = None;
            tokio::select! {
                _ = on.wait_for(|on| !*on) => return,
                Some(heard) = heard.recv() => got = Some(heard),
                () = async { tokio::time::sleep(wait.unwrap_or_default()).await }, if wait.is_some() => {}
            }
            if let Some(heard) = got {
                // Who may replace a workflow, as things stand now.
                hosted.know(
                    (self.admins)().await,
                    self.machines.owners().unwrap_or_default(),
                );
                outs = self.hear(heard, &mut hosted, &mut ids, &mut kept, &links);
            }
            let now = connection::now();
            outs.extend(hosted.tick(now));
            self.carry_out(outs, &mut hosted, &mut links, &kept, &hear, account)
                .await;
            due = hosted.next_due(now);
            list(hosted.listed(), hosted.refusals());
        }
    }

    /// What `heard` does: what to carry out.
    fn hear(
        &self,
        heard: connection::Heard,
        hosted: &mut Hosted,
        ids: &mut Ids,
        kept: &mut Memory,
        links: &Links,
    ) -> Vec<Out> {
        let now = connection::now();
        let who = |account: &str, session: Option<&str>| actor(&self.machines, account, session);
        let mut outs = Vec::new();
        match heard {
            connection::Heard::Caught {
                nick,
                channel,
                seen,
                unseen,
            } => {
                info!(
                    nick,
                    channel,
                    seen = seen.len(),
                    unseen = unseen.len(),
                    "caught up"
                );
                let seen = seen.into_iter().map(|row| (false, row));
                for (new, row) in seen.chain(unseen.into_iter().map(|row| (true, row))) {
                    if !ids.insert(&row.msgid) {
                        continue;
                    }
                    // A workflow's own is only where it stands; the rest is acted on if it
                    // hadn't been seen, and isn't from too long ago.
                    let live = new
                        && now.saturating_sub(row.at) <= MISSED.as_secs()
                        && !hosted.runs_as(&row.item.account);
                    outs.extend(hosted.take(&row, live, &who, now));
                }
                hosted.caught(&nick, &channel);
            }
            connection::Heard::Read {
                nick,
                channel,
                mark,
            } => {
                kept.read.entry(nick).or_default().insert(channel, mark);
                self.keep(kept);
            }
            connection::Heard::Live { nick, line } => {
                hosted.heard(&line);
                let param = |i: usize| line.params.get(i).map(|p| p.to_lowercase());
                match line.command.as_str() {
                    // Anyone in the organisation may invite a workflow's nick anywhere.
                    "INVITE" if nick != workflow::HOST => {
                        let by = line.tag("account").unwrap_or_default();
                        if let Some(channel) = param(1)
                            && who(by, None).is_some()
                            && let Some((_, link)) = links.get(&nick)
                        {
                            info!(nick, channel, "invited");
                            let _ = link.send(connection::Do::Join(channel.clone()));
                            kept.invited.entry(nick).or_default().insert(channel);
                            self.keep(kept);
                        }
                    }
                    // Kicked from a channel it was invited to: it stays away.
                    "KICK" if param(1).as_deref() == Some(&nick) => {
                        if let (Some(channel), Some(channels)) =
                            (param(0), kept.invited.get_mut(&nick))
                            && channels.remove(&channel)
                        {
                            self.keep(kept);
                        }
                    }
                    _ => {
                        // Heard live, a workflow's own post too: the engine said it, and
                        // reading it back as history would undo what it did since (build
                        // 3306's flaky runs each gave their step twice).
                        if let Some(row) = connection::row_of(&line)
                            && ids.insert(&row.msgid)
                        {
                            outs.extend(hosted.take(&row, true, &who, now));
                        }
                    }
                }
            }
        }
        outs
    }

    /// What's kept of the nicks, beside the history.
    fn kept(&self) -> Memory {
        std::fs::read(self.history.with_file_name(KEPT))
            .ok()
            .and_then(|kept| serde_json::from_slice(&kept).ok())
            .unwrap_or_default()
    }

    fn keep(&self, kept: &Memory) {
        let path = self.history.with_file_name(KEPT);
        let bytes = serde_json::to_vec(kept).unwrap_or_default();
        if let Err(error) = std::fs::write(&path, bytes) {
            warn!(%error, "couldn't keep what workflows' nicks have read");
        }
    }

    /// Does what the workflows said, in order: posts as their nicks, keeps those running
    /// connected in their channels and lets the rest go, and installs what was deployed once
    /// its nick is surely its own.
    async fn carry_out<F, Fut>(
        &self,
        mut outs: Vec<Out>,
        hosted: &mut Hosted,
        links: &mut Links,
        kept: &Memory,
        hear: &mpsc::UnboundedSender<connection::Heard>,
        account: &F,
    ) where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let mut changed = false;
        while let Some(out) = (!outs.is_empty()).then(|| outs.remove(0)) {
            match out {
                Out::Bot { nick, channels } => {
                    let mut channels: BTreeSet<String> = channels.into_iter().collect();
                    channels.extend(kept.invited.get(&nick).into_iter().flatten().cloned());
                    match links.get_mut(&nick) {
                        Some((joined, link)) => {
                            for channel in channels.difference(joined) {
                                let _ = link.send(connection::Do::Join(channel.clone()));
                            }
                            joined.extend(channels);
                        }
                        None => self.link(links, &nick, channels, kept, hear),
                    }
                    changed = true;
                }
                Out::Post {
                    nick,
                    channel,
                    message,
                } => {
                    if let Some((_, link)) = links.get(&nick) {
                        let _ = link.send(connection::Do::Say(channel, Box::new(message)));
                    }
                }
                Out::Check(candidate) => {
                    let nick = candidate.workflow.nick.clone();
                    // A nick a workflow runs as already is ours: deployed again, it's only
                    // replaced.
                    let login = if hosted.runs_as(&nick) {
                        Login::Ours
                    } else if account(nick.clone()).await {
                        self.login(&nick).await
                    } else {
                        Login::Unknown
                    };
                    match login {
                        Login::Ours => {
                            info!(
                                workflow = candidate.workflow.name,
                                "a workflow was deployed"
                            );
                            outs.push(Hosted::deployed(&candidate));
                            outs.extend(hosted.install(candidate));
                        }
                        Login::Theirs => {
                            let said = Hosted::refused(&candidate);
                            hosted.refuse(&candidate, &said);
                            outs.push(said);
                        }
                        Login::Unknown => {
                            let said = Hosted::unchecked(&candidate);
                            hosted.refuse(&candidate, &said);
                            outs.push(said);
                        }
                    }
                }
            }
        }
        // A nick no workflow runs as any more, as when one was deployed again with another,
        // is let go.
        if changed {
            links.retain(|nick, _| nick == workflow::HOST || hosted.runs_as(nick));
        }
    }

    fn config(&self, nick: &str) -> zirc_irc::Config {
        crate::say::config(
            &self.host,
            self.port,
            self.gate.as_deref(),
            &self.token,
            nick,
        )
    }

    /// Whose `nick`'s account is, as its login with the derived passphrase says.
    async fn login(&self, nick: &str) -> Login {
        match tokio::time::timeout(WITHIN, zirc_irc::connect(&self.config(nick))).await {
            Ok(Ok((client, _incoming))) => {
                let _ = client.quit("checked");
                Login::Ours
            }
            Ok(Err(zirc_irc::Error::Sasl(_))) => Login::Theirs,
            _ => Login::Unknown,
        }
    }

    /// Connects `nick` in `channels`, from where it read to, passing on what it hears to
    /// `hear`.
    fn link(
        &self,
        links: &mut Links,
        nick: &str,
        channels: BTreeSet<String>,
        kept: &Memory,
        hear: &mpsc::UnboundedSender<connection::Heard>,
    ) {
        let (send, todo) = mpsc::unbounded_channel();
        tokio::spawn(connection::run(
            self.config(nick),
            channels.iter().cloned().collect(),
            kept.read
                .get(&nick.to_lowercase())
                .cloned()
                .unwrap_or_default(),
            todo,
            hear.clone(),
        ));
        links.insert(nick.to_lowercase(), (channels, send));
    }
}

/// The message ids heard lately, so one heard by two nicks, or both caught up on and heard
/// live, counts once.
#[derive(Default)]
struct Ids {
    set: std::collections::HashSet<String>,
    order: std::collections::VecDeque<String>,
}

impl Ids {
    /// Whether `id` is new.
    fn insert(&mut self, id: &str) -> bool {
        if !self.set.insert(id.to_owned()) {
            return false;
        }
        self.order.push_back(id.to_owned());
        if self.order.len() > REMEMBERED
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        true
    }
}

/// Whose a nick's account is.
enum Login {
    /// A workflow's: it logs in with the derived passphrase.
    Ours,
    /// Someone else's: the server refused the derived passphrase.
    Theirs,
    /// Couldn't tell: the server didn't answer, or the account couldn't be made.
    Unknown,
}

mod connection;

#[cfg(test)]
mod tests;
