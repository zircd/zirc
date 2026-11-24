//! What zirc.dev wants of an organisation's server, as one document: its owners, teams,
//! settings, who may read what and the keys browser logins are signed with, with a
//! generation that grows with each version of it.
//!
//! zirc.dev owns this state, and the server makes itself match it; what the server has (its
//! history, who's connected) is never in it. A server applies a document only if its
//! generation is newer than the last one it applied, so a delayed or repeated one does
//! nothing, and applying it twice is harmless.
//!
//! Hosted servers get it as the `ZircServer` resource's spec, which flattens this document in
//! beside what only hosting needs (plan, limits, suspension: the tenant controller's
//! `controller/src/server.rs`). zirc.dev's web app builds it from the TypeScript these
//! generate ([`typescript`]), never from types of its own.
//!
//! The schema only grows: a new field is optional with a default, so a server that's older
//! than the document reads it safely.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::is_zero;
use ts_rs::TS;

/// The bounds on a `DesiredState`, in one place: its schema holds a server to them, and
/// zirc.dev reads them from the TypeScript the tenant controller generates.
pub mod limits {
    /// Teams an organisation may have.
    pub const TEAMS_MAX: usize = 100;
    /// Channels a team may have.
    pub const TEAM_CHANNELS_MAX: usize = 50;
    /// People a team may have.
    pub const TEAM_MEMBERS_MAX: usize = 1000;
    /// A team's name: unique in the organisation.
    pub const TEAM_NAME_PATTERN: &str = r"^[a-z0-9-]{1,30}$";
    /// The longest team name.
    pub const TEAM_NAME_MAX: usize = 30;

    /// A channel's name, as a team's channels and the default channels have them.
    pub const CHANNEL_PATTERN: &str = r"^#[^\s,]{1,63}$";

    /// Machines an organisation may have.
    pub const MACHINES_MAX: usize = 5000;

    /// Owners an organisation may have.
    pub const OWNERS_MAX: usize = 100;
    /// The longest handle (account name), as zirc.dev, zircd and the gateway hold handles to
    /// it, with room for `-<machine>` within a nick.
    pub const HANDLE_MAX: usize = crate::names::HANDLE_MAX;

    /// Keys browser logins may be signed with at once, while one replaces another.
    pub const TOKEN_KEYS_MAX: usize = 4;
    /// The longest key (PEM).
    pub const TOKEN_KEY_CHARS: usize = 200;
    /// A key: an Ed25519 public key, PEM.
    pub const TOKEN_KEY_PATTERN: &str =
        r"^-----BEGIN PUBLIC KEY-----\n[A-Za-z0-9+/=\n]+-----END PUBLIC KEY-----\n?$";

    // The settings' bounds (`super::Settings`); each plan's own, within these, are the tenant
    // controller's (`controller/plans.json`).
    /// The longest message of the day.
    pub const MOTD_CHARS: usize = 2000;
    /// The most lines in a message of the day.
    pub const MOTD_LINES: usize = 20;
    /// How long history is kept, in days.
    pub const HISTORY_RETENTION_PATTERN: &str = r"^[1-9][0-9]{0,3}d$";
    /// Channels every new connection may be put in.
    pub const DEFAULT_CHANNELS_MAX: usize = 20;
    /// The shortest longest line, in bytes: IRC's own.
    pub const LINE_LENGTH_MIN: u32 = 512;
    /// The longest line any plan allows, in bytes.
    pub const LINE_LENGTH_MAX: u32 = 4096;
    /// The fewest bytes a message of several lines may be held to.
    pub const MULTILINE_BYTES_MIN: u32 = 512;
    /// The most bytes a message of several lines may have.
    pub const MULTILINE_BYTES_MAX: u32 = 1_048_576;
    /// The most lines a message of several lines may have.
    pub const MULTILINE_LINES_MAX: u32 = 10_000;
    /// The largest burst a connection may send at once.
    pub const RATE_BURST_MAX: u32 = 10_000;
    /// The most lines a second a connection may send after its burst.
    pub const RATE_PER_SECOND_MAX: u32 = 1000;
}

/// What zirc.dev wants of an organisation's server. Each field is optional on the wire, and
/// left out it's the default: nothing, or Ergo's own.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesiredState {
    /// Which version of the document this is: zirc.dev gives each one it sends a higher
    /// number than the last, and a server never applies one older than what it has. 0, or
    /// left out, for a document sent before there were generations.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub generation: u64,

    /// The organisation's owners, by their handles: they get ops in every channel on the
    /// server, as it's made, and whoever made it doesn't (not their machines, either).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = limits::OWNERS_MAX), inner(length(max = limits::HANDLE_MAX)))]
    pub owners: Vec<String>,

    /// The organisation's admins, by their handles: the server gives them an admin's role
    /// on its own pages (`DesiredState::content_api`), as on zirc.dev. Owners aren't listed
    /// again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = limits::OWNERS_MAX), inner(length(max = limits::HANDLE_MAX)))]
    pub admins: Vec<String>,

    /// The organisation's machines, each account with its owner's handle: whose a machine is,
    /// which its name needn't say (`m3-mac` as well as `ada-mbp`). The server takes a machine's
    /// person, role and team channels from here, never from its name, once it's been told any;
    /// a server never told keeps reading `<handle>-<machine>` names, as before there were these.
    /// A list, not a map, so a merge patch of the spec replaces it whole, and a machine that's
    /// gone goes from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = limits::MACHINES_MAX))]
    pub machines: Vec<Machine>,

    /// The organisation's sub-teams, each with the channels it looks after and the people in
    /// it. Channels stay open to the whole organisation; a team's members (not their
    /// machines) get ops in its channels, and a parent team's members in its children's too.
    /// Names are unique, and a parent is one of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = limits::TEAMS_MAX))]
    pub teams: Vec<Team>,

    /// What an organisation's owners and admins may change about their server, within their
    /// plan.
    #[serde(default, skip_serializing_if = "Settings::is_default")]
    pub settings: Settings,

    /// How people and machines identify themselves on the server (docs/accounts.md).
    #[serde(default, skip_serializing_if = "Accounts::is_default")]
    pub accounts: Accounts,

    /// Who may read which conversations in the server's history, through zirc.dev (and the
    /// agents people give it to): `participant`, each person their own channels and direct
    /// messages; `channels`, every channel too, private ones included; `everything`, others'
    /// direct messages as well, for a team that's open about everything. Everyone joining is
    /// told what others may read.
    #[serde(default, skip_serializing_if = "Visibility::is_default")]
    pub visibility: Visibility,

    /// The public halves (PEM) of the keys zirc.dev signs this organisation's browser logins
    /// with: a JWT naming an account here, over SASL `IRCV3BEARER`. A list, so a new key can
    /// be added before the old one goes. Each organisation has its own, as handles are only
    /// unique within one: a token for `ada` here mustn't log in as another organisation's
    /// `ada`. Without one, there's no JWT login.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        length(max = limits::TOKEN_KEYS_MAX),
        inner(
            length(max = limits::TOKEN_KEY_CHARS),
            regex(pattern = limits::TOKEN_KEY_PATTERN)
        )
    )]
    pub token_keys: Vec<String>,

    /// The server answers content requests itself, at its own host name
    /// (`https://<host>/api/v1/…`): history, conversations, messages, export and its
    /// logs, each authorised by the person's own token (a device's, or a browser login's),
    /// with its role and the visibility checked on the server. Off by default; turning it on
    /// restarts nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub content_api: bool,
}

/// One of the organisation's machines (`DesiredState::machines`): its account, and whose it is.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    /// Its account name and nick, such as `m3-mac`.
    #[schemars(length(max = crate::names::NICK_MAX))]
    pub account: String,
    /// Its owner's handle, such as `mattias`.
    #[schemars(length(max = limits::HANDLE_MAX))]
    pub owner: String,
    /// What it is to its owner: one of their machines, or their own client. Left out for a
    /// machine, so a server from before kinds reads the list as it did.
    #[serde(default, skip_serializing_if = "MachineKind::is_machine")]
    #[ts(as = "Option<MachineKind>", optional)]
    pub kind: MachineKind,
}

/// What an account kept among an organisation's machines is to its owner ([`Machine::kind`]).
/// The server says it to everyone as the account's host ([`crate::names::Hosted`]), and
/// whose word reaches a machine's agents goes by it: a client's is its owner's own, a
/// machine's never is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum MachineKind {
    /// A machine, whose agents share its account: its host is `machine/<owner>`.
    #[default]
    Machine,
    /// Its owner's own client under another name, such as their IRC client `mattias-irc`:
    /// the person at a keyboard. Its host is `client/<owner>`.
    Client,
}

impl MachineKind {
    /// Whether it's a machine, as one that doesn't say is.
    #[must_use]
    pub fn is_machine(&self) -> bool {
        *self == Self::Machine
    }
}

/// One of the organisation's teams (`DesiredState::teams`): channels it looks after, and
/// people in it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    /// Unique in the organisation.
    #[schemars(
        regex(pattern = limits::TEAM_NAME_PATTERN),
        length(max = limits::TEAM_NAME_MAX)
    )]
    pub name: String,

    /// The team it's part of, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = limits::TEAM_NAME_MAX))]
    pub parent: Option<String>,

    /// The channels it has, such as `#frontend`.
    #[serde(default)]
    #[schemars(
        length(max = limits::TEAM_CHANNELS_MAX),
        inner(regex(pattern = limits::CHANNEL_PATTERN))
    )]
    pub channels: Vec<String>,

    /// Its people, by their handles (account names), without their machines.
    #[serde(default)]
    #[schemars(
        length(max = limits::TEAM_MEMBERS_MAX),
        inner(length(max = limits::HANDLE_MAX))
    )]
    pub members: Vec<String>,

    /// Its channels let in only its people (with a parent team's), the owners, and their
    /// machines; anyone else needs an `INVITE`. Otherwise they're open to the whole
    /// organisation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invite_only: bool,
}

/// How people and machines identify themselves (`DesiredState::accounts`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Accounts {
    /// Every connection must log in to an account with SASL, rather than give the gate
    /// password. Off by default; turn it on once everyone connecting has an account.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_sasl: bool,

    /// Apps and machines may sign in with a key made on their device ([`crate::device_key`]),
    /// which the server's sidecar checks by itself, so they keep connecting while zirc.dev is
    /// down. Passphrases keep working beside it. Off by default; turning it on restarts the
    /// server.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub device_keys: bool,
}

impl Accounts {
    /// Whether it's as left out: Ergo's own, or nothing.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The server's settings (`DesiredState::settings`), each as Ergo's own when left out. A
/// value beyond the plan's maximum is held to it, and the server says so.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// What everyone connecting is told first, instead of zirc's line naming the server.
    /// At most 20 lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = limits::MOTD_CHARS))]
    pub motd: Option<String>,

    /// How long history is kept, in days (`30d`): everything unless set, and at most what the
    /// plan allows (30 days on a trial; no cap paid). Messages older are deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = limits::HISTORY_RETENTION_PATTERN))]
    pub history_retention: Option<String>,

    /// Who may make a channel by joining one that isn't there.
    #[serde(default, skip_serializing_if = "ChannelCreation::is_default")]
    pub channel_creation: ChannelCreation,

    /// Channels every new connection joins, which isn't a person's apps coming back to their
    /// always-on account: a person's first connection, and each of a machine's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        length(max = limits::DEFAULT_CHANNELS_MAX),
        inner(regex(pattern = limits::CHANNEL_PATTERN))
    )]
    pub default_channels: Vec<String>,

    /// The longest line, in bytes, tags apart: 512 unless set, up to 2048 on a trial and 4096
    /// paid. The one setting that waits for a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = limits::LINE_LENGTH_MIN, max = limits::LINE_LENGTH_MAX))]
    pub line_length: Option<u32>,

    /// A message of several lines (`draft/multiline`): 16384 bytes and 200 lines unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiline: Option<Multiline>,

    /// How fast each connection may send once past its burst (Ergo's fakelag): 50 lines,
    /// then 20 a second, unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,

    /// Whether members may let other members' agents ask their machines' agents (`zirc
    /// ask`), each machine opting in with its own setting: allowed unless set to false
    /// ([`crate::colleagues`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colleague_queries: Option<bool>,

    /// Whether a member an agent wrote to may answer it, their reply reaching that agent as a
    /// colleague's word: allowed unless set to false ([`crate::colleagues`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colleague_answers: Option<bool>,
}

impl Settings {
    /// Whether it's as left out: Ergo's own, or nothing.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Who may make a channel (`Settings::channel_creation`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum ChannelCreation {
    /// Anyone on the server.
    #[default]
    Anyone,
    /// Only the server's operators: new channels come from the organisation's teams.
    Operators,
}

impl ChannelCreation {
    /// Whether it's as left out: Ergo's own, or nothing.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// How long a message of several lines may be (`Settings::multiline`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Multiline {
    /// Its bytes, all lines together.
    #[schemars(range(min = limits::MULTILINE_BYTES_MIN, max = limits::MULTILINE_BYTES_MAX))]
    pub max_bytes: u32,
    /// Its lines.
    #[schemars(range(min = 1, max = limits::MULTILINE_LINES_MAX))]
    pub max_lines: u32,
}

/// How fast a connection may send (`Settings::rate_limit`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    /// Lines a connection may send at once.
    #[schemars(range(min = 1, max = limits::RATE_BURST_MAX))]
    pub burst: u32,
    /// Lines a second after that.
    #[schemars(range(min = 1, max = limits::RATE_PER_SECOND_MAX))]
    pub per_second: u32,
}

impl RateLimit {
    /// What a server gets unless its settings say otherwise: 50 lines at once, then 20 a
    /// second. Ergo's own default, 5 and then 2, is for strangers on a public network: a
    /// catch-up alone is some 16 lines.
    pub const USUAL: Self = Self {
        burst: 50,
        per_second: 20,
    };
}

/// Who may read which conversations (`DesiredState::visibility`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// Each person their own channels and direct messages.
    #[default]
    Participant,
    /// Every channel too, but not others' direct messages.
    Channels,
    /// Others' direct messages as well.
    Everything,
}

impl Visibility {
    /// Whether it's as left out: Ergo's own, or nothing.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The document's types as TypeScript, for zirc.dev's web app (`lib/zirc-proto.ts`).
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        DesiredState::decl(config),
        Machine::decl(config),
        MachineKind::decl(config),
        Team::decl(config),
        Accounts::decl(config),
        Settings::decl(config),
        ChannelCreation::decl(config),
        Multiline::decl(config),
        RateLimit::decl(config),
        Visibility::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_document_is_an_empty_object() {
        assert_eq!(
            serde_json::to_value(DesiredState::default()).unwrap(),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::from_str::<DesiredState>("{}").unwrap(),
            DesiredState::default()
        );
    }

    #[test]
    fn a_document_reads_back_as_it_was_written() {
        let desired = DesiredState {
            generation: 42,
            owners: vec!["ada".into()],
            admins: vec!["grace".into()],
            machines: vec![
                Machine {
                    account: "m3-mac".into(),
                    owner: "ada".into(),
                    kind: MachineKind::Machine,
                },
                Machine {
                    account: "ada-irc".into(),
                    owner: "ada".into(),
                    kind: MachineKind::Client,
                },
            ],
            teams: vec![Team {
                name: "frontend".into(),
                parent: Some("eng".into()),
                channels: vec!["#web".into()],
                members: vec!["grace".into()],
                invite_only: true,
            }],
            settings: Settings {
                motd: Some("hi".into()),
                ..Settings::default()
            },
            accounts: Accounts {
                require_sasl: true,
                device_keys: true,
            },
            visibility: Visibility::Channels,
            token_keys: vec!["-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n".into()],
            content_api: true,
        };
        let json = serde_json::to_value(&desired).unwrap();
        assert_eq!(json["generation"], 42);
        assert_eq!(json["teams"][0]["inviteOnly"], true);
        assert_eq!(json["accounts"]["requireSasl"], true);
        assert!(json["tokenKeys"].is_array());
        assert_eq!(json["admins"], serde_json::json!(["grace"]));
        assert_eq!(json["contentApi"], true);
        assert_eq!(
            serde_json::from_value::<DesiredState>(json).unwrap(),
            desired
        );
    }

    #[test]
    fn a_newer_document_with_fields_this_doesnt_know_still_reads() {
        let desired: DesiredState =
            serde_json::from_str(r#"{"generation": 7, "somethingNew": {"a": 1}}"#).unwrap();
        assert_eq!(desired.generation, 7);
    }
}
