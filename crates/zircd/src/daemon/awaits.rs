//! What a session waits on: another session's landing, a build, or a branch. Other sessions'
//! landing lines are routine, news only to whom they name, so a session blocked on another's
//! landing never heard it land: six once sat "waiting for build 1299" for 40 minutes after it
//! had. A session says what it waits on with `zirc wait-for`, or in a post or intent that
//! says it's waiting for or blocked on a build or a session, and CI's landing line that names
//! it wakes the session ([`Why::Awaited`]). A wait ends with the line saying it landed, failed
//! or conflicts, or after [`LASTS`].
//!
//! zircd also waits for a session, by itself, on another's branch it told the session was
//! changing the session's files: an overlap, a push or a commit ([`Awaits::follow`]). On
//! 9 Nov 2026 a session missed a branch it had been told of landing, as nobody remembered
//! `zirc wait-for`. That wait is quiet: only CI's line saying the branch landed wakes the
//! session ([`Why::BranchLanded`]), once; a failure or a conflict is the branch's own
//! session's news, and it lands later. It ends when the branch lands, when this machine had
//! the branch and has it no more, or after [`FOLLOWS`].
//!
//! A session stacked on another's branch (`zirc stack`), building on it before it lands,
//! waits on it the same way, and on it moving: CI's line saying it landed wakes the session
//! ([`Why::BranchLanded`]), and so does a push that moves its head from the one the session
//! built on ([`Why::Landing`]). A landing rebases a branch, so the stack keeps that head,
//! for the session's rebase onto the default branch, until the session drops it or after
//! [`STACKED`]; a stacked branch's being deleted doesn't end it, as it's deleted once it lands.
//!
//! A session also waits on a state changing (`zirc wait-for --state master`), as one ready
//! to land waits for a broken master: the first state said of it at a level other than the
//! one it stood at as the session asked wakes the session ([`Why::StateChanged`]), once, and
//! ends the wait. Sessions polled `zirc state master --check` in a loop before.
//!
//! A session also waits for a reaction (`zirc wait-for --reaction 👀`): the machine's person
//! giving it to a message, any message they can see, wakes the session
//! ([`Why::ReactionAwaited`]), with what the message is, so that its hint fits: an issue's
//! card, a person's words or an agent's post. Once a message: the same emoji given to it again,
//! taken back and given again, or given from another of the person's apps, wakes it no more.
//! Only the person's own account counts, never a nick. The wait lasts [`REACTS`] from when
//! it was said or last woke the session, and none older than the wait is told.
//!
//! Waits are saved in the organisation's state, so a restart, as an update makes, doesn't
//! lose them: on 9 Nov 2026 two restarts in an hour each dropped a session's wait.
//!
//! [`Why::Awaited`]: zirc_core::ipc::Why::Awaited
//! [`Why::BranchLanded`]: zirc_core::ipc::Why::BranchLanded
//! [`Why::StateChanged`]: zirc_core::ipc::Why::StateChanged
//! [`Why::ReactionAwaited`]: zirc_core::ipc::Why::ReactionAwaited

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zirc_core::ipc::{Reaction, Response, StateWait, Why};
use zirc_core::paths;
use zirc_proto::Event;
use zirc_proto::state::{Level, State};

use super::{Daemon, now, read_saved};

/// How long a wait lasts unless what it waits on lands, fails or conflicts first.
pub(super) const LASTS: Duration = Duration::from_hours(3);

/// How long zircd waits on an overlapping branch for a session, from when it last heard of
/// it, unless it lands or is deleted first: a branch can take a day to land.
pub(super) const FOLLOWS: Duration = Duration::from_hours(24);

/// How long zircd keeps a session's stack on another's branch, from when it was said or the
/// branch last moved: for the branch to land, and the session to rebase once it has.
pub(super) const STACKED: Duration = Duration::from_hours(72);

/// How long a wait for a reaction lasts, from when it was said or last woke the session.
pub(super) const REACTS: Duration = Duration::from_hours(24);

/// How many messages a wait for a reaction remembers waking the session for.
const WOKEN: usize = 200;

/// The words in a post or an intent that say a session is waiting on what it names.
const WAITING: [&str; 8] = [
    "waiting for",
    "waiting on",
    "wait for",
    "waits for",
    "waits on",
    "blocked on",
    "blocked by",
    "until build",
];

/// One thing a session waits on.
#[derive(Debug, Serialize, Deserialize)]
struct Wait {
    /// A session, a branch, or a build by its number.
    on: String,
    /// When it was said, or for a followed branch, last heard of, in seconds since the
    /// Unix epoch.
    since: u64,
    /// Whether zircd follows it for the session ([`Awaits::follow`]), rather than the
    /// session having said it waits on it.
    #[serde(default)]
    followed: bool,
    /// For a followed branch: whether this machine has had it, so that its going means it
    /// was deleted.
    #[serde(default)]
    seen: bool,
    /// For a state, `on` naming it: where, and how it stood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<Stood>,
    /// For a branch the session is stacked on: its head as the session built on it, what
    /// it's landed or moved to since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stack: Option<Stacked>,
    /// For a reaction, `on` being its emoji: the messages it woke the session for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reaction: Option<Reacting>,
}

/// A wait for a reaction, past when it started.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Reacting {
    /// When it was first said, in seconds since the Unix epoch: a reaction older than that,
    /// as history replays, wakes no one.
    started: u64,
    /// The messages it woke the session for, oldest first, each with the reaction that did:
    /// only that one wakes it, so that a reaction told again, or given again, doesn't.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    woken: Vec<(String, String)>,
}

/// A branch a session is stacked on, past the head it built on.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stacked {
    /// The branch's head as the session built on it.
    tip: String,
    /// Whether CI said it landed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    landed: bool,
    /// Its head since, when a push moved it from `tip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    moved: Option<String>,
}

/// Whether two commits, either perhaps shortened, are the same.
fn same_commit(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

/// A state waited on: the repository it's in, and how it stood as the session asked.
#[derive(Debug, Serialize, Deserialize)]
struct Stood {
    /// The repository's channel; any repository's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    /// How it stood; absent when it was never said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    level: Option<Level>,
}

impl Wait {
    /// Whether `state` is said at a level other than this wait's, of the state it waits on.
    fn changed(&self, state: &State) -> bool {
        self.state.as_ref().is_some_and(|stood| {
            self.on == state.name
                && stood.repo.as_ref().is_none_or(|repo| *repo == state.repo)
                && stood.level != Some(state.level)
        })
    }

    /// Whether it waits on a landing, rather than a state or a reaction.
    fn on_landing(&self) -> bool {
        self.state.is_none() && self.reaction.is_none()
    }

    fn live(&self) -> bool {
        let lasts = match (&self.stack, self.followed) {
            _ if self.reaction.is_some() => REACTS,
            (Some(_), _) => STACKED,
            (None, true) => FOLLOWS,
            (None, false) => LASTS,
        };
        now().saturating_sub(self.since) < lasts.as_secs()
    }
}

/// By session: what it waits on.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct Awaits(BTreeMap<String, Vec<Wait>>);

impl Awaits {
    /// Notes that `session` waits on each of `on`.
    pub(super) fn add(&mut self, session: &str, on: impl IntoIterator<Item = String>) {
        let waits = self.0.entry(session.to_owned()).or_default();
        waits.retain(Wait::live);
        for on in on {
            let on = on.trim().trim_start_matches('#').to_owned();
            if on.is_empty() {
                continue;
            }
            waits.retain(|wait| !(wait.on_landing() && wait.on == on));
            waits.push(Wait {
                on,
                since: now(),
                followed: false,
                seen: false,
                state: None,
                stack: None,
                reaction: None,
            });
        }
        if waits.is_empty() {
            self.0.remove(session);
        }
    }

    /// Notes that `session` waits on each of `states` changing.
    pub(super) fn add_states(&mut self, session: &str, states: Vec<StateWait>) {
        if states.is_empty() {
            return;
        }
        let waits = self.0.entry(session.to_owned()).or_default();
        waits.retain(Wait::live);
        for StateWait { name, repo, level } in states {
            waits.retain(|wait| wait.on_landing() || wait.on != name);
            waits.push(Wait {
                on: name,
                since: now(),
                followed: false,
                seen: false,
                state: Some(Stood { repo, level }),
                stack: None,
                reaction: None,
            });
        }
    }

    /// Notes that `session` waits for each of `emojis` ([`Why::ReactionAwaited`]); one it
    /// waits for already lasts [`REACTS`] from now.
    pub(super) fn add_reactions(&mut self, session: &str, emojis: Vec<String>) {
        if emojis.is_empty() {
            return;
        }
        let waits = self.0.entry(session.to_owned()).or_default();
        waits.retain(Wait::live);
        for emoji in emojis {
            let emoji = emoji.trim().to_owned();
            if emoji.is_empty() {
                continue;
            }
            if let Some(wait) = waits
                .iter_mut()
                .find(|wait| wait.reaction.is_some() && wait.on == emoji)
            {
                wait.since = now();
                continue;
            }
            waits.push(Wait {
                on: emoji,
                since: now(),
                followed: false,
                seen: false,
                state: None,
                stack: None,
                reaction: Some(Reacting {
                    started: now(),
                    woken: Vec::new(),
                }),
            });
        }
        if waits.is_empty() {
            self.0.remove(session);
        }
    }

    /// Ends `session`'s waits for each of `emojis`.
    pub(super) fn drop_reactions(&mut self, session: &str, emojis: &[String]) {
        if let Some(waits) = self.0.get_mut(session) {
            waits.retain(|wait| {
                !(wait.reaction.is_some() && emojis.iter().any(|emoji| emoji.trim() == wait.on))
            });
            if waits.is_empty() {
                self.0.remove(session);
            }
        }
    }

    /// Notes `reaction`, the machine's person's, for each session waiting for its emoji that
    /// it didn't wake for that message yet: from now on it wakes those
    /// ([`Awaits::reaction_wakes`]), and their waits last [`REACTS`] from now. Says whether
    /// it woke any.
    pub(super) fn reacted(&mut self, reaction: &Reaction) -> bool {
        if reaction.removed {
            return false;
        }
        let given = reaction
            .time
            .as_deref()
            .and_then(zirc_core::time::seconds_at)
            .unwrap_or_else(now);
        let by = reaction.id.clone().unwrap_or_default();
        let mut woke = false;
        for wait in self.0.values_mut().flatten() {
            if !wait.live() || wait.on != reaction.emoji {
                continue;
            }
            let Some(reacting) = &mut wait.reaction else {
                continue;
            };
            if given < reacting.started || reacting.woken.iter().any(|(to, _)| *to == reaction.to) {
                continue;
            }
            reacting.woken.push((reaction.to.clone(), by.clone()));
            if reacting.woken.len() > WOKEN {
                reacting.woken.remove(0);
            }
            wait.since = now();
            woke = true;
        }
        woke
    }

    /// Whether `reaction` is the one that woke `session` for its message
    /// ([`Awaits::reacted`]).
    pub(super) fn reaction_wakes(&self, session: &str, reaction: &Reaction) -> bool {
        let by = reaction.id.as_deref().unwrap_or_default();
        !reaction.removed
            && self.0.get(session).is_some_and(|waits| {
                waits.iter().any(|wait| {
                    wait.live()
                        && wait.on == reaction.emoji
                        && wait.reaction.as_ref().is_some_and(|reacting| {
                            reacting
                                .woken
                                .iter()
                                .any(|(to, woke)| *to == reaction.to && woke == by)
                        })
                })
            })
    }

    /// Whether `state`, just said, changes a state `session` waits on.
    pub(super) fn state_changed(&self, session: &str, state: &State) -> bool {
        self.0
            .get(session)
            .is_some_and(|waits| waits.iter().any(|wait| wait.live() && wait.changed(state)))
    }

    /// Ends the waits `state`, just said, answers. Says whether any ended.
    pub(super) fn state_heard(&mut self, state: &State) -> bool {
        let before = self.state();
        self.0.retain(|_, waits| {
            waits.retain(|wait| wait.live() && !wait.changed(state));
            !waits.is_empty()
        });
        self.state() != before
    }

    /// Waits, for `session`, on `branch` landing: another's branch zircd told it was changing
    /// its files. Once a branch, however often it's heard of; nothing when the session waits
    /// on it already, as said.
    pub(super) fn follow(&mut self, session: &str, branch: &str) -> bool {
        let waits = self.0.entry(session.to_owned()).or_default();
        waits.retain(Wait::live);
        if let Some(wait) = waits
            .iter_mut()
            .find(|wait| wait.on_landing() && wait.on == branch)
        {
            if wait.followed {
                wait.since = now();
            }
            return false;
        }
        waits.push(Wait {
            on: branch.to_owned(),
            since: now(),
            followed: true,
            seen: false,
            state: None,
            stack: None,
            reaction: None,
        });
        true
    }

    /// Notes that `session` is stacked on `branch`, whose head was `tip` as it built on it,
    /// in place of any wait on it it had.
    pub(super) fn stack(&mut self, session: &str, branch: &str, tip: &str) {
        let waits = self.0.entry(session.to_owned()).or_default();
        waits.retain(|wait| wait.live() && !(wait.on_landing() && wait.on == branch));
        waits.push(Wait {
            on: branch.to_owned(),
            since: now(),
            followed: true,
            seen: false,
            state: None,
            stack: Some(Stacked {
                tip: tip.to_owned(),
                landed: false,
                moved: None,
            }),
            reaction: None,
        });
    }

    /// Forgets that `session` is stacked on `branch`.
    pub(super) fn unstack(&mut self, session: &str, branch: &str) {
        if let Some(waits) = self.0.get_mut(session) {
            waits.retain(|wait| !(wait.stack.is_some() && wait.on == branch));
            if waits.is_empty() {
                self.0.remove(session);
            }
        }
    }

    /// The branches `session` is stacked on, oldest first.
    pub(super) fn stacks(&self, session: &str) -> Vec<zirc_core::ipc::Stack> {
        self.0.get(session).map_or_else(Vec::new, |waits| {
            waits
                .iter()
                .filter(|wait| wait.live())
                .filter_map(|wait| {
                    let stack = wait.stack.as_ref()?;
                    Some(zirc_core::ipc::Stack {
                        branch: wait.on.clone(),
                        tip: stack.tip.clone(),
                        landed: stack.landed,
                        moved: stack.moved.clone(),
                    })
                })
                .collect()
        })
    }

    /// Whether a push of `branch` to `head` moves a branch `session` is stacked on, not
    /// landed yet, from the head it built on and from where it was last heard moving.
    pub(super) fn moves_stack(&self, session: &str, branch: &str, head: &str) -> bool {
        self.0.get(session).is_some_and(|waits| {
            waits
                .iter()
                .filter(|wait| wait.live() && wait.on == branch)
                .any(|wait| {
                    wait.stack.as_ref().is_some_and(|stack| {
                        !stack.landed
                            && !same_commit(&stack.tip, head)
                            && !stack
                                .moved
                                .as_deref()
                                .is_some_and(|moved| same_commit(moved, head))
                    })
                })
        })
    }

    /// Notes that `branch` was pushed to `head`, for the sessions stacked on it. Says
    /// whether that changed anything.
    pub(super) fn pushed(&mut self, branch: &str, head: &str) -> bool {
        let mut changed = false;
        for wait in self.0.values_mut().flatten() {
            if wait.on != branch {
                continue;
            }
            if let Some(stack) = &mut wait.stack
                && !stack.landed
                && !same_commit(&stack.tip, head)
                && !stack
                    .moved
                    .as_deref()
                    .is_some_and(|moved| same_commit(moved, head))
            {
                stack.moved = Some(head.to_owned());
                wait.since = now();
                changed = true;
            }
        }
        changed
    }

    /// Each session waiting on something, and what.
    pub(super) fn waiting(&self) -> Vec<(String, Vec<String>)> {
        self.0
            .keys()
            .map(|session| (session.clone(), self.of(session)))
            .filter(|(_, on)| !on.is_empty())
            .collect()
    }

    /// What `session` waits on, for `zirc wait-for` to say.
    pub(super) fn of(&self, session: &str) -> Vec<String> {
        self.0.get(session).map_or_else(Vec::new, |waits| {
            waits
                .iter()
                .filter(|wait| wait.live())
                .map(|wait| match (&wait.state, &wait.reaction) {
                    (Some(_), _) => format!("state {}", wait.on),
                    (None, Some(_)) => format!("reaction {}", wait.on),
                    (None, None) => wait.on.clone(),
                })
                .collect()
        })
    }

    /// Why `text`, a landing line from CI about `branch` if its data says, concerns
    /// `session`, if it names something the session waits on: any line on what it said it
    /// waits on, and only one saying it landed on a branch zircd follows for it.
    pub(super) fn concerns(&self, session: &str, text: &str, branch: Option<&str>) -> Option<Why> {
        let waits = self.0.get(session)?;
        let live = || waits.iter().filter(|wait| wait.live() && wait.on_landing());
        if live().any(|wait| !wait.followed && names(text, &wait.on)) {
            return Some(Why::Awaited);
        }
        live()
            .filter(|wait| !wait.stack.as_ref().is_some_and(|stack| stack.landed))
            .any(|wait| wait.followed && landed(text, branch, &wait.on))
            .then_some(Why::BranchLanded)
    }

    /// Ends the waits `text`, a landing line about `branch` if its data says, answers: one
    /// saying it landed, failed or conflicts, and of the branches followed, landed. Says
    /// whether any ended.
    pub(super) fn heard(&mut self, text: &str, branch: Option<&str>) -> bool {
        let before = self.state();
        let finished = finished(text);
        let mut marked = false;
        self.0.retain(|_, waits| {
            waits.retain_mut(|wait| {
                if !wait.live() {
                    return false;
                }
                if let Some(stack) = &mut wait.stack {
                    if !stack.landed && landed(text, branch, &wait.on) {
                        stack.landed = true;
                        marked = true;
                    }
                    return true;
                }
                if wait.followed {
                    !landed(text, branch, &wait.on)
                } else {
                    !(finished && wait.on_landing() && names(text, &wait.on))
                }
            });
            !waits.is_empty()
        });
        marked || self.state() != before
    }

    /// Ends the waits on followed branches this machine had and, as `has` says, has no more:
    /// deleted, as a branch dropped is. Says whether that changed anything.
    pub(super) fn branches_now(&mut self, has: impl Fn(&str) -> bool) -> bool {
        let before = self.state();
        self.0.retain(|_, waits| {
            waits.retain_mut(|wait| {
                if !wait.followed || wait.stack.is_some() {
                    return true;
                }
                if has(&wait.on) {
                    wait.seen = true;
                    return true;
                }
                !wait.seen
            });
            !waits.is_empty()
        });
        self.state() != before
    }

    /// How many waits there are, and how many of them on branches seen here: what saving
    /// them would change.
    fn state(&self) -> (usize, usize) {
        let waits = || self.0.values().flatten();
        (waits().count(), waits().filter(|wait| wait.seen).count())
    }
}

impl Daemon {
    /// Notes what `text`, a post or intent of `session`'s, says it waits on, if anything
    /// ([`named`]): builds, and the sessions this machine knows of.
    pub(super) fn awaits_said(&mut self, session: &str, text: &str) {
        let (_, agents) = self.known_names();
        let agents: Vec<&str> = agents.iter().map(String::as_str).collect();
        let on = named(text, &agents);
        if !on.is_empty() {
            tracing::info!("{session} waits on {}", on.join(", "));
            self.awaits.add(session, on);
            self.save_awaits();
        }
    }

    /// Takes in the waits saved before zircd last stopped.
    pub(super) fn restore_awaits(&mut self) {
        if let Some(awaits) = read_saved::<Awaits>(&paths::awaits(&self.org.state)) {
            self.awaits = awaits;
        }
    }

    /// Saves what sessions wait on, for after a restart.
    pub(super) fn save_awaits(&self) {
        let json = serde_json::to_string(&self.awaits).expect("waits serialize");
        if let Err(err) = paths::write_private(&paths::awaits(&self.org.state), &json) {
            tracing::warn!("couldn't save the waits: {err}");
        }
    }

    /// Follows, for each session `reasons` says `response` concerns, the branch it says
    /// changes the session's files, if it names one: another's push or commit, a conflict
    /// ahead, or an overlap between worktrees here.
    pub(super) fn follow_branches(&mut self, response: &Response, reasons: &[Option<Why>]) {
        let mut follow = Vec::new();
        for (watcher, why) in self.watchers.iter().zip(reasons) {
            let (Some(session), Some(why)) = (watcher.session.as_deref(), why) else {
                continue;
            };
            let branch = match (response, why) {
                (Response::Delivery(delivery), Why::Overlap | Why::Paths | Why::Landing) => {
                    match &delivery.message.event {
                        Event::Push { branch, .. } | Event::Commit { branch, .. } => {
                            Some(branch.clone())
                        }
                        _ => None,
                    }
                }
                (Response::Conflict(conflict), Why::Conflict) => Some(conflict.branch.clone()),
                (Response::LocalOverlap(overlap), Why::Overlap) => overlap
                    .worktrees
                    .iter()
                    .find(|worktree| !self.works_in(worktree, session))
                    .and_then(|worktree| worktree.branch.clone()),
                _ => None,
            };
            if let Some(branch) = branch {
                follow.push((session.to_owned(), branch));
            }
        }
        for (session, branch) in follow {
            self.follow_branch(&session, &branch);
        }
    }

    /// Follows `branch` for `session`, told it changes its files, unless it's the default
    /// branch or the session's own: those it updates from as they move.
    fn follow_branch(&mut self, session: &str, branch: &str) {
        let default_or_own = self.repos.values().any(|watched| {
            watched.default_branch == branch
                || (watched.is_of(session) && watched.snapshot.branch.as_deref() == Some(branch))
        });
        if default_or_own {
            return;
        }
        if self.awaits.follow(session, branch) {
            tracing::info!("{session} waits on {branch} landing, as it overlaps");
        }
        self.save_awaits();
    }

    /// Ends the waits on followed branches deleted here.
    pub(super) fn followed_branches_now(&mut self) {
        let repos = &self.repos;
        let changed = self.awaits.branches_now(|branch| {
            repos.values().any(|watched| {
                watched.snapshot.branches.contains(branch)
                    || watched
                        .snapshot
                        .remotes
                        .keys()
                        .any(|name| name.split_once('/').is_some_and(|(_, it)| it == branch))
            })
        });
        if changed {
            self.save_awaits();
        }
    }
}

/// Whether a landing line says how it ended, rather than that it's under way.
fn finished(text: &str) -> bool {
    text.contains(" landed ") || text.contains("failed") || text.contains("conflicts with")
}

/// Whether `text`, a landing line about `branch` if its data says, says `on` landed.
fn landed(text: &str, branch: Option<&str>, on: &str) -> bool {
    text.contains(" landed ") && branch.map_or_else(|| names(text, on), |branch| branch == on)
}

/// Whether `text` names `on`: a build by its number (`build 1299`), or a session or branch
/// by its name.
fn names(text: &str, on: &str) -> bool {
    if on.bytes().all(|b| b.is_ascii_digit()) {
        let build = format!("build {on}");
        return text.match_indices(&build).any(|(at, _)| {
            !text[at + build.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        });
    }
    zirc_core::names::mentions(text, on)
}

/// What `text`, a session's post or intent, says it waits on, if it says it's waiting: the
/// builds it names (`build 1299`) and which of `sessions` it names.
pub(super) fn named(text: &str, sessions: &[&str]) -> Vec<String> {
    let lower = text.to_lowercase();
    if !WAITING.iter().any(|words| lower.contains(words)) {
        return Vec::new();
    }
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|word| !word.is_empty())
        .collect();
    let builds = words
        .windows(2)
        .filter(|pair| pair[0] == "build" && pair[1].bytes().all(|b| b.is_ascii_digit()))
        .map(|pair| pair[1].to_owned());
    let named = sessions
        .iter()
        .filter(|session| zirc_core::names::mentions(text, session))
        .map(|session| (*session).to_owned());
    builds.chain(named).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_post_says_it_waits_on() {
        let sessions = ["cc-598aa1", "cc-ad4f7c"];
        assert_eq!(
            named("waiting for build 1299 (cc-598aa1's) to land", &sessions),
            ["1299", "cc-598aa1"]
        );
        assert_eq!(
            named("Blocked on cc-ad4f7c's landing", &sessions),
            ["cc-ad4f7c"]
        );
        // Naming them isn't waiting on them.
        assert!(named("build 1299 landed, cc-598aa1's", &sessions).is_empty());
    }

    #[test]
    fn a_landing_line_naming_a_wait_concerns_it_until_it_ends() {
        let mut awaits = Awaits::default();
        awaits.add("cc-1", ["1299".to_owned(), "cc-2".to_owned()]);
        assert_eq!(awaits.of("cc-1"), ["1299", "cc-2"]);
        let deploying = "cc-3: deploying to production: web (build 1299)";
        let awaited = Some(Why::Awaited);
        assert_eq!(awaits.concerns("cc-1", deploying, None), awaited);
        let other = "cc-3: landed x as abc (build 12990)";
        assert_eq!(awaits.concerns("cc-1", other, None), None);
        assert_eq!(awaits.concerns("cc-9", deploying, None), None);
        // Under way: still waiting.
        awaits.heard(deploying, None);
        assert_eq!(awaits.concerns("cc-1", deploying, None), awaited);
        // Landed: that wait's over, the other isn't.
        let landed = "cc-3: landed fix-login as 1a2b3c4 (build 1299)";
        assert_eq!(awaits.concerns("cc-1", landed, None), awaited);
        awaits.heard(landed, None);
        assert_eq!(awaits.concerns("cc-1", landed, None), None);
        let failed = "cc-2: build 1301 failed: //web:e2e";
        assert_eq!(awaits.concerns("cc-1", failed, None), awaited);
    }

    #[test]
    fn a_followed_branch_concerns_only_when_it_lands_and_then_ends() {
        let mut awaits = Awaits::default();
        assert!(awaits.follow("cc-1", "fix-login"));
        // Heard of again, it's still the one wait.
        assert!(!awaits.follow("cc-1", "fix-login"));
        assert_eq!(awaits.of("cc-1"), ["fix-login"]);
        let branch = Some("fix-login");
        for quiet in [
            "cc-2: build 1300 of fix-login failed in test: //api:unit",
            "cc-2: fix-login conflicts with master: rebase, resolve, land again",
        ] {
            assert_eq!(awaits.concerns("cc-1", quiet, branch), None, "{quiet}");
            awaits.heard(quiet, branch);
        }
        let other = "cc-3: landed fix-login-2 as 5d6e7f8 (build 1301)";
        assert_eq!(awaits.concerns("cc-1", other, Some("fix-login-2")), None);
        let landed = "cc-2: landed fix-login as 1a2b3c4 (build 1302)";
        assert_eq!(
            awaits.concerns("cc-1", landed, branch),
            Some(Why::BranchLanded)
        );
        // Without the line's data, by its words.
        assert_eq!(
            awaits.concerns("cc-1", landed, None),
            Some(Why::BranchLanded)
        );
        awaits.heard(landed, branch);
        assert_eq!(awaits.concerns("cc-1", landed, branch), None);
        assert!(awaits.of("cc-1").is_empty());
    }

    #[test]
    fn a_stack_wakes_on_its_branch_landing_or_moving_and_outlasts_both() {
        let mut awaits = Awaits::default();
        awaits.follow("cc-1", "api-auth");
        awaits.stack("cc-1", "api-auth", "aaaa111");
        let stacks = awaits.stacks("cc-1");
        assert_eq!(stacks.len(), 1, "the stack replaces the follow");
        assert_eq!(stacks[0].tip, "aaaa111");
        // Its own head again, shortened or not, isn't a move; another is, once.
        assert!(!awaits.moves_stack("cc-1", "api-auth", "aaaa1112222"));
        assert!(awaits.moves_stack("cc-1", "api-auth", "bbbb222"));
        assert!(!awaits.moves_stack("cc-2", "api-auth", "bbbb222"));
        assert!(awaits.pushed("api-auth", "bbbb222"));
        assert!(!awaits.pushed("api-auth", "bbbb222"));
        assert!(!awaits.moves_stack("cc-1", "api-auth", "bbbb222"));
        assert_eq!(awaits.stacks("cc-1")[0].moved.as_deref(), Some("bbbb222"));
        // Landing wakes it once, and keeps the head built on for the rebase.
        let landed = "cc-2: landed api-auth as 1a2b3c4 (build 1302)";
        let branch = Some("api-auth");
        assert_eq!(
            awaits.concerns("cc-1", landed, branch),
            Some(Why::BranchLanded)
        );
        assert!(awaits.heard(landed, branch));
        assert_eq!(awaits.concerns("cc-1", landed, branch), None);
        assert!(awaits.stacks("cc-1")[0].landed);
        // Deleted once landed: still there, until the session drops it.
        awaits.branches_now(|_| false);
        let saved = serde_json::to_string(&awaits).unwrap();
        let mut read: Awaits = serde_json::from_str(&saved).unwrap();
        assert_eq!(read.stacks("cc-1")[0].tip, "aaaa111");
        assert!(!read.moves_stack("cc-1", "api-auth", "cccc333"), "landed");
        read.unstack("cc-1", "api-auth");
        assert!(read.stacks("cc-1").is_empty());
    }

    #[test]
    fn a_branch_said_waited_on_isnt_followed_too() {
        let mut awaits = Awaits::default();
        awaits.add("cc-1", ["fix-login".to_owned()]);
        assert!(!awaits.follow("cc-1", "fix-login"));
        let failed = "cc-2: build 1300 of fix-login failed in test: //api:unit";
        assert_eq!(
            awaits.concerns("cc-1", failed, Some("fix-login")),
            Some(Why::Awaited)
        );
    }

    #[test]
    fn waits_read_back_as_they_were_saved() {
        let mut awaits = Awaits::default();
        awaits.add("cc-1", ["1299".to_owned()]);
        awaits.follow("cc-1", "fix-login");
        assert!(awaits.branches_now(|branch| branch == "fix-login"));
        let saved = serde_json::to_string(&awaits).unwrap();
        let mut read: Awaits = serde_json::from_str(&saved).unwrap();
        assert_eq!(read.of("cc-1"), ["1299", "fix-login"]);
        // Still followed, and still known to have been here.
        let landed = "cc-2: landed fix-login as 1a2b3c4 (build 1300)";
        assert_eq!(read.concerns("cc-1", landed, None), Some(Why::BranchLanded));
        assert!(read.branches_now(|_| false));
        assert_eq!(read.of("cc-1"), ["1299"]);
        // Nothing to change, nothing to save.
        assert!(!read.branches_now(|_| false));
        assert!(!read.heard("cc-3: landed other as 5d6e7f8 (build 1301)", None));
    }

    #[test]
    fn a_state_waited_on_concerns_once_it_changes_level_and_then_ends() {
        let said = |level| State {
            repo: "#monorepo".into(),
            name: "master".into(),
            level,
            why: None,
            owner: None,
            link: None,
        };
        let mut awaits = Awaits::default();
        awaits.add_states(
            "cc-1",
            vec![StateWait {
                name: "master".into(),
                repo: Some("#monorepo".into()),
                level: Some(Level::Broken),
            }],
        );
        // A landing on a branch named master is no state.
        awaits.add("cc-1", ["1299".to_owned()]);
        assert_eq!(awaits.of("cc-1"), ["state master", "1299"]);
        let landing = "cc-2: master conflicts with master (build 1299)";
        assert_eq!(awaits.concerns("cc-1", landing, None), Some(Why::Awaited));
        // Broken again, by another build: no change.
        assert!(!awaits.state_changed("cc-1", &said(Level::Broken)));
        assert!(!awaits.state_heard(&said(Level::Broken)));
        // Another repository's master, or another session: not this wait.
        let elsewhere = State {
            repo: "#other".into(),
            ..said(Level::Ok)
        };
        assert!(
            !awaits.state_changed("cc-1", &elsewhere)
                && !awaits.state_changed("cc-9", &said(Level::Ok))
        );
        // Fine again: it wakes, once.
        assert!(awaits.state_changed("cc-1", &said(Level::Ok)));
        assert!(awaits.state_heard(&said(Level::Ok)));
        assert!(!awaits.state_changed("cc-1", &said(Level::Broken)));
        assert_eq!(awaits.of("cc-1"), ["1299"]);
        // Never said: the first said wakes it, and it reads back as saved.
        awaits.add_states(
            "cc-1",
            vec![StateWait {
                name: "staging".into(),
                repo: None,
                level: None,
            }],
        );
        let read: Awaits = serde_json::from_str(&serde_json::to_string(&awaits).unwrap()).unwrap();
        let staging = State {
            name: "staging".into(),
            ..said(Level::Warning)
        };
        assert!(read.state_changed("cc-1", &staging));
    }

    #[test]
    fn a_followed_branch_deleted_here_is_waited_on_no_more() {
        let mut awaits = Awaits::default();
        awaits.follow("cc-1", "elsewhere");
        awaits.follow("cc-1", "here");
        // Never had here: kept, as another machine's branch.
        awaits.branches_now(|branch| branch == "here");
        assert_eq!(awaits.of("cc-1"), ["elsewhere", "here"]);
        // Had, and gone.
        awaits.branches_now(|_| false);
        assert_eq!(awaits.of("cc-1"), ["elsewhere"]);
    }
}
