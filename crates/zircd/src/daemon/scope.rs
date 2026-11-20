//! Where a repository's lines go, and which repository a line heard is about. Every
//! repository's sessions speak in the organisation's one [`AGENTS_CHANNEL`], `#agents`: a
//! plain `zirc post`, commits and pushes, intents, working sets, `taking:`/`done:`/`dropped:`
//! and `@everyone`, each line saying its repository ([`Message::repo`]). Inside, zircd keys
//! all it knows of a repository by its scope ([`Project::scope`], `#<repo>`), as it once keyed
//! it by the repository's channel of that name: what's sent to a scope goes out in `#agents`
//! ([`Daemon::on_wire`]), and what's heard there counts as its repository's
//! ([`zirc_core::ipc::Delivery::scope`]). A plain post addressed to one agent session (`coordinator: landed
//! it`) is a note for it alone ([`zirc_core::notes`]), in no room at all.
//!
//! Nothing said in `#agents` wakes a session unless it names it, or `@everyone`
//! ([`Daemon::is_post_room`]).
//!
//! [`Project::scope`]: zirc_core::project::Project::scope

use std::collections::BTreeSet;

use zirc_core::notes;
use zirc_proto::{AGENTS_CHANNEL, Message};

use super::Daemon;

impl Daemon {
    /// Where `message`, sent to `channel`, goes on the wire: a watched repository's scope
    /// goes out in [`AGENTS_CHANNEL`], `message` saying its repository; anything else, as it
    /// is.
    pub(super) fn on_wire(&self, channel: &str, message: &mut Message) -> String {
        match self.repos.values().find(|watched| watched.scope == channel) {
            Some(watched) => {
                message.repo = Some(watched.project.repo.clone());
                AGENTS_CHANNEL.to_owned()
            }
            None => channel.to_owned(),
        }
    }

    /// The channel `channel`, a scope or a channel, is on the wire, as an answer names it:
    /// `#agents` for a repository's scope.
    pub(super) fn wire_name(&self, channel: &str) -> String {
        if self.is_scope(channel) {
            AGENTS_CHANNEL.to_owned()
        } else {
            channel.to_owned()
        }
    }

    /// `scope` is no longer watched here, as its last checkout is unwatched or gone: it's out
    /// of `joined`, which holds a watched repository's scope while `#agents` is joined
    /// ([`Daemon::someone_joined`]).
    pub(super) fn left_scope(&mut self, scope: &str) {
        if !self.is_scope(scope) {
            self.joined.remove(scope);
        }
    }

    /// Whether `channel` is one of the watched repositories' scopes, which zircd keys them
    /// by, rather than a channel it's in on the server.
    pub(super) fn is_scope(&self, channel: &str) -> bool {
        self.repos.values().any(|watched| watched.scope == channel)
    }

    /// The scopes of the watched repositories, each once.
    pub(super) fn scopes(&self) -> BTreeSet<String> {
        self.repos
            .values()
            .map(|watched| watched.scope.clone())
            .collect()
    }

    /// Whether `channel` is where agents say what they're doing, [`AGENTS_CHANNEL`], whose
    /// lines are news only to whom they name.
    pub(super) fn is_post_room(channel: &str) -> bool {
        channel.eq_ignore_ascii_case(AGENTS_CHANNEL)
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
