//! What an organisation shows of someone's account profile, and the words every app says it
//! with. Someone's picture in an organisation is the one they set there, else their account's
//! unless they turned sharing off for that organisation, else their initials; their name the
//! same, ending at their handle. zirc.dev applies the rule when it tells a server how each
//! person shows (`web/src/lib/profiles.ts`, in SQL); [`picture_shown`] and [`name_shown`] are
//! the same rule for an app that previews it, and the tests here say it.

/// The switch, in an organisation's Profile here: the account picture shows in this
/// organisation. The account's default for new organisations says [`SHARE_PICTURE_DEFAULT`].
pub const SHARE_PICTURE: &str = "Share my account picture here";

/// Under [`SHARE_PICTURE`].
pub const SHARE_PICTURE_HINT: &str =
    "Off: this organisation shows your initials, unless you set a picture for it.";

/// The same switch for the name.
pub const SHARE_NAME: &str = "Share my account name here";

/// Under [`SHARE_NAME`].
pub const SHARE_NAME_HINT: &str =
    "Off: this organisation shows your handle, unless you set a name for it.";

/// The account's default for organisations they join from now on.
pub const SHARE_PICTURE_DEFAULT: &str = "Share my account picture with new organisations";

/// The account's default for the name.
pub const SHARE_NAME_DEFAULT: &str = "Share my account name with new organisations";

/// Under the account's defaults, which don't touch the organisations they're already in.
pub const SHARE_DEFAULT_HINT: &str =
    "For organisations you join from now on. The ones you're in keep their own setting.";

/// What an organisation's picture switch shows beside it, as [`picture_shown`] decides: the
/// picture set for it (`own`), the `account` one (shared), or initials.
#[must_use]
pub fn picture_preview(own: bool, account: bool, share: bool) -> &'static str {
    if own {
        "Showing the picture set for this organisation"
    } else if account && share {
        "Showing your account picture"
    } else {
        "Showing your initials"
    }
}

/// What an organisation's name switch shows beside it: `Showing Ada Lovelace`, `name` being
/// what [`name_shown`] decided.
#[must_use]
pub fn name_preview(name: &str) -> String {
    format!("Showing {name}")
}

/// The picture's version to show in an organisation: the `own` one set there, else the
/// `account`'s when `share` is on, else `None` for initials.
#[must_use]
pub fn picture_shown<'a>(
    own: Option<&'a str>,
    account: Option<&'a str>,
    share: bool,
) -> Option<&'a str> {
    own.or(account.filter(|_| share))
}

/// The name to show in an organisation: the `own` one set there, else the `account`'s when
/// `share` is on, else the `handle`.
#[must_use]
pub fn name_shown<'a>(
    own: Option<&'a str>,
    account: Option<&'a str>,
    share: bool,
    handle: &'a str,
) -> &'a str {
    own.or(account.filter(|_| share)).unwrap_or(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_the_override_else_the_shared_account_one_else_initials() {
        assert_eq!(picture_shown(Some("o"), Some("a"), true), Some("o"));
        // An override survives sharing being off, and the account's picture changing.
        assert_eq!(picture_shown(Some("o"), Some("a2"), false), Some("o"));
        assert_eq!(picture_shown(None, Some("a"), true), Some("a"));
        assert_eq!(picture_shown(None, Some("a"), false), None);
        assert_eq!(picture_shown(None, None, true), None);
    }

    #[test]
    fn says_what_is_shown_beside_each_switch() {
        assert_eq!(
            picture_preview(true, true, true),
            "Showing the picture set for this organisation"
        );
        assert_eq!(
            picture_preview(false, true, true),
            "Showing your account picture"
        );
        assert_eq!(picture_preview(false, true, false), "Showing your initials");
        assert_eq!(picture_preview(false, false, true), "Showing your initials");
        assert_eq!(name_preview("m"), "Showing m");
    }

    #[test]
    fn the_name_ends_at_the_handle() {
        assert_eq!(name_shown(Some("Matt"), Some("Mattias"), true, "m"), "Matt");
        assert_eq!(name_shown(None, Some("Mattias"), true, "m"), "Mattias");
        assert_eq!(name_shown(None, Some("Mattias"), false, "m"), "m");
        assert_eq!(name_shown(None, None, true, "m"), "m");
    }
}
