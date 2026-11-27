//! The office: the organisation's coworkers at their desks and everyone's sessions at hot
//! desks, as every app's Office tab draws it (the user's choices, 27 Nov 2026: a fifth tab,
//! first; a card per coworker with its character, job and one state line; everyone's
//! hot-desk sessions under them, each with whose it is; Waiting for you atop it, and no
//! longer atop Messages). A coworker's own page and the sheet of an empty desk are here
//! too. Every word and rule is here; apps only draw.
//!
//! What the server keeps of its coworkers, their shown names, jobs and who hired them,
//! comes from its content API ([`zirc_proto::roles::PATH`]), which an app reads and hands
//! in ([`Chat::learn_standing`]); what a desk hands over, its recent wakes and what waits,
//! likewise ([`Chat::learn_handover`]). Where a server says neither, the office is the
//! roles its lines name, by nick.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};
use serde::Serialize;
use ts_rs::TS;
use zirc_proto::coworkers::{Coworker, JobKind, Staff};
use zirc_proto::desk::{Came, Handover};
use zirc_proto::roles::{Look, Seat, Standing};

use super::face::Face;
use super::waiting::WaitingCard;
use super::{Chat, conversation::fold, profile};

/// The Office tab's name, in the tab bar.
pub const OFFICE: &str = "Office";

/// What heads the coworkers' cards.
pub const COWORKERS: &str = "Coworkers";

/// What heads everyone's sessions that sit at no coworker's desk.
pub const HOT_DESKS: &str = "Hot desks";

/// A coworker's state while a session sits at its desk with nothing in progress (the user's
/// choice, 27 Nov 2026).
pub const AT_DESK_IDLE: &str = "At desk, idle";

/// A coworker's state while one of its questions waits for the person.
pub const ASKED_YOU: &str = "Asked you";

/// A coworker's pill on its page while it's on something.
pub const WORKING: &str = "Working";

/// An office with no coworkers yet, in place of their cards (the user's choice, 27 Nov 2026).
pub const NO_COWORKERS: &str = "No coworkers yet.";

/// The button under [`NO_COWORKERS`], which opens zirc.dev's hire page.
pub const HIRE: &str = "Hire a coworker";

/// What heads what a coworker's on, on its page.
pub const NOW: &str = "Now";

/// What heads what lately came for a coworker, on its page.
pub const RECENT: &str = "Recent";

/// A coworker's page's row for its job.
pub const JOB: &str = "Job";

/// A coworker's page's row for its desk.
pub const DESK: &str = "Desk";

/// A coworker's page's row for who hired it.
pub const HIRED_BY: &str = "Hired by";

/// A coworker's job, on its page, while it has none.
pub const NO_JOB: &str = "No job yet: does what it's asked in a direct message.";

/// Whose a hot desk's session is when it's the person's own.
pub const YOURS: &str = "Yours";

/// What copies the sentence that seats a session.
pub const COPY: &str = "Copy";

/// What closes the empty desk's sheet.
pub const DONE: &str = "Done";

/// Where zirc.dev hires a coworker into the organisation `{org}`, under its address.
pub const HIRE_PAGE_PATH: &str = "/orgs/{org}/hire";

/// zirc.dev's page at `web` that hires a coworker into the organisation `org`.
#[must_use]
pub fn hire_page(web: &str, org: &str) -> String {
    format!(
        "{}{}",
        web.trim_end_matches('/'),
        HIRE_PAGE_PATH.replace("{org}", org)
    )
}

/// How a coworker's desk stands, which its state line's colour and its card's frame say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DeskState {
    /// A session sits there and is on something.
    Working,
    /// A session sits there and one of its questions waits for the person.
    Asked,
    /// A session sits there with nothing in progress.
    Idle,
    /// Nobody sits there: what comes for it waits.
    Empty,
}

impl DeskState {
    /// Whether the person should look: something asks or waits for them, drawn in the
    /// warning colour.
    #[must_use]
    pub const fn wants_you(self) -> bool {
        matches!(self, Self::Asked | Self::Empty)
    }
}

/// The Office tab, top to bottom ([`Chat::office`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Office {
    /// What waits for the person's answer, as Messages had it: `None` while nothing does.
    pub waiting: Option<WaitingCard>,
    /// What heads the cards: [`COWORKERS`].
    pub coworkers_heading: String,
    /// Beside it: `3 of 4 at a desk`. `None` with no coworkers.
    pub count: Option<String>,
    /// A card each, in the order they were hired.
    pub coworkers: Vec<CoworkerCard>,
    /// In place of the cards while there are none.
    pub no_coworkers: Option<NoCoworkers>,
    /// What heads the hot desks: [`HOT_DESKS`].
    pub hot_desks_heading: String,
    /// Everyone's sessions at hot desks, online first.
    pub hot_desks: Vec<HotDesk>,
}

/// What an office with no coworkers shows in place of their cards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct NoCoworkers {
    /// [`NO_COWORKERS`].
    pub line: String,
    /// The button that opens zirc.dev's hire page ([`hire_page`]): [`HIRE`].
    pub hire: String,
}

/// A coworker's card in the office.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CoworkerCard {
    /// Its nick, `john`: what its page and its conversation are found by.
    pub nick: String,
    /// The name it's shown by: `John`.
    pub name: String,
    /// Its job, as people read it: `Fixer`. `None` with none.
    pub job: Option<String>,
    /// Its character on its colour: the full figure on a card, dimmed while its desk is
    /// empty.
    pub face: Face,
    /// Its one state line: `Fixing build 431`, `Asked you`, `At desk, idle`, `Desk empty · 1
    /// waiting`.
    pub state: String,
    /// How its desk stands.
    pub desk: DeskState,
    /// What a screen reader says of the card: `John, Fixer, Fixing build 431`.
    pub label: String,
}

/// A session at a hot desk, in the office.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct HotDesk {
    /// Its conversation, `henrik-mbp/cc-1`, which a tap opens.
    pub conversation: String,
    /// The name it goes by, in mono: `checkout-form`, or its id.
    pub name: String,
    /// Whose it is and what it's doing: `Henrik's · Changing the payment form`, `Yours`.
    pub line: String,
    /// Its dealt character, on the muted ground.
    pub face: Face,
    /// Whether a watch of its is open: its dot.
    pub online: bool,
}

/// A row of a coworker's page's About part: [`JOB`], [`DESK`] or [`HIRED_BY`] and what it
/// says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AboutRow {
    /// Its label, muted: `Job`.
    pub label: String,
    /// What it says.
    pub text: String,
}

/// Something that came for a coworker, on its page or its empty desk's sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CameRow {
    /// When, in mono: `09:12`, `Thu 18:30`.
    pub at: String,
    /// What: `CI woke it: build 431 failed`.
    pub text: String,
}

/// A part of a sentence, set in mono where it names something to type or find: a
/// repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct OfficeSpan {
    /// Its words.
    pub text: String,
    /// Whether they're set in mono.
    pub mono: bool,
}

/// A coworker's own page ([`Chat::coworker_page`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CoworkerPage {
    /// Its nick, `john`.
    pub nick: String,
    /// The name it's shown by, large: `John`.
    pub name: String,
    /// Its nick as it's written to, in mono beside the name: `@john`.
    pub handle: String,
    /// Its job, in a pill: `Fixer`.
    pub job: Option<String>,
    /// Its character on its colour, filling the page's top.
    pub face: Face,
    /// How its desk stands.
    pub desk: DeskState,
    /// Its state, in a pill with a dot: `Working`, `Asked you`, `At desk, idle`, `Desk
    /// empty · 1 waiting`.
    pub pill: String,
    /// What heads what it's on: [`NOW`].
    pub now_heading: String,
    /// What it's on: `Fixing build 431`. `None` with nothing in progress.
    pub now: Option<String>,
    /// Under it, muted: what last woke it, `Woke 09:12 · CI woke it: build 431 failed`.
    pub now_under: Option<String>,
    /// Its Message button: `Message John`.
    pub message: String,
    /// Job, Desk and Hired by.
    pub about: Vec<AboutRow>,
    /// What heads what came lately: [`RECENT`].
    pub recent_heading: String,
    /// What woke it lately, newest first.
    pub recent: Vec<CameRow>,
}

/// The sheet an empty desk opens ([`Chat::empty_desk`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct EmptyDesk {
    /// The coworker's nick, whose conversation Message opens.
    pub nick: String,
    /// Its character, dimmed.
    pub face: Face,
    /// `Noor's desk is empty`.
    pub title: String,
    /// Under it, muted: `Triage · last session ended Thu 18:30`.
    pub under: String,
    /// Over what waits: `1 waiting for Noor`. `None` while nothing does.
    pub waiting_heading: Option<String>,
    /// What waits, oldest first.
    pub waiting: Vec<CameRow>,
    /// How to seat a session: `On your computer, start a Claude Code or Codex session in`
    /// `web` (mono) `and say:`.
    pub how: Vec<OfficeSpan>,
    /// The sentence to say: `You're Noor. Take your desk.`
    pub sentence: String,
    /// The button that copies it: [`COPY`].
    pub copy: String,
    /// Under it, muted: what the session does on sitting down.
    pub after: String,
    /// Its Message button: `Message Noor`.
    pub message: String,
    /// What closes the sheet: [`DONE`].
    pub done: String,
    /// Under the buttons, muted: that a message waits too.
    pub foot: String,
}

/// What the office knows of the coworkers beyond their lines: the server's staff and seats,
/// and what each desk hands over.
#[derive(Debug, Clone, Default)]
pub(super) struct Known {
    /// The staff, as the server last said it.
    staff: Option<Staff>,
    /// Each coworker's handover, by nick.
    handovers: HashMap<String, Handover>,
}

impl Chat {
    /// Takes in how the organisation's coworkers stand, as its server's content API says
    /// ([`zirc_proto::roles::PATH`]): who they are, what they look like and their seats,
    /// which hold until a line of a coworker's says otherwise.
    pub fn learn_standing(&mut self, standing: Standing) {
        let now = (self.clock)();
        for coworker in &standing.staff.coworkers {
            let nick = fold(&coworker.name);
            self.roles.insert(nick.clone());
            // What its server's metadata says wins: it's said as it changes.
            self.role_looks.entry(nick).or_insert_with(|| Look {
                picture: coworker.picture.clone(),
                hue: coworker.hue,
            });
        }
        for seat in standing.seats {
            let nick = fold(&seat.role);
            self.roles.insert(nick.clone());
            let known = self.role_seats.entry(nick).or_insert((now, seat.clone()));
            if known.0 <= now {
                *known = (now, seat);
            }
        }
        if !standing.staff.coworkers.is_empty() {
            self.office.staff = Some(standing.staff);
        }
    }

    /// Takes in what coworker `handover.coworker`'s desk hands over, as its server's content
    /// API says ([`zirc_proto::desk::HANDOVER_PATH`]).
    pub fn learn_handover(&mut self, handover: Handover) {
        self.office
            .handovers
            .insert(fold(&handover.coworker), handover);
    }

    /// The coworkers the office has, by nick, in the order they were hired: the server's
    /// staff where it said one, else every role its lines name.
    #[must_use]
    pub fn coworker_nicks(&self) -> Vec<String> {
        match &self.office.staff {
            Some(staff) => staff.coworkers.iter().map(|c| fold(&c.name)).collect(),
            None => self.roles.iter().cloned().collect(),
        }
    }

    fn coworker_of(&self, nick: &str) -> Option<&Coworker> {
        self.office.staff.as_ref()?.coworker(&fold(nick))
    }

    /// The name coworker `nick` is shown by: `John`, or its nick where the server hasn't
    /// said its staff.
    pub(super) fn coworker_name(&self, nick: &str) -> String {
        self.coworker_of(nick)
            .map_or_else(|| fold(nick), |c| c.shown().to_owned())
    }

    /// Coworker `nick`'s job as people read it: `Fixer`, or a job of the office's own by its
    /// name.
    fn coworker_job(&self, nick: &str) -> Option<String> {
        let job = self.coworker_of(nick)?.jobs.first()?;
        Some(JobKind::of_name(job).map_or_else(|| job.clone(), |kind| kind.label().to_owned()))
    }

    /// What coworker `nick`'s job does and what wakes it, on its page.
    fn coworker_job_words(&self, nick: &str) -> String {
        let staff = self.office.staff.as_ref();
        let job = self
            .coworker_of(nick)
            .and_then(|c| c.jobs.first())
            .and_then(|name| staff?.job(name));
        match job {
            None => NO_JOB.to_owned(),
            Some(job) => match job.kind() {
                Some(kind) => format!("{} Wakes on {}.", kind.line(), kind.wakes()),
                None => job.brief.clone(),
            },
        }
    }

    /// What the session at coworker `nick`'s desk is on, in words: its live intent or
    /// what it's taking on, else what the desk last handed over while it's seated.
    fn coworker_doing(&self, seat: &Seat, now: DateTime<Utc>) -> Option<String> {
        let holder = seat.holder.as_ref()?;
        let doing = self.doing_now(&holder.machine, &holder.session.session, now);
        if doing.active {
            return Some(doing.task.unwrap_or_else(|| {
                doing
                    .lead
                    .split_once(' ')
                    .map_or(doing.lead.clone(), |(_, words)| words.to_owned())
            }));
        }
        None
    }

    /// Whether a question or request of the session at the desk `seat` waits for us.
    fn coworker_asked(&self, seat: &Seat) -> bool {
        let Some(holder) = &seat.holder else {
            return false;
        };
        self.asking().iter().any(|(machine, session)| {
            machine.eq_ignore_ascii_case(&holder.machine) && *session == holder.session.session
        })
    }

    /// How coworker `nick`'s desk stands at `now`, and its state line.
    fn desk_state(&self, nick: &str, now: DateTime<Utc>) -> (DeskState, String) {
        let seat = self.role_seat(nick);
        match seat.as_ref().filter(|seat| seat.holder.is_some()) {
            None => (
                DeskState::Empty,
                zirc_proto::desk::empty(seat.map_or(0, |seat| seat.waiting)),
            ),
            Some(seat) if self.coworker_asked(seat) => (DeskState::Asked, ASKED_YOU.to_owned()),
            Some(seat) => match self.coworker_doing(seat, now) {
                Some(doing) => (DeskState::Working, doing),
                None => (DeskState::Idle, AT_DESK_IDLE.to_owned()),
            },
        }
    }

    fn coworker_card(&self, nick: &str, now: DateTime<Utc>) -> CoworkerCard {
        let name = self.coworker_name(nick);
        let job = self.coworker_job(nick);
        let (desk, state) = self.desk_state(nick, now);
        let label = [Some(name.clone()), job.clone(), Some(state.clone())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
        CoworkerCard {
            nick: fold(nick),
            name,
            job,
            face: self.role_face(nick),
            state,
            desk,
            label,
        }
    }

    /// Everyone's agent sessions their machines name, online or away, that sit at no
    /// coworker's desk, online first, then by name.
    fn hot_desks(&self) -> Vec<HotDesk> {
        let mut desks: Vec<HotDesk> = self
            .named_sessions()
            .into_iter()
            .filter(|(nick, session)| self.desk_of(nick, session).is_none())
            .map(|(nick, session)| {
                let online = self.listening(&nick, &session) == Some(super::Listening::Online);
                let whose = self.whose(&nick).map(|(owner, _)| {
                    let name = self.owner_name(&owner);
                    if name == super::speaker::YOU {
                        YOURS.to_owned()
                    } else {
                        super::speaker::possessive(&name)
                    }
                });
                let title = self.agent_title(&nick, &session);
                let doing = self.agent_handle(&session, &title).map(|_| title);
                let line = [whose, doing]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let avatar = super::Avatar::Agent {
                    tool: self.session_tools.get(&session).cloned(),
                    picture: self.session_pictures.get(&session).cloned(),
                    hue: super::nick_degrees(&session),
                    owner: self
                        .whose(&nick)
                        .map_or_else(|| nick.clone(), |(owner, _)| owner),
                    session: Some(session.clone()),
                };
                HotDesk {
                    conversation: super::agent_channel(&nick, &session),
                    name: self.session_label(&session),
                    line,
                    face: self.face(&avatar),
                    online,
                }
            })
            .collect();
        desks.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.cmp(&b.name)));
        desks
    }

    /// The Office tab, times on a clock `offset` from UTC: see [`Office`].
    #[must_use]
    pub fn office(&self, offset: FixedOffset) -> Office {
        let now = (self.clock)();
        let coworkers: Vec<CoworkerCard> = self
            .coworker_nicks()
            .iter()
            .map(|nick| self.coworker_card(nick, now))
            .collect();
        let seated = coworkers
            .iter()
            .filter(|c| c.desk != DeskState::Empty)
            .count();
        Office {
            waiting: self.waiting_card(None, offset),
            coworkers_heading: COWORKERS.to_owned(),
            count: (!coworkers.is_empty())
                .then(|| format!("{seated} of {} at a desk", coworkers.len())),
            no_coworkers: coworkers.is_empty().then(|| NoCoworkers {
                line: NO_COWORKERS.to_owned(),
                hire: HIRE.to_owned(),
            }),
            coworkers,
            hot_desks_heading: HOT_DESKS.to_owned(),
            hot_desks: self.hot_desks(),
        }
    }

    /// `came` as a row: when, on a clock `offset` from UTC, and what.
    fn came_row(&self, came: &Came, offset: FixedOffset, now: DateTime<Utc>) -> CameRow {
        let at =
            DateTime::from_timestamp(i64::try_from(came.at).unwrap_or_default(), 0).unwrap_or(now);
        let who = came.woke.from.as_deref().map(|from| self.shown_name(from));
        let what = came.woke.words(who.as_deref());
        let text = zirc_proto::clip_line(&came.text, zirc_proto::desk::SHOWN);
        CameRow {
            at: when(at, now, offset),
            text: if text.is_empty() {
                what
            } else {
                format!("{what}: {text}")
            },
        }
    }

    /// Coworker `nick`'s own page, times on a clock `offset` from UTC: `None` for no
    /// coworker of the office.
    #[must_use]
    pub fn coworker_page(&self, nick: &str, offset: FixedOffset) -> Option<CoworkerPage> {
        if !self.coworker_nicks().contains(&fold(nick)) {
            return None;
        }
        let now = (self.clock)();
        let card = self.coworker_card(nick, now);
        let seat = self.role_seat(nick);
        let handover = self.office.handovers.get(&fold(nick));
        let pill = match card.desk {
            DeskState::Working => WORKING.to_owned(),
            _ => card.state.clone(),
        };
        let now_words = match card.desk {
            DeskState::Working => Some(card.state.clone()),
            DeskState::Empty => handover.and_then(|h| h.doing.clone()),
            DeskState::Asked | DeskState::Idle => seat
                .as_ref()
                .and_then(|seat| self.coworker_doing(seat, now))
                .or_else(|| handover.and_then(|h| h.doing.clone())),
        };
        let now_under = handover.and_then(|h| h.recent.last()).map(|came| {
            let row = self.came_row(came, offset, now);
            format!("Woke {} · {}", row.at, row.text)
        });
        let mut about = vec![AboutRow {
            label: JOB.to_owned(),
            text: self.coworker_job_words(nick),
        }];
        about.push(AboutRow {
            label: DESK.to_owned(),
            text: self.desk_words(nick, seat.as_ref(), offset, now),
        });
        if let Some(by) = self.coworker_of(nick).and_then(|c| c.added_by.as_deref()) {
            about.push(AboutRow {
                label: HIRED_BY.to_owned(),
                text: self.shown_name(by),
            });
        }
        let recent = handover
            .map(|h| {
                h.recent
                    .iter()
                    .rev()
                    .map(|came| self.came_row(came, offset, now))
                    .collect()
            })
            .unwrap_or_default();
        Some(CoworkerPage {
            nick: card.nick,
            handle: format!("@{}", fold(nick)),
            message: format!("Message {}", card.name),
            name: card.name,
            job: card.job,
            face: card.face,
            desk: card.desk,
            pill,
            now_heading: NOW.to_owned(),
            now: now_words,
            now_under,
            about,
            recent_heading: RECENT.to_owned(),
            recent,
        })
    }

    /// Coworker `nick`'s desk, on its page: `Claude Code on Henrik's mbp, in web · since
    /// 08:02`, or `Desk empty · 1 waiting · in web`.
    fn desk_words(
        &self,
        nick: &str,
        seat: Option<&Seat>,
        offset: FixedOffset,
        now: DateTime<Utc>,
    ) -> String {
        let repository = seat
            .map(|seat| seat.repository.clone())
            .filter(|repository| !repository.is_empty())
            .or_else(|| self.coworker_of(nick)?.repository.clone());
        let Some(holder) = seat.and_then(|seat| seat.holder.as_ref()) else {
            let empty = zirc_proto::desk::empty(seat.map_or(0, |seat| seat.waiting));
            return match repository {
                Some(repository) => format!("{empty} · in {repository}"),
                None => format!("{empty} · no session has sat here yet"),
            };
        };
        let tool = holder
            .session
            .tool
            .as_deref()
            .map_or_else(|| holder.session.who().to_owned(), zirc_proto::tool_name);
        let place = repository.map(|r| format!(", in {r}")).unwrap_or_default();
        let since = DateTime::from_timestamp(i64::try_from(holder.since).unwrap_or_default(), 0)
            .filter(|_| holder.since > 0)
            .map(|at| format!(" · since {}", when(at, now, offset)))
            .unwrap_or_default();
        format!(
            "{tool} on {}{place}{since}",
            self.machine_said(&holder.machine)
        )
    }

    /// The sheet coworker `nick`'s empty desk opens, times on a clock `offset` from UTC:
    /// `None` while a session sits there, or for no coworker of the office.
    #[must_use]
    pub fn empty_desk(&self, nick: &str, offset: FixedOffset) -> Option<EmptyDesk> {
        if !self.coworker_nicks().contains(&fold(nick)) {
            return None;
        }
        let seat = self.role_seat(nick);
        if seat.as_ref().is_some_and(|seat| seat.holder.is_some()) {
            return None;
        }
        let now = (self.clock)();
        let name = self.coworker_name(nick);
        let handover = self.office.handovers.get(&fold(nick));
        let waiting: Vec<CameRow> = handover
            .map(|h| {
                h.waiting
                    .iter()
                    .map(|came| self.came_row(came, offset, now))
                    .collect()
            })
            .unwrap_or_default();
        let count = seat
            .as_ref()
            .map_or(0, |seat| usize::try_from(seat.waiting).unwrap_or_default())
            .max(waiting.len());
        let last = seat
            .as_ref()
            .and_then(|seat| seat.last.as_ref())
            .and_then(|last| DateTime::from_timestamp(i64::try_from(last.since).ok()?, 0))
            .map_or_else(
                || "no session has sat here yet".to_owned(),
                |at| format!("last session ended {}", when(at, now, offset)),
            );
        let under = match self.coworker_job(nick) {
            Some(job) => format!("{job} · {last}"),
            None => capitalised(&last),
        };
        let repository = seat
            .as_ref()
            .map(|seat| seat.repository.clone())
            .filter(|repository| !repository.is_empty())
            .or_else(|| self.coworker_of(nick)?.repository.clone());
        let start = "On your computer, start a Claude Code or Codex session in ";
        let how = match repository {
            Some(repository) => vec![
                OfficeSpan {
                    text: start.to_owned(),
                    mono: false,
                },
                OfficeSpan {
                    text: repository,
                    mono: true,
                },
                OfficeSpan {
                    text: " and say:".to_owned(),
                    mono: false,
                },
            ],
            None => vec![OfficeSpan {
                text: format!("{start}the repository {name} is to work in and say:"),
                mono: false,
            }],
        };
        let after = if count == 0 {
            format!("{name} sits down with what the last session was doing.")
        } else {
            format!(
                "{name} picks up what waits on sitting down, with what the last session was doing."
            )
        };
        Some(EmptyDesk {
            nick: fold(nick),
            face: self.role_face(nick),
            title: format!("{name}'s desk is empty"),
            under,
            waiting_heading: (count > 0).then(|| format!("{count} waiting for {name}")),
            waiting,
            how,
            sentence: zirc_proto::coworkers::tell_a_session(&name),
            copy: COPY.to_owned(),
            after,
            message: format!("Message {name}"),
            done: DONE.to_owned(),
            foot: format!("A message waits too, and reaches {name} at the desk."),
        })
    }
}

/// `at` on a clock `offset` from UTC, as a card says it beside `now`: `09:12` today, `Thu
/// 18:30` before.
fn when(at: DateTime<Utc>, now: DateTime<Utc>, offset: FixedOffset) -> String {
    profile::card_time(
        at.with_timezone(&offset).naive_local(),
        now.with_timezone(&offset).naive_local(),
    )
}

/// `words` with a capital first letter.
fn capitalised(words: &str) -> String {
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}
