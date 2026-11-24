//! People's names and pictures on the server, as zirc.dev has them (its profiles, with each
//! organisation's overrides): IRC METADATA on each account, under the IRCv3 registry's keys
//! [`NAME`] and [`AVATAR`], which apps subscribe to and Ergo pushes as they change.
//!
//! The sidecar sets them as Ergo's operator. A person's account is always-on, so it has a
//! client to set them on whether or not they're connected, and Ergo keeps them across
//! restarts. A machine's has one only while it's connected: the sidecar keeps what zirc.dev
//! last said (beside Ergo's history) and sets it each time the machine comes on.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::limits;

/// The METADATA key of a display name.
pub const NAME: &str = "display-name";
/// The METADATA key of a picture.
pub const AVATAR: &str = "avatar";

/// How often the sidecar looks for accounts come on, to give them theirs: a backstop, as a
/// machine's is set as it logs in (`crate::signin::Signin::logins`), and for servers without
/// device keys, whose Ergo doesn't tell the sidecar of logins.
pub const EVERY: Duration = Duration::from_mins(10);

/// The longest a machine's login takes to give it its profile: a look a second for its client
/// to be on, as it registers.
pub const ON_LOGIN: Duration = Duration::from_secs(15);

/// One account's, as zirc.dev says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// The name.
    pub name: String,
    #[serde(default)]
    /// The picture's URL.
    pub avatar: Option<String>,
}

impl Profile {
    /// Why it can't be set, if it can't.
    pub fn invalid(&self) -> Option<&'static str> {
        if self.name.trim().is_empty() || self.name.chars().count() > limits::PROFILE_NAME_CHARS {
            return Some("a name is 1 to 100 characters");
        }
        if self.name.chars().any(char::is_control) {
            return Some("a name has no control characters");
        }
        match &self.avatar {
            Some(url)
                if url.chars().count() > limits::AVATAR_CHARS
                    || !url.starts_with("https://")
                    || url.chars().any(|c| c.is_whitespace() || c.is_control()) =>
            {
                Some("a picture is an https URL")
            }
            _ => None,
        }
    }

    /// The METADATA it is: the name, and the picture or none.
    #[must_use]
    pub fn keys(&self) -> [(&str, Option<&str>); 2] {
        [
            (NAME, Some(self.name.as_str())),
            (AVATAR, self.avatar.as_deref()),
        ]
    }
}

/// What zirc.dev last said of each account, kept beside Ergo's history, and which accounts
/// have it set now.
pub struct Profiles {
    path: PathBuf,
    /// What was said of each account.
    pub said: BTreeMap<String, Profile>,
    /// Set on the account's client as it is now; a machine's goes when it does.
    pub set: BTreeSet<String>,
}

impl Profiles {
    /// What was said, as noted beside Ergo's history, if anything.
    #[must_use]
    pub fn beside(history: &Path) -> Self {
        let path = history.with_file_name("zirc-profiles.json");
        let said = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            said,
            set: BTreeSet::new(),
        }
    }

    /// Keeps `profile` for `account`, and saves.
    ///
    /// # Errors
    ///
    /// When it can't be saved.
    pub fn keep(&mut self, account: &str, profile: Profile) -> std::io::Result<()> {
        self.said.insert(account.to_owned(), profile);
        self.set.remove(account);
        let bytes = serde_json::to_vec(&self.said).map_err(std::io::Error::other)?;
        let partial = self.path.with_extension("json.partial");
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_is_a_name_and_maybe_a_picture() {
        let profile = |name: &str, avatar: Option<&str>| Profile {
            name: name.into(),
            avatar: avatar.map(Into::into),
        };
        assert_eq!(profile("Ada Lovelace", None).invalid(), None);
        assert_eq!(
            profile("Ada", Some("https://zirc.dev/avatars/1.png")).invalid(),
            None
        );
        assert!(profile("", None).invalid().is_some());
        assert!(profile(&"a".repeat(101), None).invalid().is_some());
        assert!(profile("Ada\nLovelace", None).invalid().is_some());
        assert!(profile("Ada", Some("http://x/1.png")).invalid().is_some());
        assert!(
            profile("Ada", Some("https://x/a b.png"))
                .invalid()
                .is_some()
        );
        assert_eq!(
            profile("Ada", None).keys(),
            [(NAME, Some("Ada")), (AVATAR, None)]
        );
    }

    #[test]
    fn what_zirc_dev_said_is_kept_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("ergo_history.db");
        let mut profiles = Profiles::beside(&history);
        profiles
            .keep(
                "ada",
                Profile {
                    name: "Ada".into(),
                    avatar: None,
                },
            )
            .unwrap();
        assert_eq!(Profiles::beside(&history).said["ada"].name, "Ada");
    }
}
