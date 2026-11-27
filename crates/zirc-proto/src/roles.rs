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

use std::collections::BTreeMap;

use crate::inbound::Tool;
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

/// Where a tool posts to a role, at its organisation's server's own host
/// (`https://<slug>.irc.zirc.dev/in/fixer/<secret>`), so what it posts never passes zirc.dev:
/// the role's name, then the secret only its admins see. zirc.dev makes the secret, and gives
/// the server only its digest ([`Address`]).
pub const INBOUND_PATH: &str = "/in/{role}/{secret}";

/// The command that takes a role, before the role's name: `zirc role take`.
pub const TAKE_COMMAND: &str = "zirc role take";

/// What a person tells an agent to take a role with, `{role}` its name: what zirc.dev's vacant
/// card gives to copy, and what the agent guide teaches an agent to take a role on
/// ([`tell_an_agent`]).
pub const TELL_AN_AGENT: &str = "Take the {role} role.";

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
    /// For a coworker's desk, the name the coworker is shown by
    /// ([`crate::coworkers::Coworker::shown_name`]): `John`. None for a role of before, shown
    /// by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shown_name: Option<String>,
    /// The repository its seat is taken from a checkout of, by name as zirc spells it
    /// ([`crate::Message::repo`]), such as `monorepo`.
    pub repository: String,
    /// What it does, given to whoever takes it, as zirc.dev's "What it does" says.
    pub brief: String,
    /// What wakes its holder, besides a direct message, which always does. A trigger this
    /// reader can't read, as a newer zirc.dev may say, is left out ([`crate::lenient`]).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "crate::lenient::known_ones"
    )]
    #[ts(as = "Vec<Trigger>")]
    #[schemars(with = "Vec<Trigger>")]
    pub triggers: Vec<Trigger>,
    /// How many seats it has: [`SEATS`].
    #[serde(default = "one_seat")]
    pub seats: u8,
    /// The kinds of report it takes ([`crate::role_report::Subject`]), as its admins chose
    /// them on zirc.dev's Roles tab. A kind this reader doesn't know is left out
    /// ([`crate::lenient`]), as it couldn't take one.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "crate::lenient::known_ones"
    )]
    #[ts(as = "Vec<crate::role_report::Subject>")]
    #[schemars(with = "Vec<crate::role_report::Subject>")]
    pub reports: Vec<crate::role_report::Subject>,
    /// The picture its admins picked for it, one of [`crate::characters::CHARACTERS`] or [`crate::AGENT_AVATARS`]; without one,
    /// the one its name tells ([`Definition::picture`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub picture: Option<String>,
    /// The colour its admins picked for it, a hue in degrees under 360, one of [`HUES`] as
    /// zirc.dev offers them; without one, its name's, as every nick's is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hue: Option<u16>,
}

/// The colours a role's admins pick from, each a hue in degrees with what a screen reader
/// calls it, in the order zirc.dev's picker offers them.
pub const HUES: [(u16, &str); 12] = [
    (0, "red"),
    (30, "orange"),
    (60, "yellow"),
    (90, "lime"),
    (120, "green"),
    (150, "mint"),
    (180, "teal"),
    (210, "azure"),
    (240, "blue"),
    (270, "violet"),
    (300, "magenta"),
    (330, "rose"),
];

/// The METADATA key a role's server says its [`Look`] under, on the role's nick, for apps
/// to draw it as its admins picked: deleted while they've picked nothing.
pub const LOOK_KEY: &str = "zirc.dev/role-look";

/// What a role's admins picked of how it looks: what its server says under [`LOOK_KEY`],
/// as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    /// Its picture, if one's picked ([`Definition::picture`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// Its colour, if one's picked ([`Definition::hue`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<u16>,
}

impl Look {
    /// It as its server says it, or `None` when nothing's picked, which deletes the key.
    #[must_use]
    pub fn said(&self) -> Option<String> {
        (self != &Self::default()).then(|| serde_json::to_string(self).unwrap_or_default())
    }

    /// What `said` under [`LOOK_KEY`] picks: nothing where it doesn't read, or picks what
    /// this reader can't draw.
    #[must_use]
    pub fn of(said: &str) -> Self {
        let look: Self = serde_json::from_str(said).unwrap_or_default();
        Self {
            picture: look
                .picture
                .filter(|picture| crate::characters::is_picture(picture)),
            hue: look.hue.filter(|hue| *hue < 360),
        }
    }

    /// The picture a role named `role` is drawn with: the one picked, else the one its name
    /// tells ([`crate::agent_picture`]).
    #[must_use]
    pub fn picture_of(&self, role: &str) -> &str {
        self.picture
            .as_deref()
            .unwrap_or_else(|| crate::agent_picture(Some(role), Some(role)))
    }
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
    /// Each role's webhook address, as the server knows it: by its secret's digest alone.
    /// The server never says them back ([`Listing::without_addresses`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<Address>,
    /// What they're at: anything that changes when they do, a new secret too.
    pub version: String,
}

/// The tools that sign what they post with a secret of their own, which a role's admins paste
/// on zirc.dev ([`Address::signing`]). GitHub signs with the address's own secret, and any
/// other tool with nothing.
pub const SIGNED_BY_OWN: [Tool; 3] = [Tool::Sentry, Tool::Linear, Tool::Pagerduty];

/// The most characters a tool's signing secret has.
pub const SIGNING_MAX: usize = 128;

/// A role's webhook address ([`INBOUND_PATH`]) as its server is given it: the SHA-256 of its
/// secret, in lowercase hex, never the secret, which zirc.dev keeps for the role's admins. A
/// new secret (Rotate, on zirc.dev) is a new listing, which the server takes at once, and the
/// old one opens nothing from then on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleAddress")]
pub struct Address {
    /// The role's name.
    pub role: String,
    /// The SHA-256 of its secret, in hex ([`Address::digest_of`]).
    pub digest: String,
    /// The secret each tool of [`SIGNED_BY_OWN`] signs its posts with, as the tool gave it to
    /// the role's admins: the server checks those posts' signatures with it, and takes no
    /// unsigned or wrongly signed post from that tool. Never said back
    /// ([`Listing::without_addresses`]). A tool this reader doesn't know is left out.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "crate::lenient::known_keys"
    )]
    #[ts(type = "Partial<Record<InboundTool, string>>")]
    #[schemars(with = "BTreeMap<Tool, String>")]
    pub signing: BTreeMap<Tool, String>,
}

impl Address {
    /// The rules it keeps: signing secrets only for the tools that sign with their own, each
    /// some characters on a line, [`SIGNING_MAX`] at most.
    ///
    /// # Errors
    ///
    /// What's wrong, in words for the admin.
    pub fn check(&self) -> Result<(), String> {
        for (tool, secret) in &self.signing {
            if !SIGNED_BY_OWN.contains(tool) {
                return Err(format!(
                    "{} doesn't sign with a secret of its own",
                    tool.name()
                ));
            }
            if secret.is_empty()
                || secret.chars().count() > SIGNING_MAX
                || secret.chars().any(char::is_control)
            {
                return Err(format!(
                    "{}'s signing secret is 1 to {SIGNING_MAX} characters on a line",
                    tool.name()
                ));
            }
        }
        Ok(())
    }

    /// The secret `tool` signs its posts to this address with, where its admins gave one.
    #[must_use]
    pub fn signing_secret(&self, tool: Tool) -> Option<&str> {
        self.signing.get(&tool).map(String::as_str)
    }

    /// The digest a secret is given as.
    #[must_use]
    pub fn digest_of(secret: &str) -> String {
        use sha2::{Digest as _, Sha256};
        crate::hex(&Sha256::digest(secret.as_bytes()))
    }

    /// Whether `secret` is this address's, compared in time that doesn't say how much of it
    /// matched.
    #[must_use]
    pub fn opens(&self, secret: &str) -> bool {
        let given = Self::digest_of(secret);
        given.len() == self.digest.len()
            && given
                .bytes()
                .zip(self.digest.bytes())
                .fold(0u8, |differ, (a, b)| differ | (a ^ b))
                == 0
    }
}

impl Listing {
    /// The role named `name`, if there's one.
    #[must_use]
    pub fn role(&self, name: &str) -> Option<&Definition> {
        self.roles.iter().find(|role| role.name == name)
    }

    /// Whether `secret` opens the address of the role named `role`.
    #[must_use]
    pub fn opens(&self, role: &str, secret: &str) -> bool {
        self.addresses
            .iter()
            .any(|address| address.role == role && address.opens(secret))
    }

    /// The listing without its addresses, as the server says it back to anyone.
    #[must_use]
    pub fn without_addresses(mut self) -> Self {
        self.addresses.clear();
        self
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
        for address in &self.addresses {
            address
                .check()
                .map_err(|why| format!("{}: {why}", address.role))?;
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
    /// The coworkers and jobs the roles are ([`crate::coworkers`]), each coworker's repository
    /// as the server knows it: the listing is every coworker as a role
    /// ([`crate::coworkers::Staff::desks`]). Empty from a server older than it.
    #[serde(default)]
    pub staff: crate::coworkers::Staff,
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
    /// Its character, one of [`crate::characters::CHARACTERS`], as every app draws it
    /// ([`crate::characters::of_coworkers`]).
    pub picture: String,
    /// Its colour, a hue in degrees, as every app colours its nick.
    pub hue: u16,
    /// The holder's machine as people read it: `Henrik's mbp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_machine: Option<String>,
    /// The last holder's machine, the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_machine: Option<String>,
    /// Its day in words ([`Seat::day`]).
    pub day: String,
    /// Who holds it, as a member row's second line says ([`Seat::member_line`]): `main-fix ·
    /// Henrik's mbp`, or `Desk empty · 1 waiting`. Empty from a server older than it.
    #[serde(default)]
    pub held: String,
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
    /// The coworkers and jobs the roles are ([`Standing::staff`]).
    #[serde(default)]
    pub staff: crate::coworkers::Staff,
    /// When the server last heard each of [`crate::role_report::Subject::reporters`], for
    /// every role that takes its kinds. Empty from a server older than it.
    #[serde(default)]
    pub reporters: Vec<crate::inbound::HeardReporter>,
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
    /// The name it's shown by: a coworker's (`John`), else its own.
    #[must_use]
    pub fn shown(&self) -> &str {
        self.shown_name.as_deref().unwrap_or(&self.name)
    }

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
        }
        if let Some(picture) = &self.picture
            && !crate::characters::is_picture(picture)
        {
            return Err(format!("{picture} isn't a picture a role can have"));
        }
        if self.hue.is_some_and(|hue| hue >= 360) {
            return Err("a role's colour is a hue under 360 degrees".to_owned());
        }
        Ok(())
    }

    /// What its admins picked of how it looks.
    #[must_use]
    pub fn look(&self) -> Look {
        Look {
            picture: self.picture.clone(),
            hue: self.hue,
        }
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
    /// Its repository; empty for a coworker no session has sat at yet
    /// ([`crate::coworkers::Coworker::repository`]).
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
    /// Its day, as the Roles tab's card ends: `Woke 3 times today · 1 message waiting`,
    /// `Not woken today`.
    #[must_use]
    pub fn day(&self) -> String {
        let woke = match self.woke_today {
            0 => "Not woken today".to_owned(),
            n => format!("Woke {} today", crate::times(n)),
        };
        match self.waiting {
            0 => woke,
            1 => format!("{woke} · 1 message waiting"),
            n => format!("{woke} · {n} messages waiting"),
        }
    }

    /// Under the role's name, in its conversation's header and among the members: `held by
    /// main-fix on Henrik's MacBook`, `Vacant in monorepo`. `machine` is the holder's
    /// machine as the reader's app names it.
    #[must_use]
    pub fn status(&self, machine: &str) -> String {
        match &self.holder {
            Some(holder) => format!("held by {} on {machine}", holder.session.who()),
            None if self.repository.is_empty() => VACANT_NOWHERE.to_owned(),
            None => format!("Vacant in {}", self.repository),
        }
    }

    /// A member row's second line: `main-fix · Henrik's MacBook`, or, with nobody at its
    /// desk, `Desk empty · 1 waiting` ([`crate::desk::empty`]).
    #[must_use]
    pub fn member_line(&self, machine: &str) -> String {
        match &self.holder {
            Some(holder) => format!("{} · {machine}", holder.session.who()),
            None => crate::desk::empty(self.waiting),
        }
    }
}

/// How a seat with no holder and no repository yet stands ([`Seat::status`]).
pub const VACANT_NOWHERE: &str = "Vacant";

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

/// What a person tells an agent to take role `role` with: `Take the fixer role.`
#[must_use]
pub fn tell_an_agent(role: &str) -> String {
    TELL_AN_AGENT.replace("{role}", role)
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
    /// The card's heading: `Nobody holds fixer in monorepo`.
    #[must_use]
    pub fn title(&self) -> String {
        vacant_title(&self.seat.role, &self.seat.repository)
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

/// A vacant role's card's heading ([`Vacant::title`]): `Nobody holds fixer in monorepo`.
#[must_use]
pub fn vacant_title(role: &str, repository: &str) -> String {
    format!("Nobody holds {role} in {repository}")
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
    /// role in monorepo`, `main-fix left the fixer role`: how to take it is said only while
    /// it's vacant, on its card ([`Vacant::command`]). What plain
    /// IRC clients show; apps say whose the session is ([`SeatChange::words_with`]).
    #[must_use]
    pub fn words(&self) -> String {
        self.words_with(None)
    }

    /// Its line, `whose` being the session's owner as the reader's app says it, possessive:
    /// `main-fix (Henrik's) took the fixer role in monorepo`. Zirc
    /// can't know who asked the session to take it, so it names whose the session is.
    #[must_use]
    pub fn words_with(&self, whose: Option<&str>) -> String {
        let who = match whose {
            Some(whose) => format!("{} ({whose})", self.holder.session.who()),
            None => self.holder.session.who().to_owned(),
        };
        self.words_of(&who)
    }

    /// Its line in a room, read by the session's owner: `your main-fix took the fixer role
    /// in monorepo`, `your main-fix left the fixer role`.
    #[must_use]
    pub fn words_ours(&self) -> String {
        self.words_of(&format!("your {}", self.holder.session.who()))
    }

    /// Its line, the session named as `who`.
    fn words_of(&self, who: &str) -> String {
        if self.took {
            format!("{who} took the {} role in {}", self.role, self.repository)
        } else {
            left_words(who, &self.role)
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
            Address::decl(config),
            Holder::decl(config),
            Seat::decl(config),
            Vacant::decl(config),
            SeatChange::decl(config),
            Standing::decl(config),
            SeatShown::decl(config),
            StandingShown::decl(config),
            format!("const ROLE_SEATS = {SEATS};"),
            format!(
                "const ROLE_HUES: readonly {{ hue: number; name: string }}[] = [{}];",
                HUES.map(|(hue, name)| format!("{{ hue: {hue}, name: {name:?} }}"))
                    .join(", ")
            ),
            format!("const ROLE_MAX_BRIEF = {MAX_BRIEF};"),
            format!("const TAKE_COMMAND = {TAKE_COMMAND:?};"),
            format!("const TELL_AN_AGENT = {TELL_AN_AGENT:?};"),
            format!("const ROLES_ACCOUNTS_PATH = {ACCOUNTS_PATH:?};"),
            format!("const INBOUND_PATH = {INBOUND_PATH:?};"),
            format!(
                "const SIGNED_BY_OWN: readonly InboundTool[] = [{}];",
                SIGNED_BY_OWN
                    .iter()
                    .map(|tool| format!("{:?}", tool.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            format!("const SIGNING_MAX = {SIGNING_MAX};"),
        ])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seats_day_says_its_wakes_and_what_waits() {
        let seat = |woke_today, waiting| Seat {
            role: "fixer".into(),
            repository: "monorepo".into(),
            holder: None,
            last: None,
            waiting,
            wakes_on: Vec::new(),
            woke_today,
        };
        assert_eq!(seat(0, 0).day(), "Not woken today");
        // Nobody at its desk: its row says so, and how much waits.
        assert_eq!(seat(0, 0).member_line(""), "Desk empty");
        assert_eq!(seat(0, 2).member_line(""), "Desk empty · 2 waiting");
        assert_eq!(seat(1, 1).day(), "Woke once today · 1 message waiting");
        assert_eq!(seat(3, 2).day(), "Woke 3 times today · 2 messages waiting");
    }

    #[test]
    fn the_take_command_is_zircs() {
        assert!(TAKE_COMMAND.starts_with(&format!("{} ", crate::PROGRAM)));
        assert_eq!(take_command("fixer"), "zirc role take fixer");
        assert_eq!(tell_an_agent("fixer"), "Take the fixer role.");
    }

    #[test]
    fn a_role_has_the_picture_and_colour_picked_or_its_names() {
        let mut role = fixer();
        assert_eq!(role.look().said(), None);
        assert_eq!(role.look().picture_of("fixer"), "fixer");
        assert!(!serde_json::to_string(&role).unwrap().contains("picture"));
        role.picture = Some("designer".into());
        role.hue = Some(210);
        role.check().unwrap();
        let said = role.look().said().unwrap();
        assert_eq!(said, r#"{"picture":"designer","hue":210}"#);
        assert_eq!(Look::of(&said), role.look());
        assert_eq!(role.look().picture_of("fixer"), "designer");
        // What a newer server may say and this reader can't draw is nothing picked.
        assert_eq!(
            Look::of(r#"{"picture":"hologram","hue":400}"#),
            Look::default()
        );
        role.picture = Some("hologram".into());
        assert!(role.check().unwrap_err().contains("hologram"));
        // Every colour offered is one a role can have.
        role.picture = None;
        for (hue, _) in HUES {
            role.hue = Some(hue);
            role.check().unwrap();
        }
        role.hue = Some(360);
        assert!(role.check().is_err());
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
                title: None,
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
            addresses: vec![],
            version: "1".into(),
        };
        assert!(listing.check().unwrap_err().contains("named twice"));
    }

    #[test]
    fn an_address_opens_with_its_secret_alone() {
        let secret = "k7Qp2vXe9LmR0aBcDeFgHiJk";
        let listing = Listing {
            roles: vec![fixer()],
            addresses: vec![Address {
                role: "fixer".into(),
                digest: Address::digest_of(secret),
                signing: BTreeMap::new(),
            }],
            version: "1".into(),
        };
        assert_eq!(
            Address::digest_of("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(listing.opens("fixer", secret));
        assert!(!listing.opens("fixer", "k7Qp2vXe9LmR0aBcDeFgHiJ"));
        assert!(!listing.opens("triage", secret));
        assert!(!listing.clone().without_addresses().opens("fixer", secret));
        let said = serde_json::to_value(listing.without_addresses()).unwrap();
        assert!(said.get("addresses").is_none());
        assert_eq!(
            INBOUND_PATH
                .replace("{role}", "fixer")
                .replace("{secret}", secret),
            format!("/in/fixer/{secret}")
        );
    }

    #[test]
    fn a_tool_signs_with_a_secret_of_its_own_only_where_it_has_one() {
        let address = |tool, secret: &str| Listing {
            roles: vec![fixer()],
            addresses: vec![Address {
                role: "fixer".into(),
                digest: Address::digest_of("k7Qp2vXe9LmR0aBcDeFgHiJk"),
                signing: BTreeMap::from([(tool, secret.to_owned())]),
            }],
            version: "1".into(),
        };
        let linear = address(Tool::Linear, "lin_wh_abc");
        assert_eq!(linear.check(), Ok(()));
        assert_eq!(
            linear.addresses[0].signing_secret(Tool::Linear),
            Some("lin_wh_abc")
        );
        assert_eq!(linear.addresses[0].signing_secret(Tool::Sentry), None);
        assert!(
            address(Tool::Github, "x")
                .check()
                .unwrap_err()
                .contains("GitHub doesn't sign with a secret of its own")
        );
        assert!(address(Tool::Sentry, "").check().is_err());
        assert!(address(Tool::Sentry, "a\nb").check().is_err());
        assert!(
            address(Tool::Pagerduty, &"x".repeat(SIGNING_MAX + 1))
                .check()
                .is_err()
        );
        let said = serde_json::to_value(linear.without_addresses()).unwrap();
        assert!(said.get("addresses").is_none());
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
        assert_eq!(vacant.title(), "Nobody holds fixer in monorepo");
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
            "main-fix (Henrik's) took the fixer role in monorepo"
        );
        assert_eq!(change.words_yours(), "your main-fix took the role");
        assert_eq!(SeatChange::of(&change.event()), Some(change));
    }

    /// The roles as a server answered `zirc roles` on 26 Nov 2026 (GET /api/v1/roles, its
    /// seats as the Roles tab draws them), which zirc 0.1.67 couldn't read: its holder's card
    /// had no title, which 40d9584 made optional.
    fn answered_today() -> serde_json::Value {
        serde_json::json!({
            "listing": {
                "roles": [{
                    "name": "fixer",
                    "repository": "monorepo",
                    "brief": "Owns a broken master and flaky tests.",
                    "triggers": [
                        {"state": "master", "level": "broken"},
                        {"landed": ["MODULE.bazel"]},
                        {"every": "7d", "day": "mon", "at": "09:00"},
                    ],
                    "seats": 1,
                    "reports": ["warning", "flaky", "container"],
                }, {
                    "name": "oncall",
                    "repository": "server",
                    "brief": "Owns errors in production.",
                    "reports": ["error", "issue", "incident"],
                }],
                "version": "v7",
            },
            "seats": [{
                "role": "fixer",
                "repository": "monorepo",
                "holder": {"machine": "mattias-mbp", "since": 1_764_000_000_000_u64,
                           "session": "cc-8acfe2", "name": "fixer", "tool": "claude-code"},
                "last": {"machine": "mattias-mbp", "since": 1, "session": "cc-1",
                         "title": "Fix master"},
                "waiting": 2,
                "wakes_on": ["master is broken", "a direct message"],
                "woke_today": 3,
                "picture": "fixer",
                "hue": 210,
                "holder_machine": "Mattias's mbp",
                "day": "Woke 3 times today · 2 messages waiting",
            }, {
                "role": "oncall",
                "repository": "server",
                "wakes_on": ["a direct message"],
                "picture": "oncall",
                "hue": 30,
                "day": "Not woken today",
            }],
        })
    }

    #[test]
    fn todays_listing_reads() {
        let standing: Standing = serde_json::from_value(answered_today()).unwrap();
        let (fixer, seat) = standing.role("fixer").unwrap();
        assert_eq!(fixer.triggers.len(), 3);
        let holder = seat.unwrap().holder.as_ref().unwrap();
        assert_eq!(
            (holder.session.title.as_deref(), holder.machine.as_str()),
            (None, "mattias-mbp")
        );
        let last = seat.unwrap().last.as_ref().unwrap();
        assert_eq!(last.session.title.as_deref(), Some("Fix master"));
        let (oncall, _) = standing.role("oncall").unwrap();
        assert_eq!(oncall.reports, crate::role_report::Subject::ALL[3..]);
        // And what zirc.dev is answered, the same.
        let shown: StandingShown = serde_json::from_value(answered_today()).unwrap();
        assert_eq!(
            shown.seats[0].holder_machine.as_deref(),
            Some("Mattias's mbp")
        );
    }

    #[test]
    fn a_newer_servers_listing_still_reads() {
        let mut said = answered_today();
        let fixer = &mut said["listing"]["roles"][0];
        // A kind of report, a kind of trigger and a level this reader doesn't know, and
        // fields it doesn't know, all over.
        fixer["reports"] = serde_json::json!(["warning", "deploy", "flaky"]);
        fixer["triggers"] = serde_json::json!([
            {"state": "master", "level": "broken"},
            {"state": "master", "level": "on-fire"},
            {"cron": "0 9 * * 1"},
            {"landed": ["web"], "only_on": "master"},
        ]);
        fixer["colour"] = serde_json::json!("teal");
        said["listing"]["addresses"] = serde_json::json!([{
            "role": "fixer", "digest": "ab", "signing": {"sentry": "s", "bugsnag": "b"},
            "rotated": 3,
        }]);
        said["seats"][0]["holder"]["avatar"] = serde_json::json!("gem");
        said["seats"][0]["mood"] = serde_json::json!({"calm": true});
        said["generation"] = serde_json::json!(2);
        let standing: Standing = serde_json::from_value(said).unwrap();
        let (fixer, seat) = standing.role("fixer").unwrap();
        assert_eq!(
            fixer.reports,
            [
                crate::role_report::Subject::Warning,
                crate::role_report::Subject::Flaky
            ]
        );
        assert_eq!(fixer.wakes_on(), ["master is broken", WAKES_ON_MESSAGE]);
        assert_eq!(
            standing.listing.addresses[0]
                .signing
                .keys()
                .collect::<Vec<_>>(),
            [&Tool::Sentry]
        );
        // What it wakes on is the server's words, which name what this reader can't.
        assert_eq!(
            seat.unwrap().wakes_on,
            ["master is broken", "a direct message"]
        );
        assert_eq!(
            seat.unwrap().holder.as_ref().unwrap().session.session,
            "cc-8acfe2"
        );
    }
}
