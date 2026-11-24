//! Roles: standing jobs an agent session takes, such as the fixer (the user's choices, 24 Nov
//! 2026). An organisation's admins define each on zirc.dev ([`Definition`]): its name, which
//! is its nick on the server and unique in the organisation, its one home repository, what it
//! does and what wakes it ([`crate::trigger`]). zirc.dev syncs them to the server whole
//! ([`Listing`]), and the server's sidecar keeps each role's nick connected.
//!
//! A role has one seat, taken only by command, from a checkout of its repository: `zirc role
//! take fixer`. The seat is a lease ([`crate::leases`]) named [`lease`], agreed as every lease
//! is, which the sidecar hears too: so who holds a role ([`Seat`]) is worked out the same way
//! everywhere, and a session that leaves, or whose watch goes, frees it.
//!
//! A direct message to a role's nick wakes its holder, as does anything its triggers or a
//! report match: the sidecar sends the wake on to the holder's machine ([`Wake`],
//! [`WAKE_KIND`]), whose zircd wakes the session with a line ending in the role, and what the holder says as the
//! role ([`Say`], [`SAY_KIND`]) goes out from the role's nick. To a vacant role the message
//! waits for the next holder, and only its sender is told how to fill it ([`VACANT_KIND`],
//! [`Vacant`]). When a seat is taken or left, the role's nick says so in the agents' room
//! ([`SEAT_KIND`]).
//!
//! Every word apps show for a role is here, so none says it its own way.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::is_zero;
use crate::trigger::Trigger;

/// The seats a role has: one, until one proves too few (crews wait).
pub const SEATS: u8 = 1;

/// The longest a role's brief is, in characters: room for how a job is done here, as the
/// fixer's carries what the four workflows it replaced said.
pub const MAX_BRIEF: usize = 8000;

/// Where zirc.dev gives a server its organisation's roles, whole ([`Listing`], `PUT`), and
/// reads how they stand ([`Standing`], `GET`), on the sidecar's accounts API, which only
/// zirc.dev reaches.
pub const ACCOUNTS_PATH: &str = "/v1/roles";

/// The command that takes a role, before the role's name: `zirc role take`.
pub const TAKE_COMMAND: &str = "zirc role take";

/// The most triggers a role has.
pub const MAX_TRIGGERS: usize = 16;

/// The most roles an organisation has.
pub const MAX_ROLES: usize = 32;

/// What the lease of a role's seat is named before the role's name ([`lease`]).
const LEASE_PREFIX: &str = "role:";

/// The custom event kind a role's holder is woken with, from the role's nick to the
/// holder's machine ([`Wake`]).
pub const WAKE_KIND: &str = "x-zirc/role-wake";

/// The custom event kind a holder says something as the role with, to the role's nick
/// ([`Say`]).
pub const SAY_KIND: &str = "x-zirc/role-say";

/// The custom event kind a vacant role answers a direct message with, to its sender alone
/// ([`Vacant`]).
pub const VACANT_KIND: &str = "x-zirc/role-vacant";

/// The custom event kind a role's nick says a seat was taken or left with, in the agents'
/// room and to whoever wrote to it ([`SeatChange`]).
pub const SEAT_KIND: &str = "x-zirc/role-seat";

/// A role as an organisation's admins define it on zirc.dev.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleDefinition")]
pub struct Definition {
    /// Its name, which is its nick on the server: unique in the organisation ([`check_name`]),
    /// such as `fixer`.
    pub name: String,
    /// The repository its seat is taken from a checkout of, by name as zirc spells it
    /// ([`crate::Message::repo`]), such as `monorepo`.
    pub repository: String,
    /// What it does, given to whoever takes it, as zirc.dev's "What it does" says.
    pub brief: String,
    /// What wakes its holder, besides a direct message, which always does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    /// How many seats it has: [`SEATS`].
    #[serde(default = "one_seat")]
    pub seats: u8,
    /// The kinds of report it takes ([`crate::role_report::Subject`]), as its admins chose
    /// them on zirc.dev's Roles tab.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<crate::role_report::Subject>,
}

fn one_seat() -> u8 {
    SEATS
}

/// An organisation's roles, as zirc.dev syncs them to its server: whole each time, with a
/// version that changes whenever they do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleListing")]
pub struct Listing {
    /// Every role, in the order zirc.dev lists them.
    pub roles: Vec<Definition>,
    /// What they're at: anything that changes when they do.
    pub version: String,
}

impl Listing {
    /// The role named `name`, if there's one.
    #[must_use]
    pub fn role(&self, name: &str) -> Option<&Definition> {
        self.roles.iter().find(|role| role.name == name)
    }

    /// The rules the listing keeps: each role's, at most [`MAX_ROLES`], no name twice.
    ///
    /// # Errors
    ///
    /// What's wrong, naming the role.
    pub fn check(&self) -> Result<(), String> {
        if self.roles.len() > MAX_ROLES {
            return Err(format!("an organisation has at most {MAX_ROLES} roles"));
        }
        let mut names = std::collections::BTreeSet::new();
        for role in &self.roles {
            role.check()
                .map_err(|why| format!("{}: {why}", role.name))?;
            if !names.insert(role.name.as_str()) {
                return Err(format!(
                    "{} is named twice: a role's name is unique",
                    role.name
                ));
            }
        }
        Ok(())
    }
}

/// Where the server's content API answers how its roles stand ([`Standing`]), under its
/// `/api/v1`: for the organisation's people and their machines, and zirc.dev's Roles tab.
pub const PATH: &str = "/roles";

/// How an organisation's roles stand, as its server answers at [`PATH`]: each role, and its
/// seat, in the listing's order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleStanding")]
pub struct Standing {
    /// The roles, as zirc.dev last synced them.
    pub listing: Listing,
    /// Each role's seat, in the same order.
    pub seats: Vec<Seat>,
}

/// A role's seat as zirc.dev's Roles tab draws it, filled by the server as the apps fill
/// theirs (zirc-chat's `role_face` and `holder_machine`), so the dashboard draws the same
/// face, colour and machine without a rule of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "RoleSeatShown")]
pub struct SeatShown {
    /// The seat.
    #[serde(flatten)]
    pub seat: Seat,
    /// Its picture, one of [`crate::AGENT_AVATARS`] ([`crate::agent_picture`]).
    pub picture: String,
    /// Its colour, a hue in degrees, as every app colours its nick.
    pub hue: u16,
    /// The holder's machine as people read it: `Henrik's mbp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_machine: Option<String>,
    /// The last holder's machine, the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_machine: Option<String>,
}

/// How an organisation's roles stand, as its server answers zirc.dev at [`ACCOUNTS_PATH`]:
/// [`Standing`] with each seat as the Roles tab draws it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleStandingShown")]
pub struct StandingShown {
    /// The roles, as zirc.dev last synced them.
    pub listing: Listing,
    /// Each role's seat, in the same order.
    pub seats: Vec<SeatShown>,
}

impl Standing {
    /// Role `name` and its seat, if there's one.
    #[must_use]
    pub fn role(&self, name: &str) -> Option<(&Definition, Option<&Seat>)> {
        let role = self.listing.role(name)?;
        Some((role, self.seats.iter().find(|seat| seat.role == name)))
    }
}

impl Definition {
    /// The rules a role keeps that need nothing but itself.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for the admin who wrote it.
    pub fn check(&self) -> Result<(), String> {
        if let Some(why) = crate::names::role_problem(&self.name) {
            return Err(why);
        }
        if self.repository.trim().is_empty() {
            return Err("a role has a repository its seat is taken in".to_owned());
        }
        if self.brief.trim().is_empty() || self.brief.chars().count() > MAX_BRIEF {
            return Err(format!(
                "what a role does is some words, {MAX_BRIEF} characters at most"
            ));
        }
        if self.seats != SEATS {
            return Err(format!("a role has {SEATS} seat"));
        }
        if self.triggers.len() > MAX_TRIGGERS {
            return Err(format!("a role wakes on at most {MAX_TRIGGERS} things"));
        }
        for trigger in &self.triggers {
            trigger.check()?;
            if !trigger.inputs.is_empty() || !trigger.with.is_empty() {
                return Err(
                    "a role's trigger has no inputs or with: they were a workflow's".into(),
                );
            }
        }
        Ok(())
    }

    /// What wakes it, as people read it after "Wakes on": its triggers', then a direct
    /// message, which always does.
    #[must_use]
    pub fn wakes_on(&self) -> Vec<String> {
        self.triggers
            .iter()
            .map(Trigger::words)
            .chain([WAKES_ON_MESSAGE.to_owned()])
            .collect()
    }
}

/// What every role wakes on, as [`Definition::wakes_on`] ends.
pub const WAKES_ON_MESSAGE: &str = "a direct message";

/// The lease that is role `name`'s seat.
#[must_use]
pub fn lease(name: &str) -> String {
    format!("{LEASE_PREFIX}{name}")
}

/// The role whose seat lease `name` is, if it's one's.
#[must_use]
pub fn of_lease(name: &str) -> Option<&str> {
    name.strip_prefix(LEASE_PREFIX)
}

/// A session holding a role, or that held it: its machine, and the session as its card says
/// it ([`crate::session::SessionCard`]), so its name and tool are read one way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleHolder")]
pub struct Holder {
    /// Its machine, by nick.
    pub machine: String,
    /// Since when it held it, or until when it did for the last holder: unix seconds.
    pub since: u64,
    /// The session, its title empty where its machine hasn't said one.
    #[serde(flatten)]
    pub session: crate::session::SessionCard,
}

impl Holder {
    /// Its name with its tool, as "Last held by" says it: `main-fix (Claude Code)`.
    #[must_use]
    pub fn who_with_tool(&self) -> String {
        match &self.session.tool {
            Some(tool) => format!("{} ({})", self.session.who(), crate::tool_name(tool)),
            None => self.session.who().to_owned(),
        }
    }
}

/// How a role stands: who holds its seat, if anyone, and who did last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "RoleSeat")]
pub struct Seat {
    /// The role.
    pub role: String,
    /// Its repository.
    pub repository: String,
    /// Who holds it; vacant without.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<Holder>,
    /// Who held it before, for "Last held by" and `zirc role last`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<Holder>,
    /// How many direct messages wait for the next holder.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub waiting: u32,
    /// What wakes it, as people read it ([`Definition::wakes_on`]): said here for readers
    /// that can't run this crate, such as zirc.dev's Roles tab.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wakes_on: Vec<String>,
    /// How many times it woke today, UTC: by a direct message, a trigger or a report.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub woke_today: u32,
}

impl Seat {
    /// Under the role's name, in its conversation's header and among the members: `held by
    /// main-fix on Henrik's MacBook`, `Vacant in monorepo`. `machine` is the holder's
    /// machine as the reader's app names it.
    #[must_use]
    pub fn status(&self, machine: &str) -> String {
        match &self.holder {
            Some(holder) => format!("held by {} on {machine}", holder.session.who()),
            None => format!("Vacant in {}", self.repository),
        }
    }

    /// A member row's second line: `main-fix · Henrik's MacBook`, or `vacant`.
    #[must_use]
    pub fn member_line(&self, machine: &str) -> String {
        match &self.holder {
            Some(holder) => format!("{} · {machine}", holder.session.who()),
            None => VACANT.to_owned(),
        }
    }
}

/// A vacant role's member line.
pub const VACANT: &str = "vacant";

/// The private conversation's header line before the seat's status ([`Seat::status`]).
pub const PRIVATE: &str = "Private to you";

/// A role as it's addressed, after `Message` in its conversation's composer: `the fixer`.
#[must_use]
pub fn addressed(role: &str) -> String {
    format!("the {role}")
}

/// That `who` left role `role`: `main-fix left the fixer role`, as a seat left is said
/// everywhere ([`SeatChange::words`], zircd's lease event).
#[must_use]
pub fn left_words(who: &str, role: &str) -> String {
    format!("{who} left the {role} role")
}

/// The command that takes role `role`.
#[must_use]
pub fn take_command(role: &str) -> String {
    format!("{TAKE_COMMAND} {role}")
}

/// A role's holder woken, by a direct message to the role, a trigger or a report: what a
/// [`WAKE_KIND`] event carries, from the role's nick to the holder's machine. Its text is
/// what woke it: a direct message's words, or the wake's ([`crate::woke::Woke::words`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    /// The holder's session it's for.
    pub session: String,
    /// What woke it, its [`crate::woke::Woke::role`] the role: for a direct message,
    /// [`crate::woke::What::Wrote`] from its writer, with its id as the line.
    pub woke: crate::woke::Woke,
    /// When it happened, unix seconds: what waited for the holder is older.
    pub at: u64,
}

impl Wake {
    /// Its event, `text` being what woke it.
    ///
    /// # Panics
    /// Never: it always serialises.
    #[must_use]
    pub fn event(&self, text: &str) -> crate::Event {
        crate::Event::Custom {
            kind: WAKE_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a wake serialises"),
            text: text.to_owned(),
        }
    }

    /// The wake `event` is, if it's one, with its text.
    #[must_use]
    pub fn of(event: &crate::Event) -> Option<(Self, &str)> {
        match event {
            crate::Event::Custom { kind, data, text } if kind == WAKE_KIND => {
                Some((serde_json::from_value(data.clone()).ok()?, text.as_str()))
            }
            _ => None,
        }
    }
}

/// What a holder says as its role: what a [`SAY_KIND`] event carries, to the role's nick,
/// whose sidecar says it from the role if the session sending it holds the seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Say {
    /// The role.
    pub role: String,
    /// Whom to, by nick: a direct message; without, the agents' room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The message it answers, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

impl Say {
    /// Its event, `text` being what to say.
    ///
    /// # Panics
    /// Never: it always serialises.
    #[must_use]
    pub fn event(&self, text: &str) -> crate::Event {
        crate::Event::Custom {
            kind: SAY_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a say serialises"),
            text: text.to_owned(),
        }
    }

    /// What `event` says as a role, if it's one, with its text.
    #[must_use]
    pub fn of(event: &crate::Event) -> Option<(Self, &str)> {
        match event {
            crate::Event::Custom { kind, data, text } if kind == SAY_KIND => {
                Some((serde_json::from_value(data.clone()).ok()?, text.as_str()))
            }
            _ => None,
        }
    }
}

/// A vacant role's answer to whoever wrote to it: what a [`VACANT_KIND`] event carries, so
/// their app draws the card saying how to fill it ([`Vacant::title`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "RoleVacant")]
pub struct Vacant {
    /// How the role stands.
    pub seat: Seat,
    /// The message it answers, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

impl Vacant {
    /// The card's heading: `Nobody is the fixer in monorepo`.
    #[must_use]
    pub fn title(&self) -> String {
        format!(
            "Nobody is the {} in {}",
            self.seat.role, self.seat.repository
        )
    }

    /// What the card says before the command ([`take_command`]).
    #[must_use]
    pub fn body(&self) -> String {
        format!(
            "Your message waits here for whoever takes the role next. To fill it, tell an \
             agent in a {} checkout to take it, or run:",
            self.seat.repository
        )
    }

    /// The command the card shows, with a Copy button.
    #[must_use]
    pub fn command(&self) -> String {
        take_command(&self.seat.role)
    }

    /// The card's last line: `Last held by main-fix (Claude Code) on Henrik's MacBook; the
    /// next holder can ask it. Nobody else is told.`, or without a last holder, `Nobody else
    /// is told.`. `machine` is the last holder's machine as the reader's app names it.
    #[must_use]
    pub fn footer(&self, machine: Option<&str>) -> String {
        match &self.seat.last {
            Some(last) => format!(
                "Last held by {} on {}; the next holder can ask it. {NOBODY_ELSE}",
                last.who_with_tool(),
                machine.unwrap_or(&last.machine)
            ),
            None => NOBODY_ELSE.to_owned(),
        }
    }

    /// Its event, whose text plain IRC clients show.
    ///
    /// # Panics
    /// Never: it always serialises.
    #[must_use]
    pub fn event(&self) -> crate::Event {
        crate::Event::Custom {
            kind: VACANT_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a vacant answer serialises"),
            text: format!("{}. {} {}", self.title(), self.body(), self.command()),
        }
    }

    /// The answer `event` is, if it's one.
    #[must_use]
    pub fn of(event: &crate::Event) -> Option<Self> {
        match event {
            crate::Event::Custom { kind, data, .. } if kind == VACANT_KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }
}

/// The vacant card's last words.
const NOBODY_ELSE: &str = "Nobody else is told.";

/// Under a message to a vacant role, where a sent message says it was seen: `Waiting: nobody
/// holds the fixer role`.
#[must_use]
pub fn waiting(role: &str) -> String {
    format!("Waiting: nobody holds the {role} role")
}

/// A seat taken or left: what a [`SEAT_KIND`] event carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleSeatChange")]
pub struct SeatChange {
    /// The role.
    pub role: String,
    /// Its repository.
    pub repository: String,
    /// Who took or left it.
    pub holder: Holder,
    /// Whether they took it, rather than left it.
    pub took: bool,
}

impl SeatChange {
    /// Its line, in the agents' room and the role's conversations: `main-fix took the fixer
    /// role in monorepo · zirc role take fixer`, `main-fix left the fixer role`. What plain
    /// IRC clients show; apps say whose the session is ([`SeatChange::words_with`]).
    #[must_use]
    pub fn words(&self) -> String {
        self.words_with(None)
    }

    /// Its line, `whose` being the session's owner as the reader's app says it, possessive:
    /// `main-fix (Henrik's) took the fixer role in monorepo · zirc role take fixer`. Zirc
    /// can't know who asked the session to take it, so it names whose the session is.
    #[must_use]
    pub fn words_with(&self, whose: Option<&str>) -> String {
        let who = match whose {
            Some(whose) => format!("{} ({whose})", self.holder.session.who()),
            None => self.holder.session.who().to_owned(),
        };
        if self.took {
            format!(
                "{who} took the {} role in {} · {}",
                self.role,
                self.repository,
                take_command(&self.role)
            )
        } else {
            left_words(&who, &self.role)
        }
    }

    /// Its line in its session's owner's own conversation with the role: `your main-fix took
    /// the role`, `your main-fix left the role`.
    #[must_use]
    pub fn words_yours(&self) -> String {
        let did = if self.took { "took" } else { "left" };
        format!("your {} {did} the role", self.holder.session.who())
    }

    /// Its event.
    ///
    /// # Panics
    /// Never: it always serialises.
    #[must_use]
    pub fn event(&self) -> crate::Event {
        crate::Event::Custom {
            kind: SEAT_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a seat change serialises"),
            text: self.words(),
        }
    }

    /// The change `event` is, if it's one.
    #[must_use]
    pub fn of(event: &crate::Event) -> Option<Self> {
        match event {
            crate::Event::Custom { kind, data, .. } if kind == SEAT_KIND => {
                serde_json::from_value(data.clone()).ok()
            }
            _ => None,
        }
    }
}

/// Whether `kind` is one of roles' own: none wakes a watch as a post would.
#[must_use]
pub fn is_kind(kind: &str) -> bool {
    [WAKE_KIND, SAY_KIND, VACANT_KIND, SEAT_KIND].contains(&kind)
}

/// The TypeScript the web app reads roles as.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    crate::trigger::typescript(config)
        .into_iter()
        .chain([
            Definition::decl(config),
            Listing::decl(config),
            Holder::decl(config),
            Seat::decl(config),
            Vacant::decl(config),
            SeatChange::decl(config),
            Standing::decl(config),
            SeatShown::decl(config),
            StandingShown::decl(config),
            format!("const ROLE_SEATS = {SEATS};"),
            format!("const ROLE_MAX_BRIEF = {MAX_BRIEF};"),
            format!("const TAKE_COMMAND = {TAKE_COMMAND:?};"),
            format!("const ROLES_ACCOUNTS_PATH = {ACCOUNTS_PATH:?};"),
        ])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_take_command_is_zircs() {
        assert!(TAKE_COMMAND.starts_with(&format!("{} ", crate::PROGRAM)));
        assert_eq!(take_command("fixer"), "zirc role take fixer");
    }

    fn fixer() -> Definition {
        serde_json::from_value(serde_json::json!({
            "name": "fixer",
            "repository": "monorepo",
            "brief": "Owns a broken master and flaky tests.",
            "triggers": [{"state": "master", "level": "broken"}],
        }))
        .unwrap()
    }

    fn holder() -> Holder {
        Holder {
            machine: "henrik-mbp".into(),
            since: 1,
            session: crate::session::SessionCard {
                session: "cc-1".into(),
                name: Some("main-fix".into()),
                title: String::new(),
                tool: Some(crate::CLAUDE_CODE.into()),
            },
        }
    }

    #[test]
    fn a_definition_has_one_seat_and_wakes_on_a_message_too() {
        let role = fixer();
        assert_eq!(role.seats, 1);
        assert_eq!(role.check(), Ok(()));
        assert_eq!(role.wakes_on(), ["master is broken", "a direct message"]);
        let listing = Listing {
            roles: vec![role.clone(), role],
            version: "1".into(),
        };
        assert!(listing.check().unwrap_err().contains("named twice"));
    }

    #[test]
    fn names_are_shaped_as_handles_and_not_zircs() {
        let problem = crate::names::role_problem;
        assert_eq!(problem("fixer"), None);
        assert_eq!(problem("web_fixer"), None);
        assert!(problem("Fixer").is_some());
        assert!(problem("f").is_some());
        assert!(problem("zirc").is_some());
        let mut role = fixer();
        role.seats = 2;
        assert!(role.check().is_err());
    }

    #[test]
    fn the_seat_is_a_lease() {
        assert_eq!(lease("fixer"), "role:fixer");
        assert_eq!(of_lease("role:fixer"), Some("fixer"));
        assert_eq!(of_lease("emulator"), None);
    }

    #[test]
    fn a_vacant_role_says_how_to_fill_it_as_the_mocks_do() {
        let vacant = Vacant {
            seat: Seat {
                role: "fixer".into(),
                repository: "monorepo".into(),
                holder: None,
                last: Some(holder()),
                waiting: 1,
                wakes_on: Vec::new(),
                woke_today: 0,
            },
            reply_to: None,
        };
        assert_eq!(vacant.title(), "Nobody is the fixer in monorepo");
        assert_eq!(vacant.command(), "zirc role take fixer");
        assert_eq!(
            vacant.footer(Some("Henrik's MacBook")),
            "Last held by main-fix (Claude Code) on Henrik's MacBook; the next holder can \
             ask it. Nobody else is told."
        );
        assert_eq!(vacant.seat.status("x"), "Vacant in monorepo");
        assert_eq!(Vacant::of(&vacant.event()), Some(vacant));
        assert_eq!(waiting("fixer"), "Waiting: nobody holds the fixer role");
    }

    #[test]
    fn a_held_seat_names_its_holder() {
        let seat = Seat {
            role: "fixer".into(),
            repository: "monorepo".into(),
            holder: Some(holder()),
            last: None,
            waiting: 0,
            wakes_on: Vec::new(),
            woke_today: 0,
        };
        assert_eq!(
            seat.status("Henrik's MacBook"),
            "held by main-fix on Henrik's MacBook"
        );
        assert_eq!(
            seat.member_line("Henrik's MacBook"),
            "main-fix · Henrik's MacBook"
        );
        let change = SeatChange {
            role: "fixer".into(),
            repository: "monorepo".into(),
            holder: holder(),
            took: true,
        };
        assert_eq!(
            change.words_with(Some("Henrik's")),
            "main-fix (Henrik's) took the fixer role in monorepo · zirc role take fixer"
        );
        assert_eq!(change.words_yours(), "your main-fix took the role");
        assert_eq!(SeatChange::of(&change.event()), Some(change));
    }
}
