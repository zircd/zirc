//! People's profiles at zirc.dev: the name and picture a person shows as. A person's zirc.dev
//! account holds one name and one picture, which every organisation they're in shows (the
//! user's choice, 20 Nov 2026), set on zirc.dev's account page alone. Where neither is set,
//! the name is their handle and the picture their initials.
//!
//! - `GET /api/me` answers the signed-in person's own [`Me`].
//! - `PATCH /api/me` with a [`NameChange`] sets the account's name.
//! - `POST /api/organisations/<slug>/handle` with a [`HandleChange`] changes the caller's own
//!   handle, their nick there.
//! - `GET /api/me/avatar?w=<width>` is the account's picture: 404 when there's none.
//! - `GET /api/profiles?nick=<nick>&nick=…` answers [`Profiles`]: how each nick, a person's
//!   or a machine's, shows in the caller's organisation.
//!
//! The picture is uploaded as [`crate::media`] says, [`crate::media::Purpose::AccountAvatar`]
//! (an app from before 20 Nov 2026 may send [`crate::media::Purpose::Avatar`], which zirc.dev
//! takes as the account's too), and authenticated as [`crate::media`]'s are.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where the signed-in person's profile is, under the web app's address.
pub const ME_PATH: &str = "/api/me";

/// The signed-in person's account page, under the web app's address: where its name,
/// picture and handles are set, and it's deleted. Every app's Settings opens it from Profile.
pub const ACCOUNT_PAGE_PATH: &str = "/account";

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
    /// Their organisations, with their handle in each.
    pub orgs: Vec<MeOrg>,
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
}

/// A change of the account's name: `None` clears it, so the handle shows.
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
    /// The name to show: the account's, or the handle. A
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
