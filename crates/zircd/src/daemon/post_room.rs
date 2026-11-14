//! A repository's agent room: `#<repo>/agents` unless the team's `.zirc/config.toml` names
//! another ([`zirc_core::team`]). It's where its sessions' lines go, leaving the repository's
//! channel to its people: a plain `zirc post` ([`Target::Default`]), commits and pushes,
//! intents, `taking:`/`done:`/`dropped:`, and `@everyone`, which reaches every agent. Only a
//! post for a person, naming or addressing one, goes to the repository's channel.
//!
//! A team can also give its repository areas ([`zirc_core::team::Rooms::areas`]), each a
//! room of its own, `#<repo>/<area>`: a plain post from a worktree whose changes are mostly
//! in one area's paths goes to that area's room instead of the agent room. A status line
//! (`taking:`, `done:`, `dropped:`) still goes to the agent room, which every session hears.
//! And a plain post addressed to one agent session (`coordinator: landed it`) is a note for
//! it alone ([`zirc_core::notes`]), in no room at all.
//!
//! zircd joins each watched repository's rooms as it joins its channel, and takes what's said
//! there as the repository's: claims, intents and pushes count for `zirc who`, overlaps,
//! conflicts and wakes as if said in its channel. Nothing said in the agent room wakes a
//! session unless it names it, or `@everyone`; nothing said in an area's unless it names the
//! session or a path it's changing, or the session joined the area as a topic.
//!
//! [`Target::Default`]: zirc_core::ipc::Target::Default

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use zirc_core::ipc::Target;
use zirc_core::notes;
use zirc_core::project::Project;
use zirc_core::team::{self, Room, Team};

use super::warn_once::{About, WarnOnce};
use super::{Daemon, Watched, git};

/// Why a session's post to the repository's channel is refused, and what to do instead.
pub(super) const REPO_CHANNEL_REFUSED: &str = "not posted: the repository's channel is for its \
    people, and this isn't for one. Name the person (`ada: …`), tell your user with `zirc dm`, \
    or drop --to repo to post in the agents room; or post it as it is with --force";

/// The rooms of the repository at `root`: its agent room, `None` when the team's config makes
/// it the repository's channel, and its areas' rooms. An unreadable config is said, and the
/// defaults taken: once for each way it's unreadable ([`WarnOnce`]), not at every scan.
pub(super) fn rooms_of(
    root: &Path,
    project: &Project,
    warned: &mut WarnOnce,
) -> (Option<String>, Vec<String>) {
    let team = match Team::load(root) {
        Ok(team) => {
            warned.ok(About::TeamConfig, root);
            team
        }
        Err(err) => {
            let cause = err.to_string();
            warned.warn(
                About::TeamConfig,
                root,
                &cause,
                &format!("{cause}: using the agents channel"),
            );
            Team::default()
        }
    };
    let agents = match team.room() {
        Room::Repo => None,
        Room::Topic(topic) => Some(project.topic_channel(&topic, false)),
    };
    let areas = team
        .areas()
        .map(|area| project.topic_channel(area, false))
        .filter(|area| *area != project.repo_channel() && Some(area) != agents.as_ref())
        .collect();
    (agents, areas)
}

impl Watched {
    /// Where its sessions' lines go: its agent room, or its channel.
    pub(super) fn agents_channel(&self) -> &str {
        self.room.as_deref().unwrap_or(&self.channel)
    }

    /// Its standing rooms besides the agents': `#<repo>/landings` for CI
    /// ([`zirc_proto::STANDING_ROOMS`]). Its owner is invited to them, and zircd joins them,
    /// from the day it's watched, so a person alone with their agents finds each room's job
    /// waiting rather than created by whichever script speaks first. Alerts and a broken
    /// master go to the organisation's `#ops`, which every tenant has. Joining one wakes no
    /// session: [`Daemon::is_post_room`] doesn't count them.
    pub(super) fn standing(&self) -> impl Iterator<Item = String> + '_ {
        zirc_proto::STANDING_ROOMS
            .iter()
            .filter(|room| **room != zirc_proto::AGENTS_ROOM)
            .map(|room| self.project.topic_channel(room, false))
    }

    /// Its rooms besides its channel: its agent room and its areas'.
    pub(super) fn rooms(&self) -> impl Iterator<Item = &String> {
        self.room.iter().chain(&self.areas)
    }
}

impl Daemon {
    /// Where `text`, posted with no target in the repository at `repo`, goes: its channel
    /// when it's for a person; else, when the team has areas and it isn't a status line,
    /// the room of the area most of the worktree's changes are in; else its agent room.
    pub(super) async fn default_target(&self, repo: &Path, text: &str) -> Result<Target> {
        let root = git::root(repo).await?;
        let (people, agents) = self.known_names();
        let people: Vec<&str> = people.iter().map(String::as_str).collect();
        let agents: Vec<&str> = agents.iter().map(String::as_str).collect();
        if team::for_people(text, &people, &agents) {
            return Ok(Target::Repo);
        }
        let team = Team::load(&root)?;
        if !team::is_status(text) && team.areas().next().is_some() {
            // Its work, committed or not, that the default branch doesn't have yet; without a
            // remote default branch, what's uncommitted. Asked now: the last scan may be behind.
            let changed = match git::own_paths(&root).await {
                Some(paths) => paths,
                None => git::files(&root)
                    .await
                    .map(|files| files.changed)
                    .unwrap_or_default(),
            };
            // Not before its room is joined, as just after the team added it.
            let joined = |area: &str| {
                self.repos.get(&root).is_some_and(|watched| {
                    self.joined
                        .contains(&watched.project.topic_channel(area, false))
                })
            };
            if let Some(area) = team.area_of(changed.iter().map(String::as_str))
                && joined(area)
            {
                return Ok(Target::Topic {
                    name: area.to_owned(),
                    org: false,
                });
            }
        }
        Ok(match team.room() {
            Room::Repo => Target::Repo,
            Room::Topic(name) => Target::Topic { name, org: false },
        })
    }

    /// Refuses `text` from `session`, sent to the repository's channel (`--to repo`), when
    /// the session is an agent's and the post isn't for a person: that channel is its
    /// people's ([`REPO_CHANNEL_REFUSED`]). A person at a terminal says what they like.
    pub(super) fn ensure_for_people(&self, session: &str, text: &str) -> Result<()> {
        let (people, agents) = self.known_names();
        let people: Vec<&str> = people.iter().map(String::as_str).collect();
        let agents: Vec<&str> = agents.iter().map(String::as_str).collect();
        anyhow::ensure!(
            team::may_post_to_repo(session, text, &people, &agents),
            "{REPO_CHANNEL_REFUSED}"
        );
        Ok(())
    }

    /// Whether `channel` is one of a watched repository's rooms: its agent room or an
    /// area's.
    pub(super) fn is_post_room(&self, channel: &str) -> bool {
        self.repos
            .values()
            .any(|watched| watched.rooms().any(|room| room == channel))
    }

    /// Whether `channel` is a watched repository's area's room.
    pub(super) fn is_area_room(&self, channel: &str) -> bool {
        self.repos
            .values()
            .any(|watched| watched.areas.iter().any(|area| area == channel))
    }

    /// The channel whose lines `channel`'s count as: a repository's for one of its rooms,
    /// else `channel` itself.
    pub(super) fn home_channel(&self, channel: &str) -> String {
        self.repos
            .values()
            .find(|watched| watched.rooms().any(|room| room == channel))
            .map_or_else(|| channel.to_owned(), |watched| watched.channel.clone())
    }

    /// The sessions a note can be for: this machine's own, and those its channels' working
    /// sets and claims name, with the machines they work on.
    pub(super) fn known_sessions(&self) -> Vec<notes::Known> {
        let mut known = Vec::new();
        if let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) {
            known.extend(
                self.sessions_here()
                    .into_iter()
                    .map(|session| notes::Known {
                        machine: me.clone(),
                        session: session.to_owned(),
                        name: self.name_of(session),
                    }),
            );
        }
        for channel in &self.joined {
            for member in self.presence.members(channel) {
                if let Some(session) = member.session {
                    known.push(notes::Known {
                        machine: member.nick,
                        session,
                        name: member.session_name,
                    });
                }
            }
            for claim in self.presence.claims(channel) {
                known.push(notes::Known {
                    machine: claim.nick,
                    session: claim.session,
                    name: claim.session_name,
                });
            }
        }
        known
    }

    /// The sessions working on this machine: those in its watched repositories, and those
    /// watching.
    pub(super) fn sessions_here(&self) -> BTreeSet<&str> {
        self.repos
            .values()
            .flat_map(|watched| watched.sessions.names())
            .chain(self.watchers.iter().filter_map(|w| w.session.as_deref()))
            .collect()
    }

    /// The session working on this machine that `name` names, by its id or the name it goes
    /// by, if one does: of two by one name, the one working here now.
    pub(super) fn local_session(&self, name: &str) -> Option<String> {
        if let Some(session) = self.session_here(name) {
            return Some(session);
        }
        let here = self.sessions_here();
        let named = self.book.named();
        let named: Vec<&str> = named
            .iter()
            .filter(|(_, it)| it.eq_ignore_ascii_case(name))
            .map(|(session, _)| session.as_str())
            .collect();
        let working: Vec<&str> = named
            .iter()
            .copied()
            .filter(|session| here.contains(session))
            .collect();
        match (working.as_slice(), named.as_slice()) {
            ([only], _) | ([], [only]) => Some((*only).to_owned()),
            _ => None,
        }
    }

    /// Who a post can be for: people (this machine's owner, and people's accounts and nicks
    /// heard), and agents (this machine's sessions and their names, machines, and the
    /// sessions heard in its channels).
    pub(super) fn known_names(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut people: BTreeSet<String> = self.owner.iter().cloned().collect();
        let mut agents: BTreeSet<String> = self
            .book
            .named()
            .into_iter()
            .flat_map(|(session, name)| [session, name])
            .collect();
        agents.extend(
            self.repos
                .values()
                .flat_map(|watched| watched.sessions.names().map(str::to_owned)),
        );
        if let Some(client) = &self.client {
            agents.insert(client.nick().to_owned());
        }
        for (nick, account) in &self.accounts {
            let them = if self.machines.contains_key(nick) {
                &mut agents
            } else {
                &mut people
            };
            them.insert(nick.clone());
            them.insert(account.clone());
        }
        for channel in &self.joined {
            for member in self.presence.members(channel) {
                agents.insert(member.nick);
                agents.extend(member.session);
                agents.extend(member.session_name);
            }
            for claim in self.presence.claims(channel) {
                agents.insert(claim.session);
                agents.extend(claim.session_name);
            }
        }
        // A machine's nick is never a person's.
        people.retain(|name| !agents.contains(name));
        (people, agents)
    }
}
