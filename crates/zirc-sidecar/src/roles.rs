//! The organisation's coworkers ([`zirc_proto::coworkers`]), each a nick of the server's own
//! that stands for it: listed among the members, written to by anyone, and speaking for
//! whichever session sits at its desk. What a coworker does is its job's, which several may
//! hold, each in its own repository. A coworker is a role to every reader that knows roles
//! ([`Staff::role_of`]), and so "role" below is a coworker as its desk stands.
//!
//! Who they are comes from zirc.dev, whole: a staff ([`Given`], `PUT /v1/staff`). A
//! coworker's repository is set here when a session first sits at its desk ([`Brain::sit`]), and kept when zirc.dev gives none. Who sits at a desk comes
//! from the lease events machines say in the organisation's channel ([`zirc_proto::leases`]),
//! the seat being the lease [`roles::lease`], worked out here as every zircd works it out.
//! When a seat is taken or left, the role's nick says so in the agents' room
//! ([`roles::SeatChange`]), and to whoever's messages waited for the holder.
//!
//! What comes to a role wakes its holder, through the role's nick to the holder's machine
//! ([`roles::Wake`]), whose zircd takes it only from the role's own account for a session
//! holding the role there: a direct message to the role, from a person, or what one of its
//! triggers matches ([`zirc_proto::trigger`]): a state said, words said where it listens, a
//! person's reaction, a landing's paths, a time coming round. Reports wake it too. What comes
//! for a job, by a trigger, a report or a tool's post, wakes one coworker ([`Brain::wake_job`]):
//! of those holding the job in the repository it's about, or anywhere when it's about none,
//! the one at its desk that was woken longest ago; with none at a desk it waits for the first
//! to sit. For a vacant role, what came waits for the next holder, and the person
//! who wrote is answered, alone, with how to fill it ([`roles::Vacant`]). What the holder says
//! as the role ([`roles::Say`]) goes out from the role's nick, as the holder's session.
//!
//! [`Brain`] decides all of it from what's heard, with no connection, so it's tested on its
//! own; [`Host`] keeps the nicks connected and carries out what it says. What's kept, beside
//! Ergo's history ([`KEPT`]): the staff, each role's last holder, what waits for the next
//! one, and when each role's times last came round. Nothing anyone said is logged.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};
use zirc_proto::coworkers::{Given, Staff};
use zirc_proto::desk::{self, Came, Emptied, Handover};
use zirc_proto::leases::{LeaseEvent, Leases};
use zirc_proto::roles::{
    self, Address, Definition, Holder, Listing, Say, Seat, SeatChange, Standing, Vacant, Wake,
};
use zirc_proto::session::SessionCard;
use zirc_proto::state::State;
use zirc_proto::trigger::{Happened, came_round};
use zirc_proto::woke::{By, What, Woke};
use zirc_proto::{Event, Message};

/// The file beside Ergo's history what's kept of roles is in.
pub const KEPT: &str = "zirc-roles.json";

/// How long after the sidecar starts, and after a role's nick last asked the machines what
/// they hold ([`Input::Asked`]), it only listens, in milliseconds: the machines say their
/// lease events again when asked, at most a minute apart, so until then a seat that seems
/// vacant may not be, and no seat is said taken or left. Counted from the start alone, a
/// nick that joined late heard the machines too late: a restart said its held seat left, and
/// it came back ten minutes on (25 Nov 2026).
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

/// Something that came for a job while none of its holders was at a desk: what wakes the
/// first of them to sit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// The job.
    pub job: String,
    /// The repository it's about, if it's about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// What came.
    pub waiting: Waiting,
}

/// What's kept, beside the history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// The coworkers and jobs, each coworker's repository as zirc.dev gave it or as set here.
    #[serde(default)]
    pub staff: Staff,
    /// Each job's address, as zirc.dev last gave them.
    #[serde(default)]
    pub addresses: Vec<Address>,
    /// What zirc.dev said the staff is at.
    #[serde(default)]
    pub version: String,
    /// The repository set here for each coworker zirc.dev gave none, by coworker: from the
    /// checkout of the session that first sat at its desk.
    #[serde(default)]
    pub sat: BTreeMap<String, String>,
    /// What waits for the first holder of its job to sit, oldest first.
    #[serde(default)]
    pub pending: Vec<Pending>,
    /// When each coworker was last woken, by coworker, in unix milliseconds: of several that
    /// may take what comes for a job, the one woken longest ago does.
    #[serde(default)]
    pub woke_at: BTreeMap<String, u64>,
    /// Each role's holder, as last said, by role.
    #[serde(default)]
    pub held: BTreeMap<String, Holder>,
    /// Each role's holder before, by role.
    #[serde(default)]
    pub last: BTreeMap<String, Holder>,
    /// What waits for each vacant role's next holder, oldest first, by role.
    #[serde(default)]
    pub waiting: BTreeMap<String, Vec<Waiting>>,
    /// When each job's trigger last came round and was dealt with, by job and trigger.
    #[serde(default)]
    pub fired: BTreeMap<String, BTreeMap<usize, u64>>,
    /// How many times each role woke on the day it last did, by role: the day (days since
    /// 1970, UTC) and the count.
    #[serde(default)]
    pub woke: BTreeMap<String, (u64, u32)>,
    /// What woke each coworker lately, oldest first, at most [`desk::RECENT`], by
    /// coworker: for the next session to sit at its desk ([`Brain::handover`]).
    #[serde(default)]
    pub recent: BTreeMap<String, Vec<Waiting>>,
    /// What a session at each coworker's desk last said it was doing there, by coworker.
    #[serde(default)]
    pub doing: BTreeMap<String, String>,
    /// The coworkers whose hirer was told their desk is empty with work waiting, since a
    /// session last asked for it ([`Brain::tell_hirer`]).
    #[serde(default)]
    pub told: BTreeSet<String>,
    /// The coworkers a session has ever asked for the desk of: one nobody has sat at doesn't
    /// empty.
    #[serde(default)]
    pub seated: BTreeSet<String>,
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
    /// A role's nick joined the organisation's channel and asked its machines what they hold,
    /// which they answer by saying their lease events again.
    Asked,
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
    /// Every coworker as the role it amounts to ([`Staff::desks`]).
    roles: Vec<Definition>,
    leases: Leases,
    /// Whose each machine is, by machine, as heard with its lease events.
    owners: BTreeMap<String, String>,
    /// What each machine said its sessions are called, by machine and session.
    cards: BTreeMap<(String, String), SessionCard>,
    /// When it started, or a nick last asked the machines what they hold, in unix
    /// milliseconds: until [`LISTEN_FIRST`] after, seats are learnt, never said.
    listens_from: u64,
    /// Whether what's kept changed since it was last written.
    changed: bool,
}

impl Brain {
    /// A brain that starts at `now` (unix milliseconds) from what was `kept`.
    #[must_use]
    pub fn new(kept: Kept, now: u64) -> Self {
        Self {
            roles: kept.staff.desks(),
            kept,
            listens_from: now,
            ..Self::default()
        }
    }

    /// What's kept now, if it changed since this was last asked.
    pub fn take_kept(&mut self) -> Option<Kept> {
        std::mem::take(&mut self.changed).then(|| self.kept.clone())
    }

    /// Every coworker as the role it amounts to.
    #[must_use]
    pub fn roles(&self) -> &[Definition] {
        &self.roles
    }

    /// The staff, each coworker's repository as it's known here.
    #[must_use]
    pub fn staff(&self) -> &Staff {
        &self.kept.staff
    }

    /// The role coworker `name` amounts to.
    fn role(&self, name: &str) -> Option<&Definition> {
        self.roles.iter().find(|role| role.name == name)
    }

    /// The signing secrets of job `job`'s address, by tool, when `secret` opens it; `None`
    /// when it doesn't, or the job has no address.
    #[must_use]
    pub fn opened(
        &self,
        job: &str,
        secret: &str,
    ) -> Option<BTreeMap<zirc_proto::inbound::Tool, String>> {
        self.kept
            .addresses
            .iter()
            .find(|address| address.role == job && address.opens(secret))
            .map(|address| address.signing.clone())
    }

    /// Takes zirc.dev's word for the staff, if it keeps every rule. A coworker it gives no
    /// repository keeps the one set here ([`Brain::sit`]). What waited and can now be taken
    /// is said.
    ///
    /// # Errors
    ///
    /// Why it doesn't.
    pub fn set_staff(&mut self, given: Given, now: u64) -> Result<Vec<Out>, String> {
        given.check()?;
        let Given {
            mut staff,
            addresses,
            version,
        } = given;
        // zirc.dev's word for a repository is the one; where it says none, what a session
        // sitting there set.
        self.kept.sat.retain(|name, _| {
            staff
                .coworker(name)
                .is_some_and(|coworker| coworker.repository.is_none())
        });
        for coworker in &mut staff.coworkers {
            if coworker.repository.is_none() {
                coworker.repository = self.kept.sat.get(&coworker.name).cloned();
            }
        }
        let names: BTreeSet<&str> = staff.coworkers.iter().map(|c| c.name.as_str()).collect();
        let jobs: BTreeSet<&str> = staff.jobs.iter().map(|job| job.name.as_str()).collect();
        self.kept
            .held
            .retain(|name, _| names.contains(name.as_str()));
        self.kept
            .waiting
            .retain(|name, _| names.contains(name.as_str()));
        self.kept
            .woke_at
            .retain(|name, _| names.contains(name.as_str()));
        self.kept.fired.retain(|job, _| jobs.contains(job.as_str()));
        self.kept
            .pending
            .retain(|pending| jobs.contains(pending.job.as_str()));
        self.kept.staff = staff;
        self.kept.addresses = addresses;
        self.kept.version = version;
        self.derive();
        Ok(self.deliver_pending(now))
    }

    /// Sits a session at coworker `name`'s desk for the first time, in a checkout of
    /// `repository`: the coworker's from now on, if it had none. Answers the role it amounts
    /// to now, and what waited for it that it can now take.
    ///
    /// # Errors
    ///
    /// When there's no such coworker, or it works in another repository, in words for the
    /// session.
    pub fn sit(
        &mut self,
        name: &str,
        repository: &str,
        now: u64,
    ) -> Result<(Definition, Vec<Out>), String> {
        let repository = repository.trim();
        let Some(coworker) = self
            .kept
            .staff
            .coworkers
            .iter_mut()
            .find(|coworker| coworker.name == name)
        else {
            return Err(format!("there's no coworker named {name}"));
        };
        if repository.is_empty() {
            return Err("a coworker's repository has a name".to_owned());
        }
        match coworker.repository.as_deref() {
            Some(theirs) if theirs == repository => {}
            Some(theirs) => return Err(zirc_proto::coworkers::works_elsewhere(name, theirs)),
            None => {
                coworker.repository = Some(repository.to_owned());
                self.kept.sat.insert(name.to_owned(), repository.to_owned());
                self.derive();
            }
        }
        let outs = self.deliver_pending(now);
        let role = self
            .role(name)
            .cloned()
            .ok_or_else(|| format!("there's no coworker named {name}"))?;
        Ok((role, outs))
    }

    /// Works out again what follows from the staff, which changed: the roles it amounts to.
    fn derive(&mut self) {
        self.roles = self.kept.staff.desks();
        self.changed = true;
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
                title: None,
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
            // Never the addresses' digests: what it answers reaches members and machines.
            listing: Listing {
                roles: self.roles.clone(),
                addresses: Vec::new(),
                version: self.kept.version.clone(),
            },
            seats: self.roles.iter().map(|role| self.seat(role, now)).collect(),
            staff: self.kept.staff.clone(),
        }
    }

    fn seat(&self, role: &Definition, now: u64) -> Seat {
        Seat {
            role: role.name.clone(),
            repository: role.repository.clone(),
            holder: self.holder(&role.name, now),
            last: self.kept.last.get(&role.name).cloned(),
            waiting: self.waiting_for(&role.name),
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
                    // A session asking for a desk sits there, as far as telling its hirer
                    // goes, at once: seats are said only after listening first.
                    if let LeaseEvent::Want(want) = &event
                        && let Some(desk) = roles::of_lease(&want.name)
                    {
                        let new = self.kept.seated.insert(desk.to_owned());
                        let untold = self.kept.told.remove(desk);
                        self.changed |= new || untold;
                    }
                    self.leases.apply(event);
                }
                self.seats_moved(now)
            }
            Input::Asked => {
                self.listens_from = self.listens_from.max(now);
                Vec::new()
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
                    None,
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
        for job in self.held_jobs() {
            for (i, trigger) in job.triggers.iter().enumerate() {
                let Some(schedule) = trigger.schedule().ok().flatten() else {
                    continue;
                };
                let fired = self.kept.fired.entry(job.name.clone()).or_default();
                let Some((at, act)) = came_round(schedule, fired.get(&i).copied(), seconds) else {
                    continue;
                };
                fired.insert(i, at);
                self.changed = true;
                if act {
                    let woke = Woke::new(What::Other, By::Zirc);
                    // A time is about no repository: any holder takes it.
                    outs.extend(self.wake_job(&job.name, None, woke, &trigger.words(), now));
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
            .staff
            .jobs
            .iter()
            .flat_map(|job| &job.triggers)
            .filter_map(|trigger| trigger.schedule().ok().flatten())
            .map(|schedule| schedule.next(seconds) * 1000)
            .chain([now + zirc_proto::leases::SETTLE])
            .min()
            .unwrap_or(now + zirc_proto::leases::SETTLE)
    }

    /// The jobs a coworker holds: what a trigger can wake someone for.
    fn held_jobs(&self) -> Vec<zirc_proto::coworkers::Job> {
        let staff = &self.kept.staff;
        staff
            .jobs
            .iter()
            .filter(|job| staff.holders(&job.name).next().is_some())
            .cloned()
            .collect()
    }

    /// Counts a wake of coworker `name` at `now`.
    fn count(&mut self, name: &str, now: u64) {
        let today = now / DAY_MS;
        let count = self.kept.woke.entry(name.to_owned()).or_insert((today, 0));
        if count.0 != today {
            *count = (today, 0);
        }
        count.1 = count.1.saturating_add(1);
        self.kept.woke_at.insert(name.to_owned(), now);
        self.changed = true;
    }

    /// How many things wait for coworker `name`: messages to it, and what came for a job it
    /// may take.
    fn waiting_for(&self, name: &str) -> u32 {
        u32::try_from(
            self.kept.waiting.get(name).map_or(0, Vec::len)
                + self
                    .kept
                    .pending
                    .iter()
                    .filter(|pending| self.may_take(name, pending))
                    .count(),
        )
        .unwrap_or(u32::MAX)
    }

    /// The direct message from coworker `name` telling whoever hired it, at `now`, that its
    /// desk is empty with work waiting, once per emptying (the user's choice, 27 Nov 2026):
    /// when a session has left it and something waits. None while a session sits there, for
    /// a desk nobody has sat at, when nothing waits, when told already since a session last
    /// asked for it, or when nobody's said to have hired it.
    fn tell_hirer(&mut self, name: &str, now: u64) -> Option<Out> {
        if !self.kept.seated.contains(name)
            || self.kept.told.contains(name)
            || self.holder(name, now).is_some()
            || self.waiting_for(name) == 0
        {
            return None;
        }
        let waiting = self.waiting_for(name);
        self.kept.told.insert(name.to_owned());
        self.changed = true;
        let hirer = self.kept.staff.coworker(name)?.added_by.clone()?;
        let emptied = Emptied {
            coworker: name.to_owned(),
            waiting,
        };
        Some(Out {
            role: name.to_owned(),
            to: hirer,
            message: Message::new(name, emptied.event()),
        })
    }

    /// Keeps that `woke` and `text` woke coworker `name` at `at` (unix seconds), for the next
    /// session to sit at its desk.
    fn remember(&mut self, name: &str, woke: &Woke, text: &str, at: u64) {
        let recent = self.kept.recent.entry(name.to_owned()).or_default();
        recent.push(Waiting {
            woke: woke.clone(),
            text: text.to_owned(),
            at,
        });
        if recent.len() > desk::RECENT {
            recent.remove(0);
        }
        self.changed = true;
    }

    /// What coworker `name`'s desk hands the next session to sit there: what a session there
    /// was doing, what woke the coworker lately and what waits for it. `None` when
    /// there's no such coworker.
    #[must_use]
    pub fn handover(&self, name: &str) -> Option<Handover> {
        let role = self.role(name)?;
        let came = |waited: &Waiting| Came {
            woke: waited.woke.clone(),
            text: waited.text.clone(),
            at: waited.at,
        };
        let waiting = self
            .kept
            .waiting
            .get(&role.name)
            .into_iter()
            .flatten()
            .chain(
                self.kept
                    .pending
                    .iter()
                    .filter(|pending| self.may_take(&role.name, pending))
                    .map(|pending| &pending.waiting),
            )
            .map(came)
            .collect();
        Some(Handover {
            coworker: role.name.clone(),
            doing: self.kept.doing.get(&role.name).cloned(),
            recent: self
                .kept
                .recent
                .get(&role.name)
                .into_iter()
                .flatten()
                .map(came)
                .collect(),
            waiting,
        })
    }

    /// Keeps what `message`, said by a machine, says a session at a coworker's desk is doing
    /// there: its `taking:`, `done:` or `dropped:`, or its intent, said as the coworker
    /// ([`Message::desk`]).
    fn note_doing(&mut self, message: &Message) {
        let Some(desk) = message.desk.as_deref() else {
            return;
        };
        let Some(name) = self.role(desk).map(|role| role.name.clone()) else {
            return;
        };
        let doing = match &message.event {
            Event::Intent { summary, .. } => summary,
            Event::Text { body }
                if zirc_proto::says_taking(body) || zirc_proto::says_finished(body) =>
            {
                body
            }
            _ => return,
        };
        let doing = zirc_proto::clip_line(doing, zirc_proto::session::MAX_TITLE);
        if !doing.is_empty() {
            self.kept.doing.insert(name, doing);
            self.changed = true;
        }
    }

    /// Wakes the session at coworker `role`'s desk with `woke` and `text`, or keeps it for
    /// the next one: what a direct message does.
    pub fn wake(&mut self, role: &str, woke: Woke, text: &str, now: u64) -> Vec<Out> {
        let Some(definition) = self.role(role) else {
            return Vec::new();
        };
        let role = definition.name.clone();
        let woke = Woke {
            role: Some(role.clone()),
            ..woke
        };
        self.count(&role, now);
        if let Some(holder) = self.holder(&role, now) {
            self.remember(&role, &woke, text, now / 1000);
            return vec![wake_out(&role, &holder, woke, text, now / 1000)];
        }
        let waiting = self.kept.waiting.entry(role.clone()).or_default();
        waiting.push(Waiting {
            woke,
            text: text.to_owned(),
            at: now / 1000,
        });
        if waiting.len() > MAX_WAITING {
            waiting.remove(0);
        }
        self.changed = true;
        self.tell_hirer(&role, now).into_iter().collect()
    }

    /// The coworker that takes what comes for `job` about `repository` at `now`, with the
    /// session at its desk: of the job's holders there ([`Staff::takers`]) with a session at
    /// their desks, the one woken longest ago, the first added of those never woken.
    fn taker(&self, job: &str, repository: Option<&str>, now: u64) -> Option<(String, Holder)> {
        self.kept
            .staff
            .takers(job, repository)
            .filter_map(|coworker| Some((coworker.name.clone(), self.holder(&coworker.name, now)?)))
            .min_by_key(|(name, _)| self.kept.woke_at.get(name).copied().unwrap_or(0))
    }

    /// Whether coworker `name` may take `pending`: it holds the job, in the repository it's
    /// about.
    fn may_take(&self, name: &str, pending: &Pending) -> bool {
        self.kept
            .staff
            .takers(&pending.job, pending.repository.as_deref())
            .any(|coworker| coworker.name == name)
    }

    /// Wakes one holder of `job` with `woke` and `text`, which is about `repository` or about
    /// none ([`Brain::taker`]); with none at a desk, keeps it for the first of them to sit:
    /// what a trigger, a report or a tool's post does.
    pub fn wake_job(
        &mut self,
        job: &str,
        repository: Option<&str>,
        woke: Woke,
        text: &str,
        now: u64,
    ) -> Vec<Out> {
        if self.kept.staff.job(job).is_none() {
            return Vec::new();
        }
        if let Some((name, holder)) = self.taker(job, repository, now) {
            self.count(&name, now);
            let woke = Woke {
                role: Some(name.clone()),
                ..woke
            };
            self.remember(&name, &woke, text, now / 1000);
            return vec![wake_out(&name, &holder, woke, text, now / 1000)];
        }
        if repository.is_some() && self.kept.staff.takers(job, repository).next().is_none() {
            info!(
                "what came for a job waits: none of its holders works in the repository it's about"
            );
        }
        self.kept.pending.push(Pending {
            job: job.to_owned(),
            repository: repository.map(str::to_owned),
            waiting: Waiting {
                woke,
                text: text.to_owned(),
                at: now / 1000,
            },
        });
        let of_job = |pending: &Pending| pending.job == job;
        if self.kept.pending.iter().filter(|p| of_job(p)).count() > MAX_WAITING
            && let Some(oldest) = self.kept.pending.iter().position(of_job)
        {
            self.kept.pending.remove(oldest);
        }
        self.changed = true;
        let takers: Vec<String> = self
            .kept
            .staff
            .takers(job, repository)
            .map(|coworker| coworker.name.clone())
            .collect();
        takers
            .iter()
            .filter_map(|name| self.tell_hirer(name, now))
            .collect()
    }

    /// Wakes whoever can now take what waited for a job: said when a session sits, and when
    /// the staff or a coworker's repository changes.
    fn deliver_pending(&mut self, now: u64) -> Vec<Out> {
        let mut outs = Vec::new();
        for pending in std::mem::take(&mut self.kept.pending) {
            let Some((name, holder)) = self.taker(&pending.job, pending.repository.as_deref(), now)
            else {
                self.kept.pending.push(pending);
                continue;
            };
            self.count(&name, now);
            let woke = Woke {
                role: Some(name.clone()),
                ..pending.waiting.woke
            };
            self.remember(&name, &woke, &pending.waiting.text, pending.waiting.at);
            outs.push(wake_out(
                &name,
                &holder,
                woke,
                &pending.waiting.text,
                pending.waiting.at,
            ));
        }
        outs
    }

    /// The coworker whose nick says what's said for `job` about `repository`: one that takes
    /// it, else one that holds it, else any, as someone must say it.
    #[must_use]
    pub fn speaker(&self, job: &str, repository: Option<&str>) -> Option<String> {
        let staff = &self.kept.staff;
        staff
            .takers(job, repository)
            .next()
            .or_else(|| staff.holders(job).next())
            .or_else(|| staff.coworkers.first())
            .map(|coworker| coworker.name.clone())
    }

    /// Forgets what waits about the item `id`, which closed: nobody is woken for what's done.
    pub fn drop_waiting(&mut self, id: &str) {
        let about = |waited: &Waiting| {
            waited
                .woke
                .report
                .as_ref()
                .is_some_and(|reported| reported.id == id)
        };
        let before =
            self.kept.pending.len() + self.kept.waiting.values().map(Vec::len).sum::<usize>();
        self.kept.pending.retain(|pending| !about(&pending.waiting));
        for waiting in self.kept.waiting.values_mut() {
            waiting.retain(|waited| !about(waited));
        }
        let after =
            self.kept.pending.len() + self.kept.waiting.values().map(Vec::len).sum::<usize>();
        if after != before {
            self.changed = true;
        }
    }

    /// Wakes one holder of `wake`'s job for one of its items at `now`, or keeps it for the
    /// first to sit; for a tool's new report, a card in the agents' room too, from whoever
    /// takes it, saying whether it woke them or waits ([`zirc_proto::inbound::card`]).
    pub fn reported(&mut self, wake: &crate::items::Wake, now: u64) -> Vec<Out> {
        let repository = wake.item.report.repository.as_deref();
        let mut outs = self.wake_job(&wake.role, repository, wake.woke(), &wake.text(), now);
        if wake.reminder || wake.item.report.tool.is_none() {
            return outs;
        }
        let woken = outs.first().map(|out| out.role.clone());
        let held = woken.is_some();
        let Some(role) = woken
            .or_else(|| self.speaker(&wake.role, repository))
            .and_then(|name| self.role(&name).cloned())
        else {
            return outs;
        };
        outs.push(Out {
            role: role.name.clone(),
            to: zirc_proto::AGENTS_CHANNEL.to_owned(),
            message: Message {
                repo: repo_of(&role),
                ..Message::new(
                    &role.name,
                    zirc_proto::inbound::card_event(&wake.item, held),
                )
            },
        });
        outs
    }

    /// Each seat taken or left since last said, said: in the agents' room, and to whoever's
    /// messages waited; and what waited, to the new holder.
    fn seats_moved(&mut self, now: u64) -> Vec<Out> {
        if now < self.listens_from + LISTEN_FIRST {
            return Vec::new();
        }
        let mut outs = Vec::new();
        for role in self.roles.clone() {
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
                // Left empty: whoever hired it is told now if work waits, else once some
                // comes; told so, the machine's owner isn't also told it left.
                let mut told = None;
                if let Some(out) = self.tell_hirer(&name, now) {
                    told = Some(out.to.clone());
                    outs.push(out);
                }
                if let Some(owner) = self.owners.get(&before.machine.to_lowercase())
                    && told.as_deref() != Some(owner.as_str())
                {
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
                    self.remember(&name, &waited.woke, &waited.text, waited.at);
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
        if !self.kept.pending.is_empty() {
            outs.extend(self.deliver_pending(now));
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
                repo: repo_of(role),
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
        let Some(definition) = self.role(role).cloned() else {
            return Vec::new();
        };
        // The holder, speaking as the role, or saying it woke for a person's word to the role
        // (zirc_proto::woke::Woke::chat): said in their chat with the role, as it's theirs.
        if let Some(message) = &message {
            let said = Say::of(&message.event);
            let woke = Woke::of(&message.event);
            if said.is_some() || woke.is_some() {
                let holder = self.holder(role, now);
                let theirs = sender == Sender::Machine
                    && holder.as_ref().is_some_and(|holder| {
                        holder.machine.eq_ignore_ascii_case(account)
                            && holder.session.session == message.session
                    });
                let out = match (said, woke) {
                    (Some((say, text)), _) if say.role == role => {
                        Some(Self::say_as(&definition, message, say, text))
                    }
                    (_, Some(woke)) if woke.role.as_deref() == Some(role) => {
                        woke.chat().and_then(|chat| match chat {
                            zirc_proto::woke::Chat::Direct(to) => Some(Out {
                                role: definition.name.clone(),
                                to: to.to_owned(),
                                message: Message {
                                    repo: repo_of(&definition),
                                    ..message.clone()
                                },
                            }),
                            zirc_proto::woke::Chat::Channel(_) => None,
                        })
                    }
                    _ => None,
                };
                let Some(out) = out.filter(|_| theirs) else {
                    info!(
                        role,
                        "something said as a role by a session not holding it: refused"
                    );
                    return Vec::new();
                };
                return vec![out];
            }
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
                desk: message.desk.clone(),
                repo: repo_of(role),
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
        if sender == Sender::Machine
            && let Some(message) = message
        {
            self.note_doing(message);
        }
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
            let repository = about(message).or_else(|| named(&state.repo));
            return self.triggered(&happened, &woke, repository.as_deref(), now);
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
            return self.triggered(
                &Happened::Landed { changed: &changed },
                &woke,
                about(message).as_deref(),
                now,
            );
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
        self.triggered(
            &Happened::Said { text: words },
            &woke,
            about(message).as_deref(),
            now,
        )
    }

    /// Wakes one holder of each held job with a trigger `happened` matches, which is about
    /// `repository` or about none.
    fn triggered(
        &mut self,
        happened: &Happened<'_>,
        woke: &Woke,
        repository: Option<&str>,
        now: u64,
    ) -> Vec<Out> {
        let matched: Vec<(String, String)> = self
            .held_jobs()
            .iter()
            .filter_map(|job| {
                let trigger = job.triggers.iter().find(|t| t.matches(happened))?;
                Some((job.name.clone(), trigger.words()))
            })
            .collect();
        matched
            .into_iter()
            .flat_map(|(job, words)| self.wake_job(&job, repository, woke.clone(), &words, now))
            .collect()
    }
}

/// The repository `role` works in, as a line it says is about; none before a session has sat
/// at its desk.
fn repo_of(role: &Definition) -> Option<String> {
    named(&role.repository)
}

/// The repository `said` names, by name as zirc spells it, where it names one: a state names
/// it as its channel was (`#monorepo`).
fn named(said: &str) -> Option<String> {
    let name = said.trim().trim_start_matches('#');
    (!name.is_empty()).then(|| name.to_owned())
}

/// The repository `message` is about, where it says one.
fn about(message: Option<&Message>) -> Option<String> {
    message
        .and_then(|message| message.repo.as_deref())
        .and_then(named)
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
        let names = brain.roles().iter().map(|r| r.name.clone()).collect();
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

    /// What coworker `name`'s desk hands the next session to sit there ([`Brain::handover`]).
    #[must_use]
    pub fn handover(&self, name: &str) -> Option<Handover> {
        self.with(|brain| (brain.handover(name), Vec::new()))
    }

    /// Runs `f` on the brain for what it has the nicks say, or why it won't, then connects
    /// a nick for each coworker there now is.
    fn changed<T>(
        &self,
        f: impl FnOnce(&mut Brain) -> Result<(T, Vec<Out>), String>,
    ) -> Result<T, String> {
        let (answer, names) = self.with(|brain| match f(brain) {
            Ok((answer, outs)) => {
                let names = brain.roles().iter().map(|r| r.name.clone()).collect();
                ((Ok(answer), Some(names)), outs)
            }
            Err(why) => ((Err(why), None), Vec::new()),
        });
        if let Some(names) = names {
            self.names
                .send_if_modified(|current: &mut BTreeSet<String>| {
                    let modified = *current != names;
                    *current = names;
                    modified
                });
        }
        answer
    }

    /// Takes zirc.dev's word for the staff ([`Brain::set_staff`]).
    ///
    /// # Errors
    ///
    /// Why it won't do.
    pub fn set_staff(&self, given: Given) -> Result<(), String> {
        self.changed(|brain| Ok(((), brain.set_staff(given, crate::now_ms())?)))
    }

    /// Sits a session at coworker `name`'s desk for the first time, in a checkout of
    /// `repository` ([`Brain::sit`]).
    ///
    /// # Errors
    ///
    /// Why it can't, in words for the session.
    pub fn sit(&self, name: &str, repository: &str) -> Result<Definition, String> {
        self.changed(|brain| brain.sit(name, repository, crate::now_ms()))
    }

    /// Wakes one holder of `job` with `woke` and `text`, about `repository` or none, or
    /// keeps it for the first to sit ([`Brain::wake_job`]): what a tool's post does, say.
    pub fn wake_job(&self, job: &str, repository: Option<&str>, woke: Woke, text: &str) {
        self.with(|brain| {
            (
                (),
                brain.wake_job(job, repository, woke, text, crate::now_ms()),
            )
        });
    }

    /// The signing secrets of job `job`'s address, by tool, when `secret` opens it
    /// ([`Brain::opened`]); `None` when it doesn't.
    #[must_use]
    pub fn opened(
        &self,
        job: &str,
        secret: &str,
    ) -> Option<BTreeMap<zirc_proto::inbound::Tool, String>> {
        self.with(|brain| (brain.opened(job, secret), Vec::new()))
    }

    /// Says `message` in `#ops` for `job` about `repository`, from a coworker's nick
    /// ([`Brain::speaker`]): whether there was one to say it.
    fn say_in_ops(
        &self,
        job: &str,
        repository: Option<&str>,
        message: impl FnOnce(&str) -> Message,
    ) -> bool {
        let Some(nick) = self.with(|brain| (brain.speaker(job, repository), Vec::new())) else {
            return false;
        };
        let out = Out {
            to: zirc_proto::state::CHANNEL.to_owned(),
            message: message(&nick),
            role: nick,
        };
        self.outs.send(vec![out]).is_ok()
    }

    /// Has a coworker's nick say `report` in `#ops`, where its job's items take it
    /// ([`crate::items`]): whether there was one to say it.
    #[must_use]
    pub fn report(&self, report: &zirc_proto::role_report::RoleReport) -> bool {
        self.say_in_ops(&report.role, report.repository.as_deref(), |nick| Message {
            // Its kind's level, said by the sender (the user's choice, 25 Nov 2026).
            level: Some(report.kind.level()),
            ..Message::new(nick, report.event())
        })
    }

    /// Has a coworker's nick say `closing` in `#ops`, where its job's items take it.
    pub fn close(&self, closing: &zirc_proto::role_report::Closing) {
        let said = self.say_in_ops(&closing.role, None, |nick| {
            Message::new(nick, closing.event())
        });
        if !said {
            info!("a job's item closed with no coworker to say so: left open");
        }
    }

    /// Wakes one holder of a job for one of its items, or keeps it for the first to sit: and
    /// for a tool's new report, has a nick say its card in the agents' room.
    pub fn reported(&self, wake: &crate::items::Wake) {
        self.with(|brain| ((), brain.reported(wake, crate::now_ms())));
    }

    /// Forgets what waits about the item `id`, which closed ([`Brain::drop_waiting`]).
    pub fn drop_waiting(&self, id: &str) {
        self.with(|brain| {
            brain.drop_waiting(id);
            ((), Vec::new())
        });
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
        let (nick, line) = match heard {
            connection::Heard::Live { nick, line } => (nick, line),
            connection::Heard::Asked => return Some(Input::Asked),
        };
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
