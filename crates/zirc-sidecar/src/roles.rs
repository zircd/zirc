//! The organisation's roles ([`zirc_proto::roles`]), each a nick of the server's own that
//! stands for it: listed among the members, written to by anyone, and speaking for whichever
//! session holds its one seat.
//!
//! What a role is comes from zirc.dev, whole ([`Listing`], `PUT /v1/roles`); who holds it,
//! from the lease events machines say in the organisation's channel ([`zirc_proto::leases`]),
//! the seat being the lease [`roles::lease`], worked out here as every zircd works it out.
//! When a seat is taken or left, the role's nick says so in the agents' room
//! ([`roles::SeatChange`]), and to whoever's messages waited for the holder.
//!
//! What comes to a role wakes its holder, through the role's nick to the holder's machine
//! ([`roles::Wake`]), whose zircd takes it only from the role's own account for a session
//! holding the role there: a direct message to the role, from a person, or what one of its
//! triggers matches ([`zirc_proto::trigger`]): a state said, words said where it listens, a
//! person's reaction, a landing's paths, a time coming round. Reports wake it too
//! ([`Roles::wake`]). For a vacant role, what came waits for the next holder, and the person
//! who wrote is answered, alone, with how to fill it ([`roles::Vacant`]). What the holder says
//! as the role ([`roles::Say`]) goes out from the role's nick, as the holder's session.
//!
//! [`Brain`] decides all of it from what's heard, with no connection, so it's tested on its
//! own; [`Host`] keeps the nicks connected and carries out what it says. What's kept, beside
//! Ergo's history ([`KEPT`]): the listing, each role's last holder, what waits for the next
//! one, and when each role's times last came round. Nothing anyone said is logged.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};
use zirc_proto::leases::{LeaseEvent, Leases};
use zirc_proto::roles::{
    self, Definition, Holder, Listing, Say, Seat, SeatChange, Standing, Vacant, Wake,
};
use zirc_proto::session::SessionCard;
use zirc_proto::state::State;
use zirc_proto::trigger::{Happened, came_round};
use zirc_proto::woke::{By, What, Woke};
use zirc_proto::{Event, Message};

/// The file beside Ergo's history what's kept of roles is in.
pub const KEPT: &str = "zirc-roles.json";

/// How long after the sidecar starts it only listens, in milliseconds: the machines say
/// their lease events again for whoever joins at most a minute apart, so until then a seat
/// that seems vacant may not be, and no seat is said taken or left.
pub const LISTEN_FIRST: u64 = 90_000;

/// A day, in milliseconds.
const DAY_MS: u64 = 86_400_000;

/// The most messages that wait for a vacant role's next holder: the oldest go first.
pub const MAX_WAITING: usize = 50;

/// Something that came to a role while it was vacant: what wakes its next holder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    /// What woke it, its role said.
    pub woke: Woke,
    /// Its words: a direct message's, or what woke it.
    pub text: String,
    /// When, unix seconds.
    pub at: u64,
}

/// What's kept, beside the history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// The roles, as zirc.dev last said them.
    #[serde(default)]
    pub listing: Listing,
    /// Each role's holder, as last said, by role.
    #[serde(default)]
    pub held: BTreeMap<String, Holder>,
    /// Each role's holder before, by role.
    #[serde(default)]
    pub last: BTreeMap<String, Holder>,
    /// What waits for each vacant role's next holder, oldest first, by role.
    #[serde(default)]
    pub waiting: BTreeMap<String, Vec<Waiting>>,
    /// When each role's trigger last came round and was dealt with, by role and trigger.
    #[serde(default)]
    pub fired: BTreeMap<String, BTreeMap<usize, u64>>,
    /// How many times each role woke on the day it last did, by role: the day (days since
    /// 1970, UTC) and the count.
    #[serde(default)]
    pub woke: BTreeMap<String, (u64, u32)>,
}

/// Who sent something, as the server's accounts say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    /// A person.
    Person,
    /// A machine, whose sessions are agents.
    Machine,
    /// An integration, such as CI.
    Integration,
    /// zirc's own, or nobody signed in.
    Other,
}

/// Something heard, that may concern a role.
#[derive(Debug, Clone)]
pub enum Input {
    /// A machine, `from`, said a lease event in the organisation's channel.
    Lease {
        /// The machine.
        from: String,
        /// Whose machine it is, by handle, as the server knows.
        owner: Option<String>,
        /// What it said.
        event: LeaseEvent,
    },
    /// A machine said what one of its sessions is called.
    Card {
        /// The machine.
        machine: String,
        /// The session's card.
        card: SessionCard,
    },
    /// Something was sent to the role `role`'s nick directly.
    ToRole {
        /// The role.
        role: String,
        /// Who sent it, by nick.
        from: String,
        /// Their account.
        account: String,
        /// Who they are.
        sender: Sender,
        /// Its id on the server.
        id: Option<String>,
        /// What it carries, if it's one of zirc's.
        message: Option<Message>,
        /// Its text.
        text: String,
    },
    /// Something was said in a channel the roles listen in.
    Said {
        /// The channel.
        channel: String,
        /// Who said it, by nick.
        from: String,
        /// Who they are.
        sender: Sender,
        /// Its id on the server.
        id: Option<String>,
        /// What it carries, if it's one of zirc's.
        message: Option<Message>,
        /// Its text.
        text: String,
    },
    /// A person reacted to a message in a channel the roles listen in.
    Reacted {
        /// The channel.
        channel: String,
        /// Who, by nick.
        from: String,
        /// The reaction.
        reaction: String,
        /// The message's id.
        id: String,
    },
}

/// What a role's nick does.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    /// The role whose nick says it.
    pub role: String,
    /// To whom: a nick, or a channel.
    pub to: String,
    /// What it says.
    pub message: Message,
}

/// Every decision about roles, from what's heard.
#[derive(Debug, Default)]
pub struct Brain {
    kept: Kept,
    leases: Leases,
    /// Whose each machine is, by machine, as heard with its lease events.
    owners: BTreeMap<String, String>,
    /// What each machine said its sessions are called, by machine and session.
    cards: BTreeMap<(String, String), SessionCard>,
    /// When it started, in unix milliseconds: until [`LISTEN_FIRST`] after, seats are learnt,
    /// never said.
    started: u64,
    /// Whether what's kept changed since it was last written.
    changed: bool,
}

impl Brain {
    /// A brain that starts at `now` (unix milliseconds) from what was `kept`.
    #[must_use]
    pub fn new(kept: Kept, now: u64) -> Self {
        Self {
            kept,
            started: now,
            ..Self::default()
        }
    }

    /// What's kept now, if it changed since this was last asked.
    pub fn take_kept(&mut self) -> Option<Kept> {
        std::mem::take(&mut self.changed).then(|| self.kept.clone())
    }

    /// The roles.
    #[must_use]
    pub fn listing(&self) -> &Listing {
        &self.kept.listing
    }

    /// Takes zirc.dev's word for the roles, if they keep every rule.
    ///
    /// # Errors
    ///
    /// Why they don't.
    pub fn set_listing(&mut self, listing: Listing) -> Result<(), String> {
        listing.check()?;
        let names: BTreeSet<&str> = listing.roles.iter().map(|r| r.name.as_str()).collect();
        self.kept
            .held
            .retain(|role, _| names.contains(role.as_str()));
        self.kept
            .waiting
            .retain(|role, _| names.contains(role.as_str()));
        self.kept
            .fired
            .retain(|role, _| names.contains(role.as_str()));
        self.kept.listing = listing;
        self.changed = true;
        Ok(())
    }

    /// Who holds `role` at `now` (unix milliseconds), as the lease events say.
    fn holder(&self, role: &str, now: u64) -> Option<Holder> {
        let name = roles::lease(role);
        let lease = self.leases.lease(&name, now, "");
        let held = lease.holders.into_iter().next()?;
        let turn = self.leases.turn_of(&name, &held.nick, &held.session, now)?;
        let card = self
            .cards
            .get(&(held.nick.clone(), held.session.clone()))
            .cloned()
            .unwrap_or_else(|| SessionCard {
                session: held.session.clone(),
                name: None,
                title: String::new(),
                tool: None,
            });
        Some(Holder {
            machine: held.nick,
            since: turn.start / 1000,
            session: card,
        })
    }

    /// How every role stands at `now`.
    #[must_use]
    pub fn standing(&self, now: u64) -> Standing {
        Standing {
            listing: self.kept.listing.clone(),
            seats: self
                .kept
                .listing
                .roles
                .iter()
                .map(|role| self.seat(role, now))
                .collect(),
        }
    }

    fn seat(&self, role: &Definition, now: u64) -> Seat {
        Seat {
            role: role.name.clone(),
            repository: role.repository.clone(),
            holder: self.holder(&role.name, now),
            last: self.kept.last.get(&role.name).cloned(),
            waiting: u32::try_from(self.kept.waiting.get(&role.name).map_or(0, Vec::len))
                .unwrap_or(u32::MAX),
            wakes_on: role.wakes_on(),
            woke_today: self
                .kept
                .woke
                .get(&role.name)
                .filter(|(day, _)| *day == now / DAY_MS)
                .map_or(0, |(_, count)| *count),
        }
    }

    /// What `input`, heard at `now` (unix milliseconds), has the roles' nicks say.
    pub fn heard(&mut self, input: Input, now: u64) -> Vec<Out> {
        match input {
            Input::Lease { from, owner, event } => {
                if let Some(owner) = owner {
                    self.owners.insert(from.to_lowercase(), owner);
                }
                let theirs = match &event {
                    LeaseEvent::Want(want) => want.nick.eq_ignore_ascii_case(&from),
                    LeaseEvent::Release { id, .. } | LeaseEvent::Extend { id, .. } => {
                        id.starts_with(&format!("{from}-"))
                    }
                };
                if theirs {
                    self.leases.apply(event);
                }
                self.seats_moved(now)
            }
            Input::Card { machine, card } => {
                self.cards.insert((machine, card.session.clone()), card);
                Vec::new()
            }
            Input::ToRole {
                role,
                from,
                account,
                sender,
                id,
                message,
                text,
            } => self.came_to(&role, (&from, &account, sender), id, message, text, now),
            Input::Said {
                channel,
                from,
                sender,
                id,
                message,
                text,
            } => self.said(&channel, &from, sender, id, message.as_ref(), &text, now),
            Input::Reacted {
                channel,
                from,
                reaction,
                id,
            } => {
                let woke = Woke {
                    from: Some(from),
                    channel: Some(channel),
                    line: Some(id),
                    ..Woke::new(What::Reacted, By::Person)
                };
                self.triggered(
                    &Happened::Reaction {
                        reaction: &reaction,
                    },
                    &woke,
                    now,
                )
            }
        }
    }

    /// What a role's nick says as time passes, at `now`: a seat taken or left once heard
    /// settled, and each trigger whose time came round.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        let mut outs = self.seats_moved(now);
        let seconds = now / 1000;
        for role in self.kept.listing.roles.clone() {
            for (i, trigger) in role.triggers.iter().enumerate() {
                let Some(schedule) = trigger.schedule().ok().flatten() else {
                    continue;
                };
                let fired = self.kept.fired.entry(role.name.clone()).or_default();
                let Some((at, act)) = came_round(schedule, fired.get(&i).copied(), seconds) else {
                    continue;
                };
                fired.insert(i, at);
                self.changed = true;
                if act {
                    let woke = Woke::new(What::Other, By::Zirc);
                    outs.extend(self.wake(&role.name, woke, &trigger.words(), now));
                }
            }
        }
        outs
    }

    /// When the next trigger's time comes round after `now` (unix milliseconds), in unix
    /// milliseconds, or a settled seat may need saying: whoever runs the brain ticks then.
    #[must_use]
    pub fn next_due(&self, now: u64) -> u64 {
        let seconds = now / 1000;
        self.kept
            .listing
            .roles
            .iter()
            .flat_map(|role| &role.triggers)
            .filter_map(|trigger| trigger.schedule().ok().flatten())
            .map(|schedule| schedule.next(seconds) * 1000)
            .chain([now + zirc_proto::leases::SETTLE])
            .min()
            .unwrap_or(now + zirc_proto::leases::SETTLE)
    }

    /// Wakes `role`'s holder with `woke` and `text`, or keeps it for the next one: what a
    /// trigger, a report or a direct message does.
    pub fn wake(&mut self, role: &str, woke: Woke, text: &str, now: u64) -> Vec<Out> {
        let Some(definition) = self.kept.listing.role(role) else {
            return Vec::new();
        };
        let role = definition.name.clone();
        let woke = Woke {
            role: Some(role.clone()),
            ..woke
        };
        let today = now / DAY_MS;
        let count = self.kept.woke.entry(role.clone()).or_insert((today, 0));
        if count.0 != today {
            *count = (today, 0);
        }
        count.1 = count.1.saturating_add(1);
        self.changed = true;
        if let Some(holder) = self.holder(&role, now) {
            return vec![wake_out(&role, &holder, woke, text, now / 1000)];
        }
        let waiting = self.kept.waiting.entry(role).or_default();
        waiting.push(Waiting {
            woke,
            text: text.to_owned(),
            at: now / 1000,
        });
        if waiting.len() > MAX_WAITING {
            waiting.remove(0);
        }
        self.changed = true;
        Vec::new()
    }

    /// Each seat taken or left since last said, said: in the agents' room, and to whoever's
    /// messages waited; and what waited, to the new holder.
    fn seats_moved(&mut self, now: u64) -> Vec<Out> {
        if now < self.started + LISTEN_FIRST {
            return Vec::new();
        }
        let mut outs = Vec::new();
        for role in self.kept.listing.roles.clone() {
            let name = role.name.clone();
            // Held once settled: the lease model gives a want its turn only after SETTLE.
            let now_held = self.holder(&name, now);
            let before = self.kept.held.get(&name).cloned();
            let same = |a: &Holder, b: &Holder| {
                a.machine == b.machine && a.session.session == b.session.session
            };
            match (&before, &now_held) {
                (Some(before), Some(now_held)) if same(before, now_held) => continue,
                (None, None) => continue,
                _ => {}
            }
            if let Some(before) = before {
                self.kept.last.insert(name.clone(), before.clone());
                self.kept.held.remove(&name);
                let left = Self::seat_said(&role, before.clone(), false);
                if let Some(owner) = self.owners.get(&before.machine.to_lowercase()) {
                    outs.push(Out {
                        to: owner.clone(),
                        ..left.clone()
                    });
                }
                outs.push(left);
            }
            if let Some(holder) = now_held {
                self.kept.held.insert(name.clone(), holder.clone());
                let change = Self::seat_said(&role, holder.clone(), true);
                let waited = self.kept.waiting.remove(&name).unwrap_or_default();
                // Whoever's messages waited, and the holder's owner, in their own
                // conversation with the role.
                let wrote: BTreeSet<String> = waited
                    .iter()
                    .filter(|w| w.woke.what == What::Wrote)
                    .filter_map(|w| w.woke.from.clone())
                    .chain(self.owners.get(&holder.machine.to_lowercase()).cloned())
                    .collect();
                for person in wrote {
                    outs.push(Out {
                        to: person,
                        ..change.clone()
                    });
                }
                outs.push(change);
                for waited in waited {
                    outs.push(wake_out(
                        &name,
                        &holder,
                        waited.woke,
                        &waited.text,
                        waited.at,
                    ));
                }
            }
            self.changed = true;
        }
        outs
    }

    /// The role's nick saying, in the agents' room, that `holder` took or left it.
    fn seat_said(role: &Definition, holder: Holder, took: bool) -> Out {
        let change = SeatChange {
            role: role.name.clone(),
            repository: role.repository.clone(),
            holder,
            took,
        };
        Out {
            role: role.name.clone(),
            to: zirc_proto::AGENTS_CHANNEL.to_owned(),
            message: Message {
                repo: Some(role.repository.clone()),
                ..Message::new(&role.name, change.event())
            },
        }
    }

    /// What came to role `role`'s nick directly.
    fn came_to(
        &mut self,
        role: &str,
        (from, account, sender): (&str, &str, Sender),
        id: Option<String>,
        message: Option<Message>,
        text: String,
        now: u64,
    ) -> Vec<Out> {
        let Some(definition) = self.kept.listing.role(role).cloned() else {
            return Vec::new();
        };
        // The holder, speaking as the role.
        if let Some(message) = &message
            && let Some((say, text)) = Say::of(&message.event)
        {
            let holder = self.holder(role, now);
            let theirs = sender == Sender::Machine
                && holder.as_ref().is_some_and(|holder| {
                    holder.machine.eq_ignore_ascii_case(account)
                        && holder.session.session == message.session
                });
            if !theirs || say.role != role {
                info!(
                    role,
                    "something said as a role by a session not holding it: refused"
                );
                return Vec::new();
            }
            return vec![Self::say_as(&definition, message, say, text)];
        }
        if sender != Sender::Person {
            return Vec::new();
        }
        let text = match message.map(|m| m.event) {
            Some(Event::Text { body }) => body,
            Some(_) => return Vec::new(),
            None => text,
        };
        if text.trim().is_empty() {
            return Vec::new();
        }
        let woke = Woke {
            from: Some(from.to_owned()),
            line: id.clone(),
            ..Woke::new(What::Wrote, By::Person)
        };
        let mut outs = self.wake(role, woke, &text, now);
        if outs.is_empty() {
            let vacant = Vacant {
                seat: self.seat(&definition, now),
                reply_to: id,
            };
            outs.push(Out {
                role: role.to_owned(),
                to: from.to_owned(),
                message: Message::new(role, vacant.event()),
            });
        }
        outs
    }

    /// What the holder says as the role, from the role's nick, as its session.
    fn say_as(role: &Definition, message: &Message, say: Say, text: &str) -> Out {
        Out {
            role: role.name.clone(),
            to: say
                .to
                .unwrap_or_else(|| zirc_proto::AGENTS_CHANNEL.to_owned()),
            message: Message {
                name: message.name.clone(),
                tool: message.tool.clone(),
                avatar: message.avatar.clone(),
                repo: Some(role.repository.clone()),
                ..Message::new(
                    &message.session,
                    Event::Text {
                        body: text.to_owned(),
                    },
                )
            },
        }
    }

    /// What `text`, said in `channel` by `from`, wakes: a state said, a landing, or words a
    /// role listens for.
    #[allow(clippy::too_many_arguments, reason = "one message's parts")]
    fn said(
        &mut self,
        channel: &str,
        from: &str,
        sender: Sender,
        id: Option<String>,
        message: Option<&Message>,
        text: &str,
        now: u64,
    ) -> Vec<Out> {
        let event = message.map(|m| &m.event);
        if let Some(state) = event.and_then(State::of) {
            // How something stands is the team's and its CI's to say.
            if sender == Sender::Other {
                return Vec::new();
            }
            let woke = Woke {
                level: Some(state.level),
                branch: Some(state.name.clone()),
                ..Woke::new(What::State, By::Ci)
            };
            let happened = Happened::State {
                name: &state.name,
                level: state.level,
            };
            return self.triggered(&happened, &woke, now);
        }
        if let Some(Event::Custom { kind, data, .. }) = event
            && kind == zirc_proto::ci::LANDING_KIND
            && sender == Sender::Integration
        {
            let data = zirc_proto::ci::Data::of(data);
            if data.tried || data.state.as_deref() != Some("passed") {
                return Vec::new();
            }
            let woke = Woke {
                build: data.build.clone(),
                branch: data.branch.clone(),
                landing: true,
                ..Woke::new(What::Landed, By::Ci)
            };
            let changed = data.changed;
            return self.triggered(&Happened::Landed { changed: &changed }, &woke, now);
        }
        // A person's or an integration's words, never an agent's.
        if !matches!(sender, Sender::Person | Sender::Integration) {
            return Vec::new();
        }
        let words = match event {
            Some(Event::Text { body }) => body.as_str(),
            Some(_) => return Vec::new(),
            None => text,
        };
        let by = if sender == Sender::Person {
            By::Person
        } else {
            By::Ci
        };
        let woke = Woke {
            from: Some(from.to_owned()),
            channel: Some(channel.to_owned()),
            line: id,
            ..Woke::new(What::Posted, by)
        };
        self.triggered(&Happened::Said { text: words }, &woke, now)
    }

    /// Wakes each role with a trigger `happened` matches.
    fn triggered(&mut self, happened: &Happened<'_>, woke: &Woke, now: u64) -> Vec<Out> {
        let matched: Vec<(String, String)> = self
            .kept
            .listing
            .roles
            .iter()
            .filter_map(|role| {
                let trigger = role.triggers.iter().find(|t| t.matches(happened))?;
                Some((role.name.clone(), trigger.words()))
            })
            .collect();
        matched
            .into_iter()
            .flat_map(|(role, words)| self.wake(&role, woke.clone(), &words, now))
            .collect()
    }
}

/// Role `role`'s nick waking `holder` with `woke` and `text`, which happened `at`.
fn wake_out(role: &str, holder: &Holder, woke: Woke, text: &str, at: u64) -> Out {
    let event = Wake {
        session: holder.session.session.clone(),
        woke,
        at,
    }
    .event(text);
    Out {
        role: role.to_owned(),
        to: holder.machine.clone(),
        message: Message::new(role, event),
    }
}

/// What's kept beside the history `history`.
#[must_use]
pub fn kept(history: &Path) -> Kept {
    std::fs::read(path(history))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn path(history: &Path) -> PathBuf {
    history
        .parent()
        .map_or_else(|| PathBuf::from(KEPT), |dir| dir.join(KEPT))
}

/// Writes `kept` beside the history, whole, renamed into place.
fn keep(history: &Path, kept: &Kept) {
    if let Err(error) = crate::save_json(&path(history), kept) {
        warn!(%error, "couldn't keep the roles");
    }
}

/// The roles, shared by the host that keeps their nicks connected and the endpoints that ask
/// how they stand or change them.
#[derive(Clone)]
pub struct Roles {
    brain: Arc<Mutex<Brain>>,
    history: PathBuf,
    /// What a role's nick is to say, for the host to carry out.
    outs: mpsc::UnboundedSender<Vec<Out>>,
    /// The roles' names now, for the host to connect each.
    names: Arc<tokio::sync::watch::Sender<BTreeSet<String>>>,
}

impl Roles {
    /// The roles kept beside `history`, and what their nicks are to say.
    #[must_use]
    pub fn beside(history: &Path) -> (Self, mpsc::UnboundedReceiver<Vec<Out>>) {
        let brain = Brain::new(kept(history), crate::now_ms());
        let names = brain
            .listing()
            .roles
            .iter()
            .map(|r| r.name.clone())
            .collect();
        let (outs, said) = mpsc::unbounded_channel();
        (
            Self {
                brain: Arc::new(Mutex::new(brain)),
                history: history.to_owned(),
                outs,
                names: Arc::new(tokio::sync::watch::Sender::new(names)),
            },
            said,
        )
    }

    /// Runs `f` on the brain, keeping what changed and passing on what its nicks are to say.
    fn with<T>(&self, f: impl FnOnce(&mut Brain) -> (T, Vec<Out>)) -> T {
        let mut brain = self.brain.lock().unwrap_or_else(PoisonError::into_inner);
        let (answer, outs) = f(&mut brain);
        if let Some(kept) = brain.take_kept() {
            keep(&self.history, &kept);
        }
        drop(brain);
        if !outs.is_empty() {
            let _ = self.outs.send(outs);
        }
        answer
    }

    /// How every role stands now.
    #[must_use]
    pub fn standing(&self) -> Standing {
        self.with(|brain| (brain.standing(crate::now_ms()), Vec::new()))
    }

    /// Takes zirc.dev's word for the roles.
    ///
    /// # Errors
    ///
    /// Why it won't do.
    pub fn set_listing(&self, listing: Listing) -> Result<(), String> {
        let names = listing.roles.iter().map(|r| r.name.clone()).collect();
        self.with(|brain| (brain.set_listing(listing), Vec::new()))?;
        self.names.send_replace(names);
        Ok(())
    }

    /// Wakes `role`'s holder with `woke` and `text`, or keeps it for the next one: what a
    /// report does, say.
    pub fn wake(&self, role: &str, woke: Woke, text: &str) {
        self.with(|brain| ((), brain.wake(role, woke, text, crate::now_ms())));
    }

    /// What `input` has the nicks say.
    pub fn heard(&self, input: Input) {
        self.with(|brain| ((), brain.heard(input, crate::now_ms())));
    }

    /// What the time passing has the nicks say; and when to tick next, in unix milliseconds.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.with(|brain| {
            let now = crate::now_ms();
            let outs = brain.tick(now);
            (brain.next_due(now), outs)
        })
    }

    /// The roles' names, as they change.
    #[must_use]
    pub fn names(&self) -> tokio::sync::watch::Receiver<BTreeSet<String>> {
        self.names.subscribe()
    }
}

/// The roles' names now, which count as zirc's own accounts, never a person's
/// ([`crate::nicks::is_zircs_own`]).
static NAMES: std::sync::LazyLock<std::sync::RwLock<BTreeSet<String>>> =
    std::sync::LazyLock::new(std::sync::RwLock::default);

/// Whether `account` is one of the roles' nicks.
#[must_use]
pub fn is_role(account: &str) -> bool {
    NAMES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&account.to_lowercase())
}

/// The channels a role's nick is in: the organisation's, where seats are agreed; the agents'
/// room, where seats are said and CI lands; and `#ops`, where states are said.
const CHANNELS: [&str; 3] = [
    zirc_proto::ORG_CHANNEL,
    zirc_proto::AGENTS_CHANNEL,
    zirc_proto::state::CHANNEL,
];

/// How long a role whose account couldn't be had waits to be tried again.
const RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// Keeps a nick connected for each role, passing what they hear to [`Roles`] and saying
/// what it has them say.
pub struct Host {
    /// Ergo's loopback address.
    pub host: String,
    /// Its IRC port there.
    pub port: u16,
    /// The server's gate password, if it has one.
    pub gate: Option<String>,
    /// The endpoint's token, which a role's passphrase is derived from.
    pub token: String,
    /// The server's machines, to tell a person from a machine.
    pub machines: crate::machines::Machines,
    /// The roles.
    pub roles: Roles,
}

type Links = BTreeMap<String, mpsc::UnboundedSender<connection::Do>>;

use crate::nicks::connection;

impl Host {
    /// Runs until the process ends. `account` makes sure a nick has an account, made with the
    /// derived passphrase if it has none, never one that's someone's.
    pub async fn run<F, Fut>(self, account: F, mut said: mpsc::UnboundedReceiver<Vec<Out>>)
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let mut names = self.roles.names();
        let mut links = Links::new();
        let (hear, mut heard) = mpsc::unbounded_channel();
        let mut due = crate::now_ms();
        loop {
            let wanted = names.borrow_and_update().clone();
            NAMES
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .clone_from(&wanted);
            links.retain(|name, _| wanted.contains(name));
            let mut short = false;
            let missing: Vec<String> = wanted
                .iter()
                .filter(|name| !links.contains_key(*name))
                .cloned()
                .collect();
            for name in &missing {
                if account(name.clone()).await {
                    info!(role = name, "a role's nick connects");
                    links.insert(name.clone(), self.link(name, &hear));
                } else {
                    warn!(
                        role = name,
                        "a role's nick has no account of its own: not connected"
                    );
                    short = true;
                }
            }
            let wait = std::time::Duration::from_millis(due.saturating_sub(crate::now_ms()));
            tokio::select! {
                changed = names.changed() => if changed.is_err() { return },
                Some(heard) = heard.recv() => {
                    if let Some(input) = self.input(heard, &links) {
                        self.roles.heard(input);
                    }
                }
                Some(outs) = said.recv() => {
                    for out in outs {
                        if let Some(link) = links.get(&out.role) {
                            let _ = link.send(connection::Do::Say(out.to, Box::new(out.message)));
                        }
                    }
                }
                () = tokio::time::sleep(wait) => due = self.roles.tick(),
                () = tokio::time::sleep(RETRY), if short => {}
            }
        }
    }

    /// Connects role `name`'s nick in [`CHANNELS`], passing on what it hears to `hear`.
    fn link(
        &self,
        name: &str,
        hear: &mpsc::UnboundedSender<connection::Heard>,
    ) -> mpsc::UnboundedSender<connection::Do> {
        let (send, todo) = mpsc::unbounded_channel();
        let config = crate::say::config(
            &self.host,
            self.port,
            self.gate.as_deref(),
            &self.token,
            name,
        );
        tokio::spawn(connection::run(
            config,
            CHANNELS.iter().map(|c| (*c).to_owned()).collect(),
            todo,
            hear.clone(),
        ));
        send
    }

    /// Who `account` is.
    fn sender(&self, account: &str) -> Sender {
        let Some(account) = crate::exporter::known_account(account).map(str::to_lowercase) else {
            return Sender::Other;
        };
        if crate::nicks::is_zircs_own(&account) {
            Sender::Other
        } else if zirc_proto::names::is_integration_nick(&account) {
            Sender::Integration
        } else if self.machines.owner(&account).is_some() {
            Sender::Machine
        } else {
            Sender::Person
        }
    }

    /// What `heard`, by one of the nicks, says to the roles: what came to a nick directly,
    /// and, as the first nick heard it in a channel, what was said there.
    fn input(&self, heard: connection::Heard, links: &Links) -> Option<Input> {
        let connection::Heard::Live { nick, line } = heard;
        if !matches!(line.command.as_str(), "PRIVMSG" | "TAGMSG") {
            return None;
        }
        let target = line.params.first()?.to_lowercase();
        let from = line
            .source
            .as_deref()
            .and_then(|source| source.split('!').next())?
            .to_lowercase();
        let account = line.tag("account").unwrap_or_default().to_lowercase();
        let sender = self.sender(&account);
        let id = line.tag("msgid").map(str::to_owned);
        let text = line.params.get(1).cloned().unwrap_or_default();
        let message = if line.command == "TAGMSG" {
            line.tag(zirc_proto::EVENT_TAG)
                .and_then(|payload| zirc_proto::from_payload(payload).ok())
        } else {
            zirc_proto::decode(&text).ok().flatten()
        };
        if target == nick {
            return (line.command == "PRIVMSG").then_some(Input::ToRole {
                role: nick,
                from,
                account,
                sender,
                id,
                message,
                text,
            });
        }
        // Each channel's lines once: as the first of the nicks hears them.
        if links.keys().next() != Some(&nick) || !target.starts_with('#') {
            return None;
        }
        if let Some(Event::Custom { kind, data, .. }) = message.as_ref().map(|m| &m.event)
            && kind == zirc_proto::LEASE_KIND
        {
            let event = serde_json::from_value(data.clone()).ok()?;
            let owner = self.machines.owner(&account);
            return (sender == Sender::Machine && target == zirc_proto::ORG_CHANNEL)
                .then_some(Input::Lease { from, owner, event });
        }
        if let Some(card) = message.as_ref().and_then(|m| SessionCard::of(&m.event)) {
            return (sender == Sender::Machine).then_some(Input::Card {
                machine: from,
                card,
            });
        }
        if line.command == "TAGMSG" {
            let reaction = line.tag("+draft/react")?.to_owned();
            let id = line.tag("+draft/reply")?.to_owned();
            return (sender == Sender::Person).then_some(Input::Reacted {
                channel: target,
                from,
                reaction,
                id,
            });
        }
        Some(Input::Said {
            channel: target,
            from,
            sender,
            id,
            message,
            text,
        })
    }
}

#[cfg(test)]
mod tests;
