//! People's profiles at zirc.dev: the name and picture a person shows as. A person's zirc.dev
//! account holds one profile, which every organisation they're in sees; in an organisation
//! they may set a name or picture for it alone, which only it sees. Where neither is set,
//! the name is their handle and the picture their initials.
//!
//! - `GET /api/me` answers the signed-in person's own [`Me`], their overrides included.
//! - `PATCH /api/me` with a [`NameChange`] sets the account's name.
//! - `PATCH /api/organisations/<slug>/members/<handle>` with a [`NameChange`], by the person
//!   themself, sets their name there alone.
//! - `POST /api/organisations/<slug>/handle` with a [`HandleChange`] changes the caller's own
//!   handle, their nick there.
//! - `GET /api/me/avatar?w=<width>` is the account's picture: 404 when there's none.
//! - `GET /api/profiles?nick=<nick>&nick=…` answers [`Profiles`]: how each nick, a person's
//!   or a machine's, shows in the caller's organisation.
//!
//! Pictures are uploaded as [`crate::media`] says: [`crate::media::Purpose::AccountAvatar`]
//! for the account's, [`crate::media::Purpose::Avatar`] for one organisation's. Each is
//! authenticated as [`crate::media`]'s are.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where the signed-in person's profile is, under the web app's address.
pub const ME_PATH: &str = "/api/me";

/// Where the signed-in person's account picture is, under the web app's address.
pub const ME_AVATAR_PATH: &str = "/api/me/avatar";

/// Where profiles are looked up by nick, under the web app's address.
pub const PROFILES_PATH: &str = "/api/profiles";

/// The most characters a name has.
pub const NAME_MAX_CHARS: usize = 64;

/// The most bytes a name has, in UTF-8: Ergo's `realnamelen`, as it's sent as a real name.
pub const NAME_MAX_BYTES: usize = 150;

/// A name as a person gave it, trimmed, if it's one zirc takes: not empty, without control
/// characters, and within [`NAME_MAX_CHARS`] and [`NAME_MAX_BYTES`].
#[must_use]
pub fn name(text: &str) -> Option<&str> {
    let text = text.trim();
    let fits = !text.is_empty()
        && text.chars().count() <= NAME_MAX_CHARS
        && text.len() <= NAME_MAX_BYTES
        && !text.chars().any(char::is_control);
    fits.then_some(text)
}

/// The signed-in person, as their own Profile page shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Me {
    /// The address they sign in with.
    pub email: String,
    /// The account's name, `None` when they've set none.
    pub name: Option<String>,
    /// The account picture's version, `None` when there's none.
    pub avatar: Option<String>,
    /// Whether a new organisation they join shows the account picture: [`MeOrg::share_picture`]
    /// starts as this. A zirc.dev older than the setting says yes.
    #[serde(default = "yes")]
    pub share_picture_default: bool,
    /// Whether a new organisation they join shows the account name: [`MeOrg::share_name`]
    /// starts as this.
    #[serde(default = "yes")]
    pub share_name_default: bool,
    /// Their organisations, and what they've set for each alone.
    pub orgs: Vec<MeOrg>,
}

fn yes() -> bool {
    true
}

/// One of [`Me::orgs`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MeOrg {
    /// Its short name.
    pub slug: String,
    /// Its name.
    pub name: String,
    /// Their handle there.
    pub handle: String,
    /// Their role there. Owners and admins manage its guests. A member's from a zirc.dev older
    /// than roles, which says none.
    #[serde(default, with = "crate::role::lenient")]
    #[ts(as = "crate::role::Role")]
    pub role: crate::role::Role,
    /// The name they set for it alone, `None` when it shows the account's.
    pub name_override: Option<String>,
    /// The version of the picture they set for it alone, `None` when it shows the account's.
    pub avatar_override: Option<String>,
    /// Whether this organisation shows the account picture when they've set none for it.
    /// Off, it shows their initials: the choice is theirs, per organisation.
    #[serde(default = "yes")]
    pub share_picture: bool,
    /// Whether this organisation shows the account name when they've set none for it. Off,
    /// it shows their handle.
    #[serde(default = "yes")]
    pub share_name: bool,
}

/// A change of what an organisation (`PATCH /api/organisations/<slug>/members/<handle>`) or
/// new ones (`PATCH /api/me`, the `_default` keys) may show of the account's profile; a key
/// left out stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ShareChange {
    /// Show the account picture here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_picture: Option<bool>,
    /// Show the account name here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_name: Option<bool>,
}

/// A change of name: `None` clears it, so the account's shows (in an organisation) or the
/// handle does (for the account).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct NameChange {
    /// The new name.
    pub name: Option<String>,
}

/// A change of handle, the nick, in one organisation ([`crate::irc::handle_path`]). The
/// person's apps keep their passphrase and sign in as the new handle, those elsewhere once
/// the server refuses the old one ([`crate::login::PERSON_PATH`]); their machines follow by
/// themselves. The old nick is free for anyone to take, and what was said under it stays
/// under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct HandleChange {
    /// The new handle: [`crate::names`]' rules; refused with 409 when anyone has it.
    pub handle: String,
}

/// How a nick shows in an organisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Profile {
    /// The name to show: the organisation's override, the account's, or the handle. A
    /// machine's is its owner's with the machine's: "Ada's mbp".
    pub name: String,
    /// The picture's version, `None` for initials. Draw it from
    /// [`crate::media::avatar_path`] with `?v=<version>`, so a new one isn't cached as the
    /// old.
    pub avatar: Option<String>,
}

/// [`PROFILES_PATH`]'s answer: each nick asked about that's someone in the organisation.
pub type Profiles = BTreeMap<String, Profile>;

/// The TypeScript declarations of these types, for [`crate::login::typescript`].
#[must_use]
pub(crate) fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Me::decl(config),
        MeOrg::decl(config),
        NameChange::decl(config),
        ShareChange::decl(config),
        HandleChange::decl(config),
        Profile::decl(config),
        "type Profiles = { [nick in string]?: Profile };".to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_kept_short() {
        assert_eq!(name("  Ada Lovelace "), Some("Ada Lovelace"));
        assert_eq!(name("   "), None);
        assert_eq!(name("a\u{7}b"), None);
        assert!(name(&"a".repeat(NAME_MAX_CHARS)).is_some());
        assert_eq!(name(&"a".repeat(NAME_MAX_CHARS + 1)), None);
        // 50 four-byte characters: few enough characters, too many bytes.
        assert_eq!(name(&"😀".repeat(50)), None);
    }

    #[test]
    fn a_cleared_name_is_null() {
        let change: NameChange = serde_json::from_str(r#"{"name":null}"#).unwrap();
        assert_eq!(change.name, None);
        let profile = Profile {
            name: "Ada".into(),
            avatar: None,
        };
        assert_eq!(
            serde_json::to_string(&profile).unwrap(),
            r#"{"name":"Ada","avatar":null}"#
        );
    }
}
