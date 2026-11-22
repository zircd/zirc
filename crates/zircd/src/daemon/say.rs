//! What this machine says: intents, decisions, state, posts, DMs, reactions, typing and
//! the messages it publishes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use anyhow::Result;
use zirc_core::decisions;
use zirc_core::ipc::{Delivery, Direct, Reaction, Response, Target, Why};
use zirc_core::project::ORG_CHANNEL;
use zirc_irc::{Client, Line};
use zirc_proto::decision::Decision;
use zirc_proto::telemetry as usage;
use zirc_proto::telemetry::Event as Usage;
use zirc_proto::{AGENTS_CHANNEL, Event, Message};

use crate::git;

use super::book::Field;
use super::{
    Daemon, GUESTS, Seen, TYPING_EVERY, Typing, Which, Wrote, is_guest, is_nick, now,
    only_cis_word, reaction, remember, vouch,
};

impl Daemon {
    /// Tells `#agents` what `session` is about to change in the repository at `repo`.
    pub(super) async fn intend(
        &mut self,
        repo: &Path,
        session: &str,
        summary: String,
        paths: &[PathBuf],
        seconds: u64,
    ) -> Result<Response> {
        let channel = self.watched(repo).await?.scope.clone();
        let root = git::root(repo).await?;
        let base = repo.canonicalize().unwrap_or_else(|_| repo.to_owned());
        let paths = paths
            .iter()
            .map(|path| git::relative(&root, &base, path))
            .collect::<Result<Vec<String>>>()?;
        self.connected()?;
        self.auto_intents
            .declared(session, &channel, &paths, now() + seconds);
        let event = Event::Intent {
            summary,
            paths,
            expires: Some(now() + seconds),
            lasts: Some(seconds),
        };
        self.publish(&channel, session, event);
        self.count(&Usage::IntentPosted(usage::IntentPosted {}));
        Ok(Response::channel(self.wire_name(&channel)))
    }

    /// The default branch of the repositories in the scope `channel`.
    pub(super) fn default_branch(&self, channel: &str) -> Option<String> {
        self.repos
            .values()
            .find(|watched| watched.scope == channel)
            .map(|watched| watched.default_branch.clone())
    }

    /// Posts this machine's owner's decision about `about` in their words, `quote`, to the
    /// organisation's `#ops`, from inside the watched repository `repo`: relayed by
    /// `session`, or, without one, from a person at a terminal here. Returns the channels.
    pub(super) async fn decide(&mut self, repo: &Path, decision: Decision) -> Result<Vec<String>> {
        let by = self
            .owner
            .clone()
            .context("this machine has no owner to decide anything: zirc login sets one")?;
        anyhow::ensure!(
            !decision.quote.trim().is_empty(),
            "say what they said: --quote"
        );
        anyhow::ensure!(
            !decision.about.trim().is_empty(),
            "say what it's about: --about"
        );
        // Inside a watched repository, as the sessions it's for work in one.
        self.watched(repo).await?;
        let ops = decisions::CHANNEL.to_owned();
        let sender = decision.session.clone().unwrap_or_else(|| by.clone());
        let event = Decision { by, ..decision }.event();
        let client = self.connected()?;
        if !self.joined.contains(&ops) {
            client.send(Line::new("JOIN", [ops.as_str()]))?;
        }
        // Once, in #ops: every machine watching a repository is in it, so the sessions it's
        // for hear it there, and a copy in the repository's channel was a second line for
        // every reader. A decision that couldn't be sent is said so, never as decided.
        let message = self.signed(&sender, event);
        self.publish_out(&ops, message, None, None)
            .context("the decision wasn't sent")?;
        Ok(vec![ops])
    }

    /// Says how something in the repository containing `repo` stands
    /// ([`zirc_proto::state`]), in `#ops` as decisions are, where every machine watching a
    /// repository is: the repository is the one it was set in, never the caller's word.
    pub(super) async fn set_state(
        &mut self,
        repo: &Path,
        session: &str,
        state: zirc_proto::state::State,
    ) -> Result<String> {
        if let Some(problem) = zirc_proto::state::check_name(&state.name) {
            anyhow::bail!("{}: {problem}", state.name);
        }
        let watched = self.watched(repo).await?;
        let state = zirc_proto::state::State {
            repo: watched.scope.clone(),
            ..state
        };
        let ops = zirc_core::states::CHANNEL.to_owned();
        let client = self.connected()?;
        if !self.joined.contains(&ops) {
            client.send(Line::new("JOIN", [ops.as_str()]))?;
        }
        self.holds.said(session, &state);
        self.publish(&ops, session, state.event());
        Ok(ops)
    }

    /// Sends `event` for the organisation's workflows to `channel` as `session`, joining it
    /// first ([`zirc_core::ipc::Request::Workflow`]): a workflow's file, to `#workflows`
    /// alone, what a worker did with a step, or words to a workflow's nick. A file is sent by
    /// [`Daemon::deploy`] instead, once the server's workflows are surely on.
    pub(super) async fn workflow_send(
        &mut self,
        repo: &Path,
        session: &str,
        channel: &str,
        event: Event,
    ) -> Result<String> {
        self.workflow_checked(repo, channel, &event).await?;
        self.workflow_emit(session, channel, event)
    }

    /// Whether `event` may go to `channel` as one of the organisation's workflows' events.
    pub(super) async fn workflow_checked(
        &mut self,
        repo: &Path,
        channel: &str,
        event: &Event,
    ) -> Result<()> {
        use zirc_proto::workflow::{CHANNEL, Deploy};
        let deploy = Deploy::of(event).is_some();
        let fits = matches!(event, Event::Text { .. })
            || deploy
            || zirc_proto::run::StepAction::of(event).is_some();
        anyhow::ensure!(
            fits,
            "only a workflow's file, a step's action or words go to a workflow"
        );
        anyhow::ensure!(
            !deploy || channel == CHANNEL,
            "a workflow's file goes to {CHANNEL}"
        );
        anyhow::ensure!(
            channel.len() > 1 && channel.starts_with('#') && !channel.contains([' ', ',']),
            "{channel} isn't a channel"
        );
        self.watched(repo).await?;
        self.connected()?;
        Ok(())
    }

    /// Sends `event`, checked, to `channel` as `session`, joining it first.
    pub(super) fn workflow_emit(
        &mut self,
        session: &str,
        channel: &str,
        event: Event,
    ) -> Result<String> {
        let client = self.connected()?;
        if !self.joined.contains(channel) {
            client.send(Line::new("JOIN", [channel]))?;
        }
        let message = self.signed(session, event);
        self.emit(channel, message)?;
        Ok(channel.to_owned())
    }

    /// Sends a team's own kind of event to `channel`, whole or not at all: unlike a post, it
    /// can't be split or shortened.
    pub(super) fn emit(&mut self, channel: &str, message: Message) -> Result<()> {
        let longest = self
            .client
            .as_ref()
            .and_then(Client::multiline)
            .map_or(zirc_proto::MAX_BODY_BYTES, |limits| limits.max_bytes);
        if let Err(zirc_proto::Error::TooLong(len)) = zirc_proto::encode_up_to(&message, longest) {
            anyhow::bail!(
                "the event is {len} bytes encoded, and the server takes {longest}: carry less \
                 data or say less"
            );
        }
        self.publish_message(channel, message);
        Ok(())
    }

    /// Where `to` goes from the repository containing `repo`, once it's joined: its scope,
    /// which goes out in `#agents` ([`Daemon::on_wire`]), or an organisation's channel.
    pub(super) async fn channel_to(&mut self, repo: &Path, to: Target) -> Result<String> {
        let watched = self.watched(repo).await?;
        let channel = match to {
            Target::Agents => watched.scope.clone(),
            Target::Org => ORG_CHANNEL.to_owned(),
            Target::Topic { name } => zirc_core::project::topic_channel(&name),
        };
        self.connected()?;
        anyhow::ensure!(
            self.joined.contains(&channel),
            "{} isn't joined: join a topic first with `zirc join`",
            if self.is_scope(&channel) {
                AGENTS_CHANNEL
            } else {
                &channel
            }
        );
        Ok(channel)
    }

    /// Says `text` to `to` alone, or else to this machine's owner, from `session`. Returns
    /// whom it went to.
    pub(super) fn dm(&mut self, session: &str, text: &str, to: Option<String>) -> Result<String> {
        let to = match to {
            Some(to) => {
                anyhow::ensure!(is_nick(&to), "{to:?} isn't a person's nick or account");
                anyhow::ensure!(!is_guest(&to), "{GUESTS}");
                anyhow::ensure!(
                    !self.is_machine(&to),
                    "{to} is a machine: its agents hear only its owner, so this wouldn't reach \
                     anyone. Message its owner, ask its agents with `zirc ask`, or post naming \
                     its session"
                );
                to
            }
            None => self
                .owner
                .clone()
                .context("this machine has no owner to message: zirc login sets one")?,
        };
        let client = self.connected()?;
        zirc_core::send::send_message(
            client,
            &to,
            &self.signed(
                session,
                Event::Text {
                    body: text.to_owned(),
                },
            ),
        )
        .with_context(|| format!("couldn't send to {to}"))?;
        // Their reply reaches this session, while the organisation lets colleagues answer.
        if self.owner.as_deref() != Some(to.as_str()) {
            remember(
                &mut self.wrote_to,
                Wrote {
                    to: to.clone(),
                    session: session.to_owned(),
                    at: now(),
                },
            );
        }
        Ok(to)
    }

    /// Remembers this machine's own message to `channel`, as the server echoes it with its
    /// id, so a reaction to it can be told as its session's. Only this machine's own echo
    /// says whose a message is: never a nick or a tag on someone else's line.
    pub(super) fn echoed(&mut self, channel: &str, from: &str, body: &str, line: &Line) {
        if let Some(id) = line.tag("msgid") {
            let message = zirc_proto::decode(body).ok().flatten();
            let session = message.as_ref().map(|message| message.session.as_str());
            let about = message.as_ref().map(|message| {
                zirc_core::ipc::Reacted::of(&Delivery::new(channel, from, message.clone()))
            });
            self.recent.heard(id, channel, from, session, about);
            if let Some(sealed) = line.tag(zirc_proto::sealed::TAG) {
                self.recent.sealed(id, sealed);
            }
            if let Some(message) = message {
                if matches!(message.event, Event::Text { .. }) {
                    self.post_echoed(channel, id, &message.session);
                }
                if Decision::of(&message.event).is_some() {
                    self.decision_echoed(channel, &message.session);
                }
                self.request_echoed(channel, id, &message.event);
                if let Some(to) = line.tag("+draft/reply") {
                    self.request_reply_echoed(channel, id, to, &message.session);
                }
            }
        }
    }

    /// Tells watchers of the reaction a `TAGMSG` in `target` from `from` gives or takes back,
    /// if it's one.
    pub(super) fn reacted(&mut self, target: &str, from: &str, line: &Line) {
        if let Some(mut reaction) = reaction(target, from, line)
            && !self.request_reacted(&reaction)
        {
            reaction.on = self
                .recent
                .find(&reaction.to)
                .ok()
                .and_then(|said| said.about);
            if reaction.on.is_some() || !self.fetch_reacted(&reaction) {
                self.tell_reaction(reaction);
            }
        }
    }

    /// Tells watchers of `reaction`, ending the waits for it.
    pub(super) fn tell_reaction(&mut self, reaction: Reaction) {
        if self.owners(&reaction) && self.awaits.reacted(&reaction) {
            self.save_awaits();
        }
        self.tell_watchers(&Response::Reaction(reaction));
    }

    /// Asks the server for the message `reaction` is on, which zircd hadn't heard, so that the
    /// session it wakes reads what it's on as it would for one heard: only for the machine's
    /// person's reaction in a channel, which a session may wait for. Whether it asked: the
    /// reaction is told once the server answers, or once it's waited long enough
    /// ([`Daemon::fetched`]).
    fn fetch_reacted(&mut self, reaction: &Reaction) -> bool {
        if reaction.removed || !reaction.channel.starts_with('#') || !self.owners(reaction) {
            return false;
        }
        let Some(client) = &self.client else {
            return false;
        };
        if !client.has_cap("draft/chathistory") || !client.has_cap("batch") {
            return false;
        }
        // Ergo gives what's around the message with it, half before and half after.
        let around = Line::new(
            "CHATHISTORY",
            [
                "AROUND",
                reaction.channel.as_str(),
                &format!("msgid={}", reaction.to),
                "3",
            ],
        );
        if client.send(around).is_err() {
            return false;
        }
        // Nobody waits on it: what it finds is told with the reaction.
        let (answer, _) = tokio::sync::oneshot::channel();
        let what = super::Ask::Reacted {
            reaction: Box::new(reaction.clone()),
            batch: None,
            found: None,
        };
        self.wait(answer, what);
        true
    }

    /// Whether `reaction` is the machine's person's, as the server's account tag says: what
    /// a wait for a reaction takes.
    fn owners(&self, reaction: &Reaction) -> bool {
        reaction.account.as_deref().is_some_and(|account| {
            self.owner
                .as_deref()
                .is_some_and(|owner| owner.eq_ignore_ascii_case(account))
        })
    }

    /// Whether `reaction` concerns `session`: it's on a message this machine sent (`me`), as
    /// its own echo said, and the session's, or any for the machine's person.
    pub(super) fn reacted_to(
        &self,
        reaction: &Reaction,
        me: &str,
        session: Option<&str>,
    ) -> Option<Why> {
        let said = self.recent.find(&reaction.to).ok()?;
        let ours = said.from.eq_ignore_ascii_case(me);
        (ours && (session.is_none() || said.session.as_deref() == session)).then_some(Why::Reacted)
    }

    /// Remembers what a person or agent said in a channel, to react to later by its id.
    pub(super) fn remember(&mut self, delivery: &Delivery) {
        self.remember_gotcha(delivery);
        self.same_work_heard(delivery);
        self.note_hints_changed(delivery);
        if let Some(id) = &delivery.id
            && !matches!(delivery.message.event, Event::Working { .. })
        {
            let session = Some(delivery.message.session.as_str()).filter(|it| !it.is_empty());
            self.recent.heard(
                id,
                &delivery.channel,
                &delivery.from,
                session,
                Some(zirc_core::ipc::Reacted::of(delivery)),
            );
            if let Some(sealed) = &delivery.sealed {
                self.recent.sealed(id, sealed);
            }
        }
    }

    /// Gives `direct` the id `line` came with, and remembers it, to react to later.
    pub(super) fn remember_direct(&mut self, direct: &mut Direct, line: &Line) {
        direct.id = line.tag("msgid").map(str::to_owned);
        if let Some(id) = &direct.id {
            self.recent.heard(
                id,
                &direct.from,
                &direct.from,
                direct.from_session.as_deref(),
                None,
            );
        }
    }

    /// Reacts with `emoji` from `session` to the message `which` names; or takes the
    /// reaction back if `remove`. Returns where the message was said.
    pub(super) fn react(
        &self,
        session: &str,
        which: &Which<'_>,
        emoji: &str,
        remove: bool,
    ) -> Result<String> {
        let said = self.said(which)?;
        let client = self.connected()?;
        let tag = if remove {
            "+draft/unreact"
        } else {
            "+draft/react"
        };
        let mut line = Line::new("TAGMSG", [said.target.as_str()]);
        line.tags = vec![
            (tag.to_owned(), Some(emoji.to_owned())),
            ("+draft/reply".to_owned(), Some(said.id.clone())),
        ];
        client
            .send(line)
            .with_context(|| format!("couldn't react in {}", said.target))?;
        tracing::debug!(
            "{session} reacted {emoji} to {} in {}",
            said.id,
            said.target
        );
        Ok(said.target)
    }

    /// The web app this organisation signed in through, as `config.toml` names it now.
    fn web(&self) -> Option<String> {
        self.config
            .ask(|settings| crate::config::web(settings, &self.org.name))
    }

    /// Whether a post waits for its echo, to answer with its link: when the server echoes
    /// and the organisation has a web app to link to.
    pub(super) fn links_posts(&self) -> bool {
        self.client
            .as_ref()
            .is_some_and(|client| client.has_cap("echo-message"))
            && self.web().is_some()
    }

    /// Answers the post of `session`'s in `channel` waiting on its echo, `id`, with its link.
    fn post_echoed(&mut self, channel: &str, id: &str, session: &str) {
        let Some(at) = self.asking.iter().position(|asking| {
            matches!(&asking.what, super::Ask::Echo { channel: c, session: s }
                if c.eq_ignore_ascii_case(channel) && s == session)
        }) else {
            return;
        };
        let link = self.message_link(channel, id).ok();
        self.finish(
            at,
            Response::Channel {
                channel: channel.to_owned(),
                link,
            },
        );
    }

    /// Whether a decision waits for its echo, to be said as decided only once the server has
    /// it: when the server echoes.
    pub(super) fn confirms_decisions(&self) -> bool {
        self.client
            .as_ref()
            .is_some_and(|client| client.has_cap("echo-message"))
    }

    /// Answers the decision from `sender` in `channel` waiting on its echo: the server has it.
    fn decision_echoed(&mut self, channel: &str, sender: &str) {
        let Some(at) = self.asking.iter().position(|asking| {
            matches!(&asking.what, super::Ask::Decided { channel: c, sender: s }
                if c.eq_ignore_ascii_case(channel) && s == sender)
        }) else {
            return;
        };
        self.finish(
            at,
            Response::Decided {
                channels: vec![channel.to_owned()],
            },
        );
    }

    /// The link to message `id` in `target`.
    fn message_link(&self, target: &str, id: &str) -> Result<String> {
        let web = self
            .web()
            .with_context(|| format!("{} has no web app to link to", self.org.name))?;
        zirc_proto::link::conversation_link(&web, &self.org.name, target, Some(id)).with_context(
            || format!("{target} isn't a channel: only channels' messages have links"),
        )
    }

    /// The message `which` names, as heard lately or as its asker says.
    pub(super) fn said(&self, which: &Which<'_>) -> Result<Seen> {
        let said = match *which {
            Which {
                last: Some(who), ..
            } => self
                .recent
                .latest_from(who)
                .with_context(|| format!("nothing heard lately from {who} to react to"))?,
            Which {
                message: Some(id),
                to,
                ..
            } => match (self.recent.find(id), to) {
                (Ok(said), _) => said,
                // Said where the asker knows, as zirc search tells.
                (Err(_), Some(to)) => Seen {
                    id: id.to_owned(),
                    target: to.to_owned(),
                    from: String::new(),
                    session: None,
                    about: None,
                    sealed: None,
                },
                (Err(err), None) => return Err(err),
            },
            _ => anyhow::bail!("say which message: its id, or --last <who>"),
        };
        Ok(said)
    }

    /// Shows `to` that `session` is typing, for `seconds` at most.
    pub(super) fn start_typing(&mut self, session: String, to: &str, seconds: u64) -> Result<()> {
        anyhow::ensure!(
            is_nick(to) || (to.starts_with('#') && !to.contains([',', ' '])),
            "{to:?} isn't a person's nick or a channel"
        );
        self.send_typing(to, "active")?;
        let now = self.clock.now();
        self.typing.insert(
            (session, to.to_owned()),
            Typing {
                until: now + Duration::from_secs(seconds),
                sent: now,
            },
        );
        Ok(())
    }

    /// Says again, every [`TYPING_EVERY`], that the sessions typing still are, and that
    /// they've stopped once their time's up.
    pub(super) fn keep_typing(&mut self, now: Instant) {
        let mut done = Vec::new();
        let mut again = Vec::new();
        for ((session, to), typing) in &mut self.typing {
            if now >= typing.until {
                done.push((session.clone(), to.clone()));
            } else if now.duration_since(typing.sent) >= TYPING_EVERY {
                typing.sent = now;
                again.push(to.clone());
            }
        }
        for to in again {
            let _ = self.send_typing(&to, "active");
        }
        for (session, to) in done {
            self.stop_typing(&session, &to);
        }
    }

    /// Says `session` has stopped typing to `to`, if it was: it's said something there, or
    /// its time's up.
    pub(super) fn stop_typing(&mut self, session: &str, to: &str) {
        if self
            .typing
            .remove(&(session.to_owned(), to.to_owned()))
            .is_some()
        {
            let _ = self.send_typing(to, "done");
        }
    }

    /// The `TAGMSG` that tells `to` this machine is typing, or has stopped (`state`).
    pub(super) fn send_typing(&self, to: &str, state: &str) -> Result<()> {
        let client = self.connected()?;
        let mut line = Line::new("TAGMSG", [to]);
        line.tags = vec![("+typing".to_owned(), Some(state.to_owned()))];
        client
            .send(line)
            .with_context(|| format!("couldn't tell {to}"))?;
        Ok(())
    }

    /// Whether `nick` is a machine running zirc, as this one or one that says what it's
    /// working on or about to change in a channel here.
    pub(super) fn is_machine(&self, nick: &str) -> bool {
        let me = self.client.as_ref().map(Client::nick);
        me.is_some_and(|me| me.eq_ignore_ascii_case(nick))
            || self.joined.iter().any(|channel| {
                self.presence
                    .members(channel)
                    .iter()
                    .any(|member| member.nick.eq_ignore_ascii_case(nick))
                    || self
                        .presence
                        .intents(channel, now())
                        .iter()
                        .any(|intent| intent.nick.eq_ignore_ascii_case(nick))
            })
    }

    /// Sends `event` to `channel`, and, unless it's a working set, to this machine's own
    /// watchers too: the server doesn't echo a machine's messages back to it, and other
    /// sessions here want to hear them.
    pub(super) fn publish(&mut self, channel: &str, session: &str, event: Event) {
        self.publish_message(channel, self.signed(session, event));
    }

    /// `event` from `session`, with what the session's called ([`Daemon::name_of`]), the
    /// tool it runs in and the picture it goes by.
    pub(super) fn signed(&self, session: &str, event: Event) -> Message {
        Message {
            name: self.name_of(session),
            tool: self.book.get(Field::Tool, session),
            avatar: self.book.get(Field::Avatar, session),
            ..Message::new(session, event)
        }
    }

    /// What `session` is called: the name it was given ([`Request::Name`]), or else the
    /// branch it works on in a watched worktree, its last part (`notify-one-rule` for
    /// `mattias/notify-one-rule`), so that no session goes by its bare id. Never the default
    /// branch, which says nothing of its work; a worktree it was added to last comes first.
    pub(super) fn name_of(&self, session: &str) -> Option<String> {
        self.book.get(Field::Name, session).or_else(|| {
            let latest = self
                .repos
                .values()
                .filter(|watched| watched.sessions.latest() == Some(session));
            let any = self.repos.values().filter(|watched| watched.is_of(session));
            latest.chain(any).find_map(|watched| {
                let branch = watched.snapshot.branch.as_deref()?;
                if branch == watched.default_branch {
                    return None;
                }
                zirc_proto::session_name(branch.rsplit('/').next().unwrap_or(branch))
            })
        })
    }

    /// The same for a whole `message`, as an integration's, with its `app` label.
    pub(super) fn publish_message(&mut self, channel: &str, message: Message) {
        self.publish_reply(channel, message, None);
    }

    /// [`Daemon::publish_message`], as a reply to the message `reply` names by its id.
    pub(super) fn publish_reply(&mut self, channel: &str, message: Message, reply: Option<&str>) {
        self.publish_sealed(channel, message, reply, None);
    }

    /// [`Daemon::publish_reply`], with `sealed`, its sealed parts' tag value, if it has some.
    pub(super) fn publish_sealed(
        &mut self,
        channel: &str,
        message: Message,
        reply: Option<&str>,
        sealed: Option<String>,
    ) {
        if self.client.is_none() {
            tracing::debug!("not connected, dropping an event for {channel}");
            return;
        }
        if let Err(err) = self.publish_out(channel, message, reply, sealed) {
            tracing::warn!("{err:#}");
        }
    }

    /// [`Daemon::publish_sealed`], saying whether it was sent: an error when this machine
    /// isn't connected or the line couldn't be sent, and then no watcher here is told of it
    /// either. Sent is not kept: only the server's echo says that.
    pub(super) fn publish_out(
        &mut self,
        channel: &str,
        mut message: Message,
        reply: Option<&str>,
        sealed: Option<String>,
    ) -> Result<()> {
        if message.name.is_none() {
            message.name = self.name_of(&message.session);
        }
        if message.tool.is_none() {
            message.tool = self.book.get(Field::Tool, &message.session);
        }
        if message.avatar.is_none() {
            message.avatar = self.book.get(Field::Avatar, &message.session);
        }
        if message.task.is_none() {
            message.task = self.tasks.on(&message.session).cloned();
        }
        // A repository's scope goes out in `#agents`, saying the repository.
        let scope = channel;
        let channel = &self.on_wire(scope, &mut message);
        if matches!(message.event, Event::Text { .. } | Event::Custom { .. }) {
            self.stop_typing(&message.session, channel);
        }
        let Some(client) = &self.client else {
            anyhow::bail!("not connected to the server");
        };
        tracing::debug!("sending a {} event to {channel}", message.event.kind());
        let nick = client.nick().to_owned();
        // Debug, such as a working set or a lease event, goes out of the channel's history
        // (send_message).
        let tags: Vec<_> = reply
            .map(|id| ("+draft/reply".to_owned(), Some(id.to_owned())))
            .into_iter()
            .chain(
                sealed
                    .clone()
                    .map(|value| (zirc_proto::sealed::TAG.to_owned(), Some(value))),
            )
            .collect();
        zirc_core::send::send_tagged(client, channel, &message, &tags)
            .with_context(|| format!("couldn't send to {channel}"))?;
        // This machine's own working sets are its worktrees', kept apart.
        if !matches!(message.event, Event::Working { .. }) {
            let default = self.default_branch(scope);
            self.presence
                .apply(scope, &nick, &message, now(), default.as_deref());
        }
        if !matches!(message.event, Event::Working { .. }) {
            let mut delivery = Delivery {
                account: self.account.clone(),
                reply: reply.map(str::to_owned),
                sealed,
                ..Delivery::new(channel, nick, message)
            };
            vouch(&mut delivery, self.zirc_dev);
            only_cis_word(&mut delivery);
            self.tell_watchers(&Response::Delivery(delivery));
        }
        Ok(())
    }
}
