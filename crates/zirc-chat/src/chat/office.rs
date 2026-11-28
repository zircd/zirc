//! The office: the organisation's coworkers at their desks and everyone's sessions at hot
//! desks, as every app's Office tab draws it (the user's choices, 27 Nov 2026: a fifth tab,
//! first; a card per coworker with its character, job and one state line; everyone's
//! hot-desk sessions under them, each with whose it is). A question an agent asked the
//! person shows on its asker's row, and a tap opens it ([`OfficeQuestion`]); nothing waits
//! pinned atop the Office (the user's choices, 28 Nov 2026: zirc leans on watching what
//! agents do and stepping in, not on answering them). A coworker's own page and the sheet
//! of an empty desk are here too. Every word and rule is here; apps only draw.
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
use zirc_proto::coworkers::{Coworker, Staff};
use zirc_proto::desk::{Came, Handover};
use zirc_proto::roles::{Look, Seat, Standing};

use super::face::Face;
use super::waiting::{Asked, MORE_ANSWERS, WaitingAction, WaitingItem};
use super::{Chat, conversation::fold, profile};

/// The Office tab's name, in the tab bar.
pub const OFFICE: &str = "Office";

/// What heads the coworkers' cards (zirc-proto's, as zirc.dev names its tab so).
pub const COWORKERS: &str = zirc_proto::coworkers::COWORKERS;

/// What heads everyone's sessions that sit at no coworker's desk.
pub const HOT_DESKS: &str = zirc_proto::hot_desks::HEADING;

/// A coworker's state while a session sits at its desk with nothing in progress (the user's
/// choice, 27 Nov 2026).
pub const AT_DESK_IDLE: &str = "At desk, idle";

/// A coworker's state while one of its questions waits for the person.
pub const ASKED_YOU: &str = zirc_proto::home::ASKED_YOU;

/// What opens the asking session's conversation, on an opened question.
pub const OPEN: &str = "Open";

/// A coworker's pill on its page while it's on something.
pub const WORKING: &str = "Working";

/// An office with no coworkers yet, in place of their cards (the user's choice, 27 Nov 2026).
pub const NO_COWORKERS: &str = zirc_proto::home::NO_COWORKERS;

/// The button under [`NO_COWORKERS`], which opens zirc.dev's hire page.
pub const HIRE: &str = zirc_proto::home::HIRE_FIRST;

/// What heads what a coworker's on, on its page.
pub const NOW: &str = "Now";

/// What heads what lately came for a coworker, on its page.
pub const RECENT: &str = "Recent";

/// A coworker's page's row for its job.
pub const JOB: &str = "Job";

/// A coworker's page's row for its desk.
pub const DESK: &str = "Desk";

/// A coworker's page's row for who hired it.
pub const HIRED_BY: &str = zirc_proto::coworkers::HIRED_BY;

/// A coworker's job, on its page, while it has none: what such a coworker does, as the brief
/// of the role it amounts to says it (zirc-proto's `coworkers::NO_JOB_BRIEF`).
pub const NO_JOB: &str = zirc_proto::coworkers::NO_JOB_BRIEF;

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

/// zirc.dev's page at `web` for coworker `nick` of the organisation `org`, at its form for
/// its name: what [`CoworkerCard::give_a_name`] opens.
#[must_use]
pub fn coworker_web_page(web: &str, org: &str, nick: &str) -> String {
    format!(
        "{}{}",
        web.trim_end_matches('/'),
        zirc_proto::coworkers::COWORKER_NAME_PATH
            .replace("{org}", org)
            .replace("{name}", &fold(nick))
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
    /// The office's name, atop every badge: `Home`, the organisation's ([`Chat::org_title`]),
    /// or [`OFFICE`] before it's known.
    pub name: String,
    /// What a coworker's badge says it is, beside the office's name:
    /// [`zirc_proto::coworkers::STAFF`].
    pub staff: String,
    /// What a hot desk's badge says it is: [`zirc_proto::coworkers::HOT_DESK`].
    pub hot_desk: String,
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
    /// Everyone's sessions at hot desks while they're in the office: online, or away less
    /// than five minutes ([`zirc_proto::hot_desks::in_office`]), online first.
    pub hot_desks: Vec<HotDesk>,
    /// The line the rest fold under, `29 away`, which opens to show them; `None` while none
    /// are.
    pub hot_desks_away: Option<String>,
    /// The rest, by name, shown once [`Office::hot_desks_away`] is opened.
    pub hot_desks_folded: Vec<HotDesk>,
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

/// A coworker's card in the office, drawn as its office badge (the user's choice, 27 Nov 2026):
/// clipped on, [`Office::name`] and [`Office::staff`] across the top, its character on its
/// colour, its name large, its job in small capitals, [`CoworkerCard::handle`] and a strip of
/// its state at the bottom, toned by [`CoworkerCard::desk`]: green working, amber asked or
/// empty, an empty desk's badge edged amber with its character faded, plain idle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CoworkerCard {
    /// Its nick, `john`: what its page and its conversation are found by.
    pub nick: String,
    /// The name it's shown by: `John`, or its job's, `Fixer`, while it has none of its own
    /// ([`Coworker::badge_name`]).
    pub name: String,
    /// Its job, as people read it: `Fixer`. `None` with none, or while it's the name.
    pub job: Option<String>,
    /// Under the job, in mono: `@john · hired 27 Nov` ([`zirc_proto::coworkers::badge_handle`]).
    pub handle: String,
    /// While it has no name of its own, the link that opens its page on zirc.dev
    /// ([`coworker_web_page`]), where one is set: [`zirc_proto::coworkers::GIVE_A_NAME`].
    pub give_a_name: Option<String>,
    /// Its character on its colour: the full figure on a card, dimmed while its desk is
    /// empty.
    pub face: Face,
    /// Its one state line: `Fixing build 431`, `Asked you · Land John's checkout fix?`,
    /// `At desk, idle`, `Desk empty · 1 waiting`.
    pub state: String,
    /// How its desk stands.
    pub desk: DeskState,
    /// What its session asked the person, which a tap on the card opens in place of its
    /// page: `None` while nothing waits ([`DeskState::Asked`] while something does).
    pub question: Option<OfficeQuestion>,
    /// What a screen reader says of the card: `John, Fixer, Fixing build 431`.
    pub label: String,
}

/// A session at a hot desk, in the office, drawn as a temporary badge: as a coworker's, but
/// dashed, [`Office::hot_desk`] atop it, its dealt character dimmed on grey, its name in mono,
/// [`HotDesk::whose`] under it and [`HotDesk::doing`] in the strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct HotDesk {
    /// Its conversation, `henrik-mbp/cc-1`, which a tap opens.
    pub conversation: String,
    /// The name it goes by, in mono: `checkout-form`, or its id.
    pub name: String,
    /// Whose it is and its tool: `Henrik's · Claude Code`, `Yours · Codex`.
    pub whose: String,
    /// What it's doing, in the badge's strip: `Changing the payment form`, or `Asked you ·
    /// Revoke last month's TLS certificate?` while [`HotDesk::question`] waits. `None` while
    /// nothing's said.
    pub doing: Option<String>,
    /// What it asked the person, which a tap opens in place of its conversation, its strip
    /// drawn in the warning colour: `None` while nothing waits.
    pub question: Option<OfficeQuestion>,
    /// What a screen reader says of the badge: `checkout-form, a session at a hot desk,
    /// Henrik's · Claude Code, Changing the payment form`.
    pub label: String,
    /// Its dealt character, on the muted ground, without its owner's small face: whose it is
    /// is [`HotDesk::whose`].
    pub face: Face,
    /// Whether a watch of its is open: its dot.
    pub online: bool,
}

/// A question an agent asked the person, opened from its asker's row on the Office, as a
/// sheet (the user's choices, 28 Nov 2026): the asker, when it asked, the question, what yes
/// and no do and its note, a note when the session has kept working since, then its answers,
/// its ⋯ menu and Open. The Office's rows have no answers of their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct OfficeQuestion {
    /// What waits, as Waiting for you has it: its key answers it ([`Chat::answer_waiting`]);
    /// its face, question, what each answer does, its note, its buttons and its ⋯ menu are
    /// drawn as they are. A question put directly has no buttons here: [`OfficeQuestion::open`]
    /// opens its conversation to answer in.
    pub item: WaitingItem,
    /// The asker, as its row names it: `Sam`, `tls-rotation`.
    pub name: String,
    /// Whether [`OfficeQuestion::name`] is set in mono, as a hot desk's is.
    pub mono: bool,
    /// Beside the name, muted: the coworker's job, `Reviewer`, or whose the session is and
    /// its tool, `Yours · Claude Code`.
    pub under: String,
    /// Under the name, in the warning colour: `Asked you 25 min ago`.
    pub asked: String,
    /// Over the answers, while the session has kept working since it asked (pushed, said a
    /// new task): `It has kept working since asking: 2 pushes, now on Rotate the staging
    /// certificates. If you answered in the session, it'll take this back.`
    pub kept_working: Option<String>,
    /// What the ⋯ button is called: [`MORE_ANSWERS`].
    pub more: String,
    /// The button that opens the asking session's conversation: [`OPEN`].
    pub open: String,
    /// That conversation, `henrik-mbp/cc-1`.
    pub opens: String,
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
    /// Atop the page, as atop its badge: the office's name, `Home`.
    pub office: String,
    /// Beside it: [`zirc_proto::coworkers::STAFF`].
    pub staff: String,
    /// The name it's shown by, large: `John`.
    pub name: String,
    /// Its nick as it's written to and when it was hired, in mono under the name, as its
    /// badge says it: `@john · hired 27 Nov`.
    pub handle: String,
    /// Its job, in a pill: `Fixer`. `None` with none, or while it's the name.
    pub job: Option<String>,
    /// As on its badge ([`CoworkerCard::give_a_name`]).
    pub give_a_name: Option<String>,
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

    pub(super) fn coworker_of(&self, nick: &str) -> Option<&Coworker> {
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
    pub(super) fn coworker_job(&self, nick: &str) -> Option<String> {
        self.coworker_of(nick)?.job_title()
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
                Some(kind) => format!(
                    "{} {} {}.",
                    kind.line(),
                    zirc_proto::coworkers::WAKES_ON,
                    kind.wakes()
                ),
                None => job.brief.clone(),
            },
        }
    }

    /// What the session at coworker `nick`'s desk is on, in words: its live intent or
    /// what it's taking on, else what the desk last handed over while it's seated.
    fn coworker_doing(&self, seat: &Seat, now: DateTime<Utc>) -> Option<String> {
        let holder = seat.holder.as_ref()?;
        self.doing_words(&holder.machine, &holder.session.session, now)
    }

    /// What session `session` on `machine` is on, in words: its live intent's task or
    /// summary, or what it's taking on. `None` with nothing in progress.
    fn doing_words(&self, machine: &str, session: &str, now: DateTime<Utc>) -> Option<String> {
        let doing = self.doing_now(machine, session, now);
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

    /// What of `asked` the session at the desk `seat` asked, the longest waiting first.
    fn seat_asked<'a>(seat: &Seat, asked: &'a [Asked]) -> Option<&'a Asked> {
        let holder = seat.holder.as_ref()?;
        asked_by(asked, &holder.machine, &holder.session.session)
    }

    /// What the opened question of `asked` says at `now`, its asker named `name` (in mono
    /// with `mono`), `under` beside it.
    fn office_question(
        &self,
        asked: &Asked,
        name: String,
        mono: bool,
        under: String,
        now: DateTime<Utc>,
    ) -> OfficeQuestion {
        let mut item = asked.item.clone();
        // Open does what Reply did: there's one way to the conversation.
        item.buttons.retain(|b| b.action != WaitingAction::Reply);
        let ago = u64::try_from((now - asked.at).num_seconds()).unwrap_or_default();
        OfficeQuestion {
            item,
            name,
            mono,
            under,
            asked: zirc_proto::request::asked_you_ago(ago),
            kept_working: self.kept_working(&asked.machine, &asked.session, asked.at, now),
            more: MORE_ANSWERS.to_owned(),
            open: OPEN.to_owned(),
            opens: super::agent_channel(&asked.machine, &asked.session),
        }
    }

    /// Whether session `session` on `machine` has kept working since it asked at `since`,
    /// and in what words ([`zirc_proto::request::kept_working`]): it pushed since, or said a
    /// task it hadn't before (an intent unlike its last, or `taking:`). `None` while it
    /// hasn't, as a session waiting on its answer doesn't.
    fn kept_working(
        &self,
        machine: &str,
        session: &str,
        since: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Option<String> {
        let mut pushes = std::collections::HashSet::new();
        let mut intent_before: Option<(DateTime<Utc>, &str)> = None;
        let mut intents_after: Vec<&str> = Vec::new();
        for entry in self.conversations.iter().flat_map(|c| &c.entries) {
            let super::conversation::EntryKind::Zirc(message) = &entry.kind else {
                continue;
            };
            if message.session != session || !entry.from.eq_ignore_ascii_case(machine) {
                continue;
            }
            match &message.event {
                zirc_proto::Event::Push { head, branch, .. } if entry.time >= since => {
                    // Said in more than one channel, it's one push.
                    pushes.insert((branch.as_str(), head.as_str()));
                }
                zirc_proto::Event::Intent { summary, .. } if entry.time < since => {
                    if intent_before.is_none_or(|(at, _)| at <= entry.time) {
                        intent_before = Some((entry.time, summary.as_str()));
                    }
                }
                zirc_proto::Event::Intent { summary, .. } => intents_after.push(summary.as_str()),
                _ => {}
            }
        }
        let before = intent_before.map(|(_, summary)| summary);
        let new_intent = intents_after.iter().any(|summary| Some(*summary) != before);
        // A claim is known to the second.
        let new_claim = self
            .claim(machine, session)
            .is_some_and(|(_, at)| at.timestamp() >= since.timestamp());
        if pushes.is_empty() && !new_intent && !new_claim {
            return None;
        }
        let now_on = self.doing_words(machine, session, now);
        Some(zirc_proto::request::kept_working(
            pushes.len(),
            now_on.as_deref(),
        ))
    }

    /// How coworker `nick`'s desk stands at `now`, its state line, and what of `asked` its
    /// session asked the person.
    fn desk_state<'a>(
        &self,
        nick: &str,
        asked: &'a [Asked],
        now: DateTime<Utc>,
    ) -> (DeskState, String, Option<&'a Asked>) {
        let seat = self.role_seat(nick);
        match seat.as_ref().filter(|seat| seat.holder.is_some()) {
            None => (
                DeskState::Empty,
                zirc_proto::desk::empty(seat.map_or(0, |seat| seat.waiting)),
                None,
            ),
            Some(seat) => match Self::seat_asked(seat, asked) {
                Some(asked) => (
                    DeskState::Asked,
                    zirc_proto::request::asked_you(&asked.item.question),
                    Some(asked),
                ),
                None => match self.coworker_doing(seat, now) {
                    Some(doing) => (DeskState::Working, doing, None),
                    None => (DeskState::Idle, AT_DESK_IDLE.to_owned(), None),
                },
            },
        }
    }

    /// What coworker `nick`'s badge says under its job, the day it was hired on a clock
    /// `offset` from UTC: `@john · hired 27 Nov`.
    fn badge_handle(&self, nick: &str, offset: FixedOffset, now: DateTime<Utc>) -> String {
        let hired = self
            .coworker_of(nick)
            .and_then(|c| c.hired)
            .and_then(|at| DateTime::from_timestamp(i64::try_from(at).ok()?, 0))
            .map(|at| {
                profile::day_when(
                    at.with_timezone(&offset).naive_local(),
                    now.with_timezone(&offset).naive_local(),
                )
            });
        zirc_proto::coworkers::badge_handle(&fold(nick), hired.as_deref())
    }

    fn coworker_card(
        &self,
        nick: &str,
        asked: &[Asked],
        offset: FixedOffset,
        now: DateTime<Utc>,
    ) -> CoworkerCard {
        let coworker = self.coworker_of(nick);
        let name = coworker.map_or_else(|| fold(nick), Coworker::badge_name);
        let job = coworker.and_then(Coworker::badge_job);
        let give_a_name = coworker
            .filter(|c| !c.named())
            .map(|_| zirc_proto::coworkers::GIVE_A_NAME.to_owned());
        let (desk, state, asked) = self.desk_state(nick, asked, now);
        let question = asked.map(|asked| {
            self.office_question(
                asked,
                name.clone(),
                false,
                job.clone().unwrap_or_default(),
                now,
            )
        });
        let label = [Some(name.clone()), job.clone(), Some(state.clone())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
        CoworkerCard {
            nick: fold(nick),
            name,
            job,
            handle: self.badge_handle(nick, offset, now),
            give_a_name,
            face: self.role_face(nick),
            state,
            desk,
            question,
            label,
        }
    }

    /// Everyone's agent sessions their machines name, online or away, and any other asking
    /// the person, that sit at no coworker's desk, split as [`zirc_proto::hot_desks::hot_desks`] decides: those in the
    /// office, online first, then by name; the line the rest fold under; and the rest.
    fn hot_desks(&self, asked: &[Asked]) -> (Vec<HotDesk>, Option<String>, Vec<HotDesk>) {
        let now = (self.clock)();
        let mut faces = HashMap::new();
        let mut questions: HashMap<String, (&Asked, String)> = HashMap::new();
        // Every session its machine names, and any other asking the person: its question
        // is on its row, so it has one.
        let mut sessions = self.named_sessions();
        for asked in asked {
            let named = sessions.iter().any(|(nick, session)| {
                nick.eq_ignore_ascii_case(&asked.machine) && *session == asked.session
            });
            if !named && !asked.machine.is_empty() {
                sessions.push((asked.machine.clone(), asked.session.clone()));
            }
        }
        let seen: Vec<zirc_proto::hot_desks::Seen> = sessions
            .into_iter()
            .filter(|(nick, session)| self.desk_of(nick, session).is_none())
            .map(|(nick, session)| {
                let (online, away_since) = match self.listening(&nick, &session) {
                    Some(super::Listening::Online) => (true, None),
                    Some(super::Listening::Away { since, .. }) => {
                        (false, u64::try_from(since.timestamp()).ok())
                    }
                    Some(super::Listening::Ended { .. }) | None => (false, None),
                };
                let owner = self.whose(&nick).map(|(owner, _)| self.owner_name(&owner));
                let mine = owner.as_deref() == Some(super::speaker::YOU);
                let title = self.agent_title(&nick, &session);
                let mut doing = self.agent_handle(&session, &title).map(|_| title);
                let asking = asked_by(asked, &nick, &session);
                if let Some(asked) = asking {
                    doing = Some(zirc_proto::request::asked_you(&asked.item.question));
                }
                let tool = self.session_tools.get(&session);
                let avatar = super::Avatar::Agent {
                    tool: tool.cloned(),
                    picture: self.session_pictures.get(&session).cloned(),
                    hue: super::nick_degrees(&session),
                    owner: self
                        .whose(&nick)
                        .map_or_else(|| nick.clone(), |(owner, _)| owner),
                    session: Some(session.clone()),
                };
                let key = super::agent_channel(&nick, &session);
                // Without its owner's small face: whose it is is the badge's own line.
                let mut face = self.face(&avatar);
                if let Face::Agent {
                    owner, owner_photo, ..
                } = &mut face
                {
                    owner.clear();
                    *owner_photo = None;
                }
                faces.insert(key.clone(), face);
                if let Some(asked) = asking {
                    questions.insert(key.clone(), (asked, self.session_label(&session)));
                }
                zirc_proto::hot_desks::Seen {
                    key,
                    session: session.clone(),
                    name: self.session_label(&session),
                    owner: owner.filter(|_| !mine),
                    mine,
                    tool: tool.map(|t| zirc_proto::tool_name(t)),
                    doing,
                    online,
                    away_since,
                    asking: asking.is_some(),
                }
            })
            .collect();
        let desks = zirc_proto::hot_desks::hot_desks(&zirc_proto::hot_desks::Facts {
            seen,
            // Its face is dealt as every session's ([`Chat::face`]); the badge's own isn't read.
            taken: Vec::new(),
            now: u64::try_from(now.timestamp()).unwrap_or_default(),
        });
        let drawn = |badges: Vec<zirc_proto::hot_desks::Badge>| -> Vec<HotDesk> {
            badges
                .into_iter()
                .filter_map(|badge| {
                    let question = questions.get(&badge.key).map(|(asked, name)| {
                        self.office_question(asked, name.clone(), true, badge.whose.clone(), now)
                    });
                    Some(HotDesk {
                        question,
                        face: faces.get(&badge.key)?.clone(),
                        conversation: badge.key,
                        name: badge.name,
                        whose: badge.whose,
                        doing: badge.doing,
                        label: badge.label,
                        online: badge.online,
                    })
                })
                .collect()
        };
        (drawn(desks.shown), desks.away, drawn(desks.folded))
    }

    /// The office's name atop its badges: the organisation's, or [`OFFICE`] before it's
    /// known.
    fn office_name(&self) -> String {
        Some(self.org_title())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| OFFICE.to_owned())
    }

    /// The Office tab, times on a clock `offset` from UTC: see [`Office`].
    #[must_use]
    pub fn office(&self, offset: FixedOffset) -> Office {
        let now = (self.clock)();
        let asked = self.waiting_asked(offset);
        let coworkers: Vec<CoworkerCard> = self
            .coworker_nicks()
            .iter()
            .map(|nick| self.coworker_card(nick, &asked, offset, now))
            .collect();
        let (hot_desks, hot_desks_away, hot_desks_folded) = self.hot_desks(&asked);
        let seated = coworkers
            .iter()
            .filter(|c| c.desk != DeskState::Empty)
            .count();
        Office {
            name: self.office_name(),
            staff: zirc_proto::coworkers::STAFF.to_owned(),
            hot_desk: zirc_proto::coworkers::HOT_DESK.to_owned(),
            coworkers_heading: COWORKERS.to_owned(),
            count: (!coworkers.is_empty())
                .then(|| format!("{seated} of {} at a desk", coworkers.len())),
            no_coworkers: coworkers.is_empty().then(|| NoCoworkers {
                line: NO_COWORKERS.to_owned(),
                hire: HIRE.to_owned(),
            }),
            coworkers,
            hot_desks_heading: HOT_DESKS.to_owned(),
            hot_desks,
            hot_desks_away,
            hot_desks_folded,
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
        let card = self.coworker_card(nick, &self.waiting_asked(offset), offset, now);
        let seat = self.role_seat(nick);
        let handover = self.office.handovers.get(&fold(nick));
        let pill = match card.desk {
            DeskState::Working => WORKING.to_owned(),
            DeskState::Asked => ASKED_YOU.to_owned(),
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
            office: self.office_name(),
            staff: zirc_proto::coworkers::STAFF.to_owned(),
            handle: card.handle,
            message: format!("Message {}", card.name),
            name: card.name,
            job: card.job,
            give_a_name: card.give_a_name,
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

/// What of `asked` session `session` on `machine` asked, the longest waiting first.
fn asked_by<'a>(asked: &'a [Asked], machine: &str, session: &str) -> Option<&'a Asked> {
    asked
        .iter()
        .find(|asked| asked.machine.eq_ignore_ascii_case(machine) && asked.session == session)
}
