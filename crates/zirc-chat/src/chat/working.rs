//! Who in a channel is changing what, a set of paths per branch, and what sessions said
//! they're about to change (`zirc intend`) and whose work each runs into. What a script
//! reads of a channel's presence as text; the apps' cards and rows say it their own way
//! ([`super::AgentCard`], [`super::BothChanging`]).

use std::collections::{BTreeMap, BTreeSet};

use super::{Chat, Person};
use crate::presence;

/// Someone in a channel, or still announcing work in it, and what they're changing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingPerson {
    /// Who they are, as [`Chat::people`] says; made from what's known of them when they
    /// aren't here, never an operator.
    pub person: Person,
    /// Whether they're in the channel now.
    pub here: bool,
    /// What they're changing, a set per branch.
    pub sets: Vec<WorkingSet>,
}

impl WorkingPerson {
    /// How many paths they're changing, on every branch.
    #[must_use]
    pub fn files(&self) -> usize {
        self.sets.iter().map(|set| set.paths.len()).sum()
    }
}

/// The paths someone has uncommitted changes in on one branch, or on their whole machine
/// when its zircd doesn't say which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingSet {
    /// The branch, when known.
    pub branch: Option<String>,
    /// The session working there, when known.
    pub session: Option<String>,
    /// The paths, sorted, each with whether someone else is changing it too.
    pub paths: Vec<(String, bool)>,
}

/// What a session said it's about to change, and whose work it runs into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intending {
    /// The machine's nick.
    pub nick: String,
    /// The session that said so.
    pub session: String,
    /// What it's about to do.
    pub summary: String,
    /// The paths or directories it expects to touch, each with whether someone else is
    /// changing it, or about to.
    pub paths: Vec<(String, bool)>,
    /// Who else is changing those paths or about to, sorted.
    pub with: Vec<String>,
    /// When it ends on its own, in seconds since the Unix epoch.
    pub expires: Option<u64>,
}

impl Chat {
    /// Who is in `channel` and what they're changing, then anyone still announcing work
    /// there who isn't: those changing something first.
    #[must_use]
    pub fn working_people(&self, channel: &str) -> Vec<WorkingPerson> {
        let shared: BTreeSet<String> = self
            .both_changing(channel)
            .into_iter()
            .map(|both| both.path)
            .collect();
        let mut working: BTreeMap<String, Vec<WorkingSet>> = BTreeMap::new();
        for member in self.presence.members(channel) {
            working.entry(member.nick).or_default().push(WorkingSet {
                branch: member.branch,
                session: member.session,
                paths: member
                    .paths
                    .into_iter()
                    .map(|path| {
                        let both = shared.contains(&path);
                        (path, both)
                    })
                    .collect(),
            });
        }
        let mut people: Vec<WorkingPerson> = self
            .people(channel)
            .into_iter()
            .map(|person| WorkingPerson {
                here: true,
                sets: working.remove(&person.nick).unwrap_or_default(),
                person,
            })
            .collect();
        people.extend(working.into_iter().map(|(nick, sets)| WorkingPerson {
            person: Person {
                name: self.shown_name(&nick),
                me: nick.eq_ignore_ascii_case(&self.me),
                machine: self.machine(&nick),
                op: false,
                away: self.away(&nick).map(str::to_owned),
                app: self.app_of(&nick),
                nick,
            },
            here: false,
            sets,
        }));
        people.sort_by_key(|person| person.sets.is_empty());
        people
    }

    /// What sessions in `channel` are about to change, as they said with `zirc intend`, and
    /// whom each runs into: someone else changing those paths, or about to.
    #[must_use]
    pub fn intending(&self, channel: &str) -> Vec<Intending> {
        let now = u64::try_from((self.clock)().timestamp()).unwrap_or_default();
        let intents = self.presence.intents(channel, now);
        let members = self.presence.members(channel);
        intents
            .iter()
            .map(|intent| {
                let mut with = BTreeSet::new();
                // Anyone's work but this session's own. Another worktree on the same
                // machine counts when its zircd says which, by its branch: a machine-wide
                // set may be this session's own.
                let own = |nick: &str, session: Option<&str>| {
                    nick == intent.nick && session == Some(intent.session.as_str())
                };
                let paths = intent
                    .paths
                    .iter()
                    .map(|path| {
                        let mut hit = false;
                        for member in &members {
                            let mine = member.nick == intent.nick
                                && (member.branch.is_none()
                                    || own(&member.nick, member.session.as_deref()));
                            if !mine
                                && !presence::touching(&member.paths, &[path.as_str()]).is_empty()
                            {
                                with.insert(member.nick.clone());
                                hit = true;
                            }
                        }
                        for other in &intents {
                            if !own(&other.nick, Some(other.session.as_str()))
                                && !presence::touching(&other.paths, &[path.as_str()]).is_empty()
                            {
                                with.insert(other.nick.clone());
                                hit = true;
                            }
                        }
                        (path.clone(), hit)
                    })
                    .collect();
                Intending {
                    nick: intent.nick.clone(),
                    session: intent.session.clone(),
                    summary: intent.summary.clone(),
                    paths,
                    with: with.into_iter().collect(),
                    expires: intent.expires,
                }
            })
            .collect()
    }
}
