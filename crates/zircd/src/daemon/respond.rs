//! The daemon's answers to socket requests ([`Daemon::respond`]), one arm per request.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use zirc_core::agents::Agents;
use zirc_core::ipc::{Request, Response, Status, Target};
use zirc_core::paths;
use zirc_proto::decision::Decision;
use zirc_proto::{Event, Message};

use crate::git;

use super::book::Field;
use super::{Daemon, Guest, Watched, Which, wakes};

impl Daemon {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per request, each a call: the match is exhaustive, so a new request can't be forgotten"
    )]
    pub(super) async fn respond(&mut self, request: Request) -> Result<Response> {
        if let Some((session, how)) = wakes::acting(&request) {
            let session = session.to_owned();
            self.book.seen(&session, super::now());
            self.session_acted(&session, how);
        }
        if let Some(guest) = &self.org.guest {
            anyhow::ensure!(
                Guest::takes(&request),
                "{} is a guest's terminal, in {} alone: it posts there, and nothing else",
                self.org.name,
                guest.room
            );
        }
        Ok(match request {
            Request::Status => self.status(),
            Request::RepoAdd { path, session } => {
                self.add(&path, session).await?;
                self.save();
                self.repo_list()
            }
            Request::RepoRemove { path, session } => self.repo_remove(path, session).await?,
            Request::RepoList => self.repo_list(),
            Request::Post {
                repo,
                session,
                text,
                to,
                app,
                everyone,
                reply,
            } => match reply {
                Some(reply) => self.say_reply(&session, &text, &reply, app.as_deref(), &[])?,
                None => {
                    self.say(&repo, session, (text, Vec::new()), to, app, everyone)
                        .await?
                }
            },
            Request::SealedPost {
                repo,
                session,
                text,
                to,
                reply,
            } => {
                let split = zirc_core::sealed::split(&text);
                match reply {
                    Some(reply) => {
                        self.say_reply(&session, &split.text, &reply, None, &split.parts)?
                    }
                    None => {
                        self.say(&repo, session, (split.text, split.parts), to, None, false)
                            .await?
                    }
                }
            }
            Request::Unseal { message, part } => self.unseal(&message, part)?,
            Request::Decide {
                repo,
                session,
                quote,
                about,
                link,
                for_,
                replaces,
            } => {
                let decision = Decision {
                    by: String::new(),
                    quote,
                    about,
                    session,
                    link,
                    for_,
                    replaces,
                };
                let channels = self.decide(&repo, decision).await?;
                Response::Decided { channels }
            }
            Request::SetState {
                repo,
                session,
                name,
                level,
                why,
                owner,
                link,
                until,
            } => {
                let state = zirc_proto::state::State {
                    repo: String::new(),
                    name,
                    level,
                    why,
                    owner,
                    link,
                    until,
                };
                let channel = self.set_state(&repo, &session, state).await?;
                Response::channel(channel)
            }
            Request::AskOwner {
                session,
                question,
                yes,
                no,
                recommend,
                irreversible,
            } => self.ask_owner(&session, question, yes, no, recommend, irreversible)?,
            Request::Ask { session, to, text } => {
                let (id, to) = self.ask(&session, &to, &text)?;
                Response::Asked { id, to }
            }
            Request::Note { session, to, text } => {
                let to = self.note(&session, &to, &text)?;
                Response::Noted { to }
            }
            Request::Reply {
                session,
                query,
                text,
            } => {
                let to = self.reply(&session, &query, &text)?;
                self.stop_typing(&session, &to);
                Response::channel(to)
            }
            Request::Name {
                session,
                name,
                tool,
                avatar,
                title,
                link,
            } => {
                let (tool, title, link) = (tool.as_deref(), title.as_deref(), link.as_deref());
                self.name_session(&session, name, tool, avatar, title, link)?
            }
            Request::Queries { set } => self.queries(set)?,
            Request::Dm { session, text, to } => {
                let to = self.dm(&session, &text, to)?;
                self.stop_typing(&session, &to);
                Response::channel(to)
            }
            Request::Emit {
                repo,
                session,
                kind,
                data,
                text,
                to,
                app,
            } => {
                self.emit_custom(&repo, session, (kind, data, text), to, app)
                    .await?
            }
            Request::Record {
                session,
                kind,
                data,
            } => self.used(&session, kind, data)?,
            Request::Role {
                session,
                role,
                join,
            } => self.role(&session, &role, join)?,
            Request::React {
                session,
                message,
                last,
                to,
                emoji,
                remove,
                ..
            } => {
                let which = Which {
                    message: message.as_deref(),
                    last: last.as_deref(),
                    to: to.as_deref(),
                };
                let to = self.react(&session, &which, &emoji, remove)?;
                Response::channel(to)
            }
            Request::Typing {
                session,
                to,
                seconds,
                ..
            } => {
                self.start_typing(session, &to, seconds)?;
                Response::channel(to)
            }
            Request::Intend {
                repo,
                session,
                summary,
                paths,
                seconds,
            } => {
                self.awaits_said(&session, &summary);
                self.task_intended(&session);
                self.intend(&repo, &session, summary, &paths, seconds)
                    .await?
            }
            Request::Join {
                repo,
                topic,
                session,
            } => self.join_topic(&repo, &topic, session).await?,
            Request::Leave {
                repo,
                topic,
                session,
            } => self.leave_topic(&repo, &topic, session).await?,
            Request::Who => self.who(),
            Request::LocalSessions => self.local_sessions(),
            Request::LiveSessions => {
                let here: Vec<String> = self.live.sessions();
                let (live, gone) = self.elsewhere.now(&here);
                Response::LiveSessions { live, gone }
            }
            Request::Wakes { days } => Response::Wakes {
                counts: self.wake_counts(days),
                breaker: Some(self.breaker.state(self.clock.now())),
            },
            Request::Task { session, landing } => Response::Task {
                task: if landing {
                    Some(self.task_landed(&session))
                } else {
                    self.tasks.on(&session).cloned()
                },
            },
            Request::WakesResume => Response::Wakes {
                counts: Vec::new(),
                breaker: Some(self.wakes_resume()),
            },
            Request::Lease {
                name,
                session,
                seconds,
                pid,
                wait,
            } => self.lease(&name, &session, seconds, pid, wait).await?,
            Request::LeaseRelease { name, session } => self.lease_release(&name, &session)?,
            Request::Leases => self.leases(),
            Request::Watch { .. } => anyhow::bail!("watching is its own kind of connection"),
            Request::Trim { .. }
            | Request::Read { .. }
            | Request::Conversations { .. }
            | Request::Workflow { .. } => {
                anyhow::bail!("this waits on the server, and is answered when it says")
            }
            Request::WaitFor {
                session,
                on,
                states,
                reactions,
                drop,
            } => self.wait_for(&session, on, states, (reactions, drop)),
            Request::Stack {
                session,
                branch,
                tip,
                drop,
                gone,
            } => self.stack(&session, branch.as_deref(), tip.as_deref(), (drop, gone)),
            Request::Stop => {
                tracing::info!("stopping, as asked");
                self.stopping = true;
                Response::Done
            }
            Request::Restart => {
                tracing::info!("restarting, as asked");
                self.restart();
                Response::Done
            }
        })
    }

    /// How this machine's zircd stands ([`Request::Status`]).
    fn status(&mut self) -> Response {
        // The channels it's in on the server, not the repositories' scopes ([`Daemon::is_scope`]).
        let desired = self.desired_channels();
        let channels = self
            .joined
            .iter()
            .filter(|channel| desired.contains(*channel) || !self.is_scope(channel))
            .cloned()
            .collect();
        Response::Status(Status {
            connected: self.client.is_some(),
            server: self.server.clone(),
            nick: self.client.as_ref().map(|client| client.nick().to_owned()),
            channels,
            problem: self.problem.clone(),
            watching: self.watching(),
            protocol: zirc_core::ipc::PROTOCOL,
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            orgs: Vec::new(),
            breaker: Some(self.breaker.state(self.clock.now())),
            guest: self.org.guest.as_ref().map(|guest| guest.of.clone()),
            busy_ms: Some(self.busy.longest_ms(Instant::now())),
        })
    }

    /// Stops watching the repository at `path`, or only stops counting `session` as working
    /// in it ([`Request::RepoRemove`]).
    async fn repo_remove(&mut self, path: PathBuf, session: Option<String>) -> Result<Response> {
        let root = git::root(&path).await.unwrap_or(path);
        if let Some(session) = session {
            let watched = self.repos.get_mut(&root);
            anyhow::ensure!(
                watched.is_some_and(|watched| watched.sessions.remove(&session)),
                "{session} isn't working in {}",
                root.display()
            );
        } else {
            let watched = self.repos.remove(&root);
            anyhow::ensure!(watched.is_some(), "{} isn't being watched", root.display());
            // A linked worktree stays unwatched, though its main checkout is watched
            // ([`Daemon::find_worktrees`]).
            if root.join(".git").is_file() {
                self.unwatched.insert(root);
            }
        }
        self.save();
        self.reconcile();
        Ok(self.repo_list())
    }

    /// Says `text` as `session` ([`Request::Post`]): in a guest's terminal's room; as a note
    /// when it's addressed to one agent session; else in the channel `to` names, or the one
    /// such a post goes to.
    async fn say(
        &mut self,
        repo: &Path,
        session: String,
        (text, sealed): (String, Vec<String>),
        to: Target,
        app: Option<String>,
        everyone: bool,
    ) -> Result<Response> {
        self.awaits_said(&session, &text);
        // A guest's terminal says everything in its room.
        if let Some(guest) = &self.org.guest {
            let channel = guest.room.clone();
            self.connected()?;
            anyhow::ensure!(
                self.joined.contains(&channel),
                "{channel} isn't joined yet: zircd is joining it"
            );
            let sealed = self.seal_for(&channel, &sealed, None)?;
            let message = Message {
                app: app.as_deref().and_then(zirc_proto::app_label),
                ..Message::new(session, Event::Text { body: text })
            };
            self.publish_sealed(&channel, message, None, sealed);
            return Ok(Response::channel(channel));
        }
        // Addressed to one agent session, it's a note for it alone.
        if matches!(to, Target::Agents)
            && !everyone
            && let Some((address, said)) = self.note_address(&text)
        {
            anyhow::ensure!(
                sealed.is_empty(),
                "a note to one session can't hold sealed parts: post them in a channel"
            );
            let to = self.note_to(&session, &address, &said)?;
            return Ok(Response::Noted { to });
        }
        let channel = self.channel_to(repo, to).await?;
        let sealed = self.seal_for(&channel, &sealed, None)?;
        self.task_said(&session, &text, &channel);
        let message = Message {
            app: app.as_deref().and_then(zirc_proto::app_label),
            everyone,
            repo: Some(self.watched(repo).await?.project.repo.clone()),
            ..Message::new(session.clone(), Event::Text { body: text.clone() })
        };
        self.publish_sealed(&channel, message, None, sealed);
        self.task_ended(&session, &text);
        Ok(Response::channel(self.wire_name(&channel)))
    }

    /// Says `text` from `session` as a reply to the message `reply` names, in the channel it
    /// was said in ([`Request::Post`]'s `reply`): only a channel's, as a direct message is
    /// answered with `zirc dm`.
    fn say_reply(
        &mut self,
        session: &str,
        text: &str,
        reply: &str,
        app: Option<&str>,
        sealed: &[String],
    ) -> Result<Response> {
        let said = self.said(&super::Which {
            message: Some(reply),
            last: None,
            to: None,
        })?;
        anyhow::ensure!(
            said.target.starts_with('#'),
            "{} was said to you directly, not in a channel: answer it with zirc dm",
            said.id
        );
        self.connected()?;
        if let Some(guest) = &self.org.guest {
            anyhow::ensure!(
                guest.room.eq_ignore_ascii_case(&said.target),
                "a guest's terminal says nothing outside {}",
                guest.room
            );
        }
        let sealed = self.seal_for(&said.target, sealed, Some(&said.id))?;
        self.awaits_said(session, text);
        self.task_said(session, text, &said.target);
        let message = Message {
            app: app.and_then(zirc_proto::app_label),
            ..Message::new(
                session,
                Event::Text {
                    body: text.to_owned(),
                },
            )
        };
        self.publish_sealed(&said.target, message, Some(&said.id), sealed);
        self.task_ended(session, text);
        Ok(Response::channel(said.target))
    }

    /// `session` joins or leaves `role` ([`Request::Role`]), said with its machine's live
    /// sessions from the next second.
    fn role(&mut self, session: &str, role: &str, join: bool) -> Result<Response> {
        zirc_proto::workflow::check_role(role).map_err(|why| anyhow::anyhow!(why))?;
        self.book.seen(session, super::now());
        let mut roles = self.roles_of(session);
        if join {
            roles.insert(role.to_owned());
        } else {
            roles.remove(role);
        }
        let joined: Vec<String> = roles.into_iter().collect();
        self.book
            .set(Field::Roles, session, joined.join(","))
            .context("couldn't save the session's roles")?;
        Ok(Response::Roles { roles: joined })
    }

    /// The roles `session` joined.
    pub(super) fn roles_of(&self, session: &str) -> std::collections::BTreeSet<String> {
        self.book
            .get(Field::Roles, session)
            .unwrap_or_default()
            .split(',')
            .filter(|role| !role.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Names `session`, and keeps its tool, avatar, title and link ([`Request::Name`]).
    fn name_session(
        &mut self,
        session: &str,
        name: Option<String>,
        tool: Option<&str>,
        avatar: Option<String>,
        title: Option<&str>,
        link: Option<&str>,
    ) -> Result<Response> {
        self.book.seen(session, super::now());
        if let Some(avatar) = avatar {
            let avatar = zirc_proto::agent_avatar(&avatar).with_context(|| {
                let ids: Vec<&str> = zirc_proto::AGENT_AVATARS
                    .iter()
                    .map(|(id, _)| *id)
                    .collect();
                format!("{avatar:?} isn't a role: pick one of {}", ids.join(", "))
            })?;
            self.book
                .set(Field::Avatar, session, avatar.to_owned())
                .context("couldn't save the session's avatar")?;
        }
        if let Some(name) = name {
            let name = zirc_proto::session_name(&name).with_context(|| {
                format!("{name:?} has nothing a name can have: letters, digits, - _ or .")
            })?;
            self.book
                .set(Field::Name, session, name)
                .context("couldn't save the session's name")?;
        }
        if let Some(tool) = tool.and_then(zirc_proto::session_tool) {
            self.book
                .set(Field::Tool, session, tool)
                .context("couldn't save the session's tool")?;
        }
        self.title_session(session, title, link)?;
        Ok(Response::Done)
    }

    /// Who may query this machine's agents, after setting it when `set` says
    /// ([`Request::Queries`]).
    fn queries(&mut self, set: Option<zirc_core::agents::Queries>) -> Result<Response> {
        if let Some(queries) = set {
            let agents = Agents { queries };
            agents
                .save(&paths::agents(&self.home))
                .context("couldn't save who may query this machine's agents")?;
            self.agents = agents;
        }
        Ok(Response::Queries {
            queries: self.agents.queries,
            colleagues_kept_out: !self.colleagues().queries,
        })
    }

    /// Sends a team's own event, `(kind, data, text)`, as `session` ([`Request::Emit`]),
    /// refusing a kind that isn't one and a card apps can't draw.
    async fn emit_custom(
        &mut self,
        repo: &Path,
        session: String,
        (kind, data, text): (String, serde_json::Value, String),
        to: Target,
        app: Option<String>,
    ) -> Result<Response> {
        let channel = self.channel_to(repo, to).await?;
        if let Some(problem) = zirc_proto::check_kind(&kind) {
            anyhow::bail!("{kind}: {problem}");
        }
        // A card apps can't draw is refused here, whoever's asking, saying why.
        if let Some(card) = data.get(zirc_proto::card::KEY) {
            let card: zirc_proto::card::Card = serde_json::from_value(card.clone())
                .context("its card isn't a card: see docs/cards.md")?;
            card.validate()
                .map_err(|why| anyhow::anyhow!("its card can't be drawn: {why}"))?;
        }
        let message = Message {
            app: app.as_deref().and_then(zirc_proto::app_label),
            repo: Some(self.watched(repo).await?.project.repo.clone()),
            ..Message::new(session, Event::Custom { kind, data, text })
        };
        // Its echo may not come back: this machine's own gotcha is kept as it's said.
        if zirc_core::gotchas::is_room(&channel) {
            self.remember_gotcha(&zirc_core::ipc::Delivery {
                time: Some(zirc_core::time::tag(super::now())),
                ..zirc_core::ipc::Delivery::new(
                    channel.clone(),
                    self.client
                        .as_ref()
                        .map(|client| client.nick().to_owned())
                        .unwrap_or_default(),
                    message.clone(),
                )
            });
        }
        self.emit(&channel, message)?;
        Ok(Response::channel(self.wire_name(&channel)))
    }

    /// Joins `session` to the organisation's topic `topic` ([`Request::Join`]), from inside
    /// the watched repository `repo`.
    async fn join_topic(
        &mut self,
        repo: &Path,
        topic: &str,
        session: Option<String>,
    ) -> Result<Response> {
        self.watched(repo).await?;
        let channel = zirc_core::project::topic_channel(topic);
        self.topics
            .entry(channel.clone())
            .or_default()
            .insert(session);
        self.save_topics();
        self.reconcile();
        Ok(Response::channel(channel))
    }

    /// Takes `session` out of a topic, or everyone when it names none ([`Request::Leave`]).
    async fn leave_topic(
        &mut self,
        repo: &Path,
        topic: &str,
        session: Option<String>,
    ) -> Result<Response> {
        self.watched(repo).await?;
        let channel = zirc_core::project::topic_channel(topic);
        let joined = match session {
            None => self.topics.remove(&channel).is_some(),
            Some(session) => match self.topics.get_mut(&channel) {
                None => false,
                Some(joiners) => {
                    let left = joiners.remove(&Some(session));
                    if joiners.is_empty() {
                        self.topics.remove(&channel);
                    }
                    left
                }
            },
        };
        anyhow::ensure!(joined, "{channel} isn't a joined topic");
        self.save_topics();
        self.reconcile();
        Ok(Response::channel(channel))
    }

    /// Has `session`'s watch wake on what `on` and `states` name ([`Request::WaitFor`]).
    fn wait_for(
        &mut self,
        session: &str,
        on: Vec<String>,
        states: Vec<zirc_core::ipc::StateWait>,
        (reactions, drop): (Vec<String>, bool),
    ) -> Response {
        self.awaits.add(session, on);
        self.awaits.add_states(session, states);
        if drop {
            self.awaits.drop_reactions(session, &reactions);
        } else {
            self.awaits.add_reactions(session, reactions);
        }
        self.save_awaits();
        Response::Awaits {
            on: self.awaits.of(session),
        }
    }

    /// Notes, forgets or says the branches `session` is stacked on ([`Request::Stack`]).
    fn stack(
        &mut self,
        session: &str,
        branch: Option<&str>,
        tip: Option<&str>,
        (drop, gone): (bool, bool),
    ) -> Response {
        match (branch, tip) {
            (Some(branch), _) if drop => {
                self.awaits.unstack(session, branch);
                self.save_awaits();
            }
            (Some(branch), Some(tip)) => {
                tracing::info!("{session} is stacked on {branch} at {tip}");
                self.awaits.stack(session, branch, tip);
                self.save_awaits();
                self.read_base(session, branch, tip);
                // Origin said so as the stack was said: no ref of it is left to see go.
                if gone && let Some(root) = self.root_of(session) {
                    self.stacks_gone(&root, vec![(branch.to_owned(), tip.to_owned())]);
                }
            }
            _ => {}
        }
        Response::Stacks {
            stacks: self.awaits.stacks(session),
        }
    }

    /// The watched repository containing `path`.
    pub(super) async fn watched(&self, path: &Path) -> Result<&Watched> {
        let root = git::root(path).await?;
        self.repos
            .get(&root)
            .with_context(|| format!("{} isn't being watched", root.display()))
    }
}
