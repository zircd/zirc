//! A person's role in an organisation, as zirc.dev keeps it and every part of zirc says it:
//! the web app's members, the controller, the apps and integrations told who pressed a card's
//! button. One list, so none of them spells a role its own way.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A person's role in an organisation.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    TS,
    JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Everything, including roles, billing and deleting the organisation. An organisation
    /// always keeps one.
    Owner,
    /// All but what only owners do.
    Admin,
    /// Their own machines and handle: what someone is when nothing says otherwise.
    #[default]
    Member,
}

/// A role as zirc.dev says it, for `#[serde(with)]`, which ts-rs reads (it warns of
/// `deserialize_with`): one it doesn't say, or one this zirc doesn't know, from an older or
/// newer zirc.dev, is a member's. Written as [`Role`] writes itself.
pub mod lenient {
    use serde::{Deserialize, Serialize};

    use super::Role;

    /// `role` as [`Role`] writes it.
    ///
    /// # Errors
    ///
    /// When the serializer fails.
    pub fn serialize<S: serde::Serializer>(role: &Role, serializer: S) -> Result<S::Ok, S::Error> {
        role.serialize(serializer)
    }

    /// The role said, a member's when it's none or one this zirc doesn't know.
    ///
    /// # Errors
    ///
    /// Never for a string or null; only when it's neither.
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Role, D::Error> {
        let text = Option::<String>::deserialize(deserializer)?;
        Ok(text.as_deref().and_then(Role::named).unwrap_or_default())
    }
}

impl Role {
    /// Every role, most powerful first.
    pub const ALL: [Self; 3] = [Self::Owner, Self::Admin, Self::Member];

    /// How it's written: `owner`, `admin` or `member`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    /// The role written `text`, if it's one.
    #[must_use]
    pub fn named(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == text)
    }

    /// Whether it's an owner's or an admin's: what [`crate::card::Who::Admins`] lets press.
    #[must_use]
    pub fn is_admin(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_written_as_zirc_dev_keeps_it() {
        for role in Role::ALL {
            assert_eq!(
                serde_json::to_value(role).unwrap(),
                serde_json::Value::from(role.as_str())
            );
            assert_eq!(Role::named(role.as_str()), Some(role));
        }
        assert_eq!(Role::named("Owner"), None);
        assert!(Role::Admin.is_admin() && !Role::Member.is_admin());
    }

    #[test]
    fn a_role_zirc_dev_doesnt_say_or_this_zirc_doesnt_know_is_a_members() {
        #[derive(Deserialize)]
        struct Org {
            #[serde(default, with = "lenient")]
            role: Role,
        }
        let role = |json: &str| serde_json::from_str::<Org>(json).unwrap().role;
        assert_eq!(role(r#"{"role":"admin"}"#), Role::Admin);
        for json in [
            r#"{"role":""}"#,
            r#"{"role":"steward"}"#,
            r#"{"role":null}"#,
            "{}",
        ] {
            assert_eq!(role(json), Role::Member, "{json}");
        }
    }
}
