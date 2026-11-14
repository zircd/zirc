//! The repositories this machine watches: adding them, saving them, and the channels
//! they put it in.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::Result;
use zirc_core::ipc::{Delivery, Repo, Response};
use zirc_core::project::ORG_CHANNEL;
use zirc_core::{channels, decisions, paths};
use zirc_irc::Line;
use zirc_proto::{Event, Message};

use crate::git;

use super::{
    Daemon, ReadRepo, SAVING, SHARED_FOR, Saved, SavedTopics, Sessions, WATCHER_SESSION, Watched,
    now, post_room, read_repo, read_saved_repos, warn_once,
};

impl Daemon {
    pub(super) fn repo_list(&self) -> Response {
        Response::Repos {
            repos: self
                .repos
                .iter()
                .map(|(path, watched)| Repo {
                    path: path.clone(),
                    channel: watched.channel.clone(),
                    sessions: watched.sessions.0.clone(),
                    org: self.org.several.then(|| self.org.name.clone()),
                    shared: self
                        .shared
                        .get(path)
                        .map(|shared| {
                            shared
                                .iter()
                                .filter(|it| now().saturating_sub(it.at) < SHARED_FOR)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect(),
        }
    }

    /// Notes that `session`, registered at `root` now, was registered at `other` too, when
    /// `other` is another worktree still there, not the main checkout it leaves for a
    /// worktree of its own: two sessions under one name. Warned of, once, in the session's
    /// watch, and for a day in what `zirc repo add` and `zirc status` say; never refused.
    pub(super) fn shared_with(&mut self, root: &Path, session: &str, other: &Path, channel: &str) {
        if !other.join(".git").is_file() {
            return;
        }
        let shared = zirc_core::ipc::Shared {
            session: session.to_owned(),
            with: other.to_owned(),
            at: now(),
        };
        let body = zirc_core::ipc::shared_warning(
            &shared,
            &root.display().to_string(),
            &other.display().to_string(),
        );
        let list = self.shared.entry(root.to_owned()).or_default();
        list.retain(|it| it.session != session && now().saturating_sub(it.at) < SHARED_FOR);
        list.push(shared);
        let me = self.client.as_ref().map(|client| client.nick().to_owned());
        self.tell_watchers(&Response::Delivery(Delivery::new(
            channel,
            me.unwrap_or_default(),
            Message::new(
                WATCHER_SESSION,
                Event::Text {
                    body: format!("{session}: {body}"),
                },
            ),
        )));
    }

    pub(super) async fn add(&mut self, path: &Path, session: Option<String>) -> Result<()> {
        let root = git::root(path).await?;
        // Added by hand, it's watched again, even a worktree a person had stopped watching.
        self.unwatched.remove(&root);
        let mut sessions = Sessions::default();
        if let Some(session) = session {
            // Working here, it no longer works in another checkout of the repository, as
            // one registered in the main checkout before it had a worktree of its own: a
            // worktree of this one, remote or not, or another clone of the same remote.
            let common = git::common_dir(&root).await?;
            let remote = git::remote_url(&root).await;
            let mut taken = Vec::new();
            for (other, watched) in &mut self.repos {
                if *other == root || !watched.sessions.contains(&session) {
                    continue;
                }
                let same = watched.common == common
                    || (remote.is_some() && git::remote_url(other).await == remote);
                if same && watched.sessions.remove(&session) {
                    tracing::info!(
                        "{session} works in {} now, not {}",
                        root.display(),
                        other.display()
                    );
                    taken.push((other.clone(), watched.channel.clone()));
                }
            }
            for (other, channel) in taken {
                self.shared_with(&root, &session, &other, &channel);
            }
            sessions.add(session);
        }
        if let Some(watched) = self.repos.get_mut(&root) {
            for it in sessions.0 {
                watched.add_session(it.session);
            }
            // Who's working on what may be different now.
            let channel = watched.channel.clone();
            if self.joined.contains(&channel) {
                self.announce(&channel, false, self.clock.now());
                self.check_overlaps(&channel);
            }
            return Ok(());
        }
        let read = read_repo(root.clone(), self.inputs.clone()).await?;
        self.watch_read(read, sessions);
        self.invite_owner_to(&root);
        Ok(())
    }

    /// Watches the linked worktrees of each repository whose main checkout is watched, as
    /// its person asked for the repository, with no session until one adds itself there: so
    /// what changes in each is seen whether or not an agent ran `zirc repo add` in it, as
    /// `zirc repo here` already counts it watched. Never one a person stopped watching
    /// ([`Daemon::unwatched`]), and one it couldn't read only once: `zirc repo add` there
    /// tries again.
    pub(super) async fn find_worktrees(&mut self) {
        if self.org.guest.is_some() {
            return;
        }
        let commons: BTreeSet<PathBuf> = self
            .repos
            .iter()
            .filter(|(root, watched)| {
                git::main_checkout(&watched.common).as_deref() == Some(root.as_path())
            })
            .map(|(_, watched)| watched.common.clone())
            .collect();
        let new: Vec<PathBuf> = commons
            .iter()
            .flat_map(|common| git::linked_worktrees(common))
            .filter(|root| {
                !self.repos.contains_key(root)
                    && !self.unwatched.contains(root)
                    && !self.warned.said(warn_once::About::Scan, root)
            })
            .collect();
        let mut found = false;
        for root in new {
            match read_repo(root.clone(), self.inputs.clone()).await {
                Ok(read) => {
                    self.warned.ok(warn_once::About::Scan, &root);
                    tracing::info!(
                        "watching {}, a worktree of a watched checkout",
                        root.display()
                    );
                    self.watch_read(read, Sessions::default());
                    found = true;
                }
                Err(err) => {
                    let cause = format!("{err:#}");
                    let message =
                        format!("couldn't watch the worktree {}: {cause}", root.display());
                    self.warned
                        .warn(warn_once::About::Scan, &root, &cause, &message);
                }
            }
        }
        if found {
            self.save();
        }
    }

    /// This machine has just started watching the repository at `root`, as `zirc repo
    /// add` asked: its owner's apps join its channel and rooms, by invitation, once this
    /// machine is in each. Only rooms it's new to: one it was in already, as for another
    /// checkout of the same repository, its owner was invited to then. Never on restoring
    /// what was watched, nor for a machine without an owner.
    fn invite_owner_to(&mut self, root: &Path) {
        if self.owner.is_none() {
            return;
        }
        let Some(watched) = self.repos.get(root) else {
            return;
        };
        let rooms: Vec<String> = std::iter::once(&watched.channel)
            .chain(watched.rooms())
            .cloned()
            .chain(watched.standing())
            .filter(|room| !self.joined.contains(room))
            .collect();
        self.invite_owner.extend(rooms);
    }

    /// Watches a repository [`read_repo`] read, for `sessions`; or, when it's watched
    /// already, as another path into it can be, adds them to it.
    pub(super) fn watch_read(&mut self, read: ReadRepo, sessions: Sessions) {
        let ReadRepo {
            root,
            common,
            project,
            default_branch,
            snapshot,
            watcher,
        } = read;
        if let Some(watched) = self.repos.get_mut(&root) {
            for it in sessions.0 {
                watched.add_session(it.session);
            }
            return;
        }
        let channel = project.repo_channel();
        tracing::info!("watching {} for {channel}", root.display());
        let joined = self.joined.contains(&channel);
        let (agents, areas) = post_room::rooms_of(&root, &project, &mut self.warned);
        self.repos.insert(
            root,
            Watched {
                common,
                room: agents,
                areas,
                project,
                channel: channel.clone(),
                default_branch,
                snapshot,
                touched: None,
                held: false,
                sessions,
                announced: VecDeque::new(),
                _watcher: watcher,
            },
        );
        self.reconcile();
        // A channel joined already won't see us join again, so say what we're doing now.
        if joined {
            self.announce(&channel, false, self.clock.now());
            self.check_overlaps(&channel);
        }
    }

    pub(super) fn save(&self) {
        let _saving = SAVING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = paths::repos(&self.home);
        // The other organisations' repositories, as they saved them.
        let Some(others) = read_saved_repos(&path) else {
            return;
        };
        // With one organisation, as before several: a host that moves renames it.
        let org = self.org.several.then(|| self.org.name.clone());
        let watched = self.repos.iter().map(|(path, watched)| Saved {
            path: path.clone(),
            sessions: watched.sessions.0.clone(),
            org: org.clone(),
            unwatched: false,
        });
        // Those still there: a worktree removed since is nothing to remember.
        let unwatched = self
            .unwatched
            .iter()
            .filter(|path| path.exists())
            .map(|path| Saved {
                path: path.clone(),
                sessions: Vec::new(),
                org: org.clone(),
                unwatched: true,
            });
        let saved: Vec<Saved> = others
            .into_iter()
            .filter(|saved| !saved.is_of(&self.org))
            .chain(watched)
            .chain(unwatched)
            .collect();
        let json = serde_json::to_string_pretty(&saved).expect("paths serialize");
        if let Err(err) = paths::write_private(&path, &json) {
            tracing::warn!("couldn't save the watched repositories: {err}");
        }
    }

    pub(super) fn save_topics(&self) {
        let json = serde_json::to_string_pretty(&SavedTopics::Joiners(self.topics.clone()))
            .expect("names serialize");
        if let Err(err) = paths::write_private(&paths::topics(&self.org.state), &json) {
            tracing::warn!("couldn't save the joined topics: {err}");
        }
    }

    /// The channels to be in: the organisation's and its `#ops`, where decisions are, each
    /// watched repository's, and the topics joined, a branch's channel among them when
    /// joined by name.
    pub(super) fn desired_channels(&self) -> BTreeSet<String> {
        if let Some(guest) = &self.org.guest {
            return BTreeSet::from([guest.room.clone()]);
        }
        let mut desired: BTreeSet<String> = self.topics.keys().cloned().collect();
        // A branch's channel only by choice, as a topic named for the branch: joining every
        // pushed branch's, as a landing pushes each, made a channel per landing that nobody
        // else was in.
        for watched in self.repos.values() {
            desired.insert(watched.channel.clone());
            // Its agent room, which every session here hears, and its areas' (post_room).
            desired.extend(watched.rooms().cloned());
            // Its landings and ops, which a new repository has from the first day.
            desired.extend(watched.standing());
        }
        // Only while this machine's asked its owner something, to hear their answer.
        if self.requests.any_open() {
            desired.insert(zirc_proto::request::CHANNEL.to_owned());
        }
        if !self.repos.is_empty() {
            desired.insert(ORG_CHANNEL.to_owned());
            // Decisions people made concern every session here, wherever they're working,
            // and so does how shared things stand, such as a broken master.
            desired.insert(decisions::CHANNEL.to_owned());
            desired.insert(zirc_core::states::CHANNEL.to_owned());
        }
        desired
    }

    /// Joins the channels to be in and leaves the rest.
    pub(super) fn reconcile(&mut self) {
        let desired = self.desired_channels();
        let Some(client) = &self.client else { return };
        let plan = channels::reconcile(
            self.joined.iter().map(String::as_str),
            desired.iter().map(String::as_str),
        );
        for channel in &plan.join {
            let _ = client.join(channel);
        }
        for channel in &plan.part {
            let _ = client.send(Line::new("PART", [channel.as_str()]));
        }
    }
}
