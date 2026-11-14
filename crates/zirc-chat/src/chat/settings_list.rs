//! Settings as every app lists it (the user's choice, 11 Nov 2026): one ordered list of
//! sections in three groups, the account's, then the organisation shown's, then the app's,
//! each with its title. Every app draws it its own way, pages in the desktop's sheet and
//! sections on the Mac, the phones and the web, but in this order with these titles, and
//! shows only the sections that apply to it ([`settings_sections`]). What a section holds,
//! its rows, is each app's to draw from the words the core gives it; the words several apps
//! say alike are here.
//!
//! A change applies as it's made, in every app; only typed fields, a name or a server, have a
//! Save of their own.

use serde::Serialize;
use ts_rs::TS;

use super::words::INVITE_PEOPLE;

/// Which app lists Settings: what applies to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum SettingsApp {
    /// The desktop apps, GPUI's and the Mac's: everything, a server of the person's own
    /// choosing among it.
    Desktop,
    /// The phones: everything but choosing a server, which their sign-in sets.
    Phone,
    /// Web chat, in a browser: how it looks and whom the person blocks, and their name and
    /// picture, which zirc.dev's account page edits. It shows no notifications to choose
    /// among.
    Web,
}

/// The group a section is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum SettingsGroup {
    /// The person's zirc.dev account, the same in every organisation.
    Account,
    /// The organisation shown, by its name.
    Organisation,
    /// This app, on this device.
    App,
}

/// What a section is, which says what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum SettingsKind {
    /// The account's name and picture, and what new organisations are shown of them.
    Profile,
    /// The organisations signed in to, adding one, and signing out of everything; before
    /// signing in, signing in.
    Organisations,
    /// Deleting the account, for good.
    DeleteAccount,
    /// The person's handle, and their name and picture just for the organisation shown.
    OrgProfile,
    /// Whether the person is away there, and why.
    Status,
    /// Muting the organisation: nothing of it notifies.
    Mute,
    /// Inviting people to the organisation, on zirc.dev's page for it: an owner's or an
    /// admin's alone. A row that opens the page, with nothing of its own to show.
    Invite,
    /// What notifies, and the system's own notification settings.
    Notifications,
    /// How the app looks: light, dark or the system's, and on a desktop how messages lay out
    /// and where code opens.
    Appearance,
    /// Who may query this machine's agents.
    Agents,
    /// Whom the person blocks, and the anonymous usage counts.
    Privacy,
    /// A server of the person's own choosing, rather than zirc.dev's.
    Server,
    /// The app's version and licences.
    About,
}

/// A section of Settings, as every app lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SettingsSection {
    /// What it is.
    pub kind: SettingsKind,
    /// The group it's in.
    pub group: SettingsGroup,
    /// Its title.
    pub title: String,
    /// What scripts and tests find it by, whatever it's titled: `settings:<id>`.
    pub id: String,
}

/// The title of the account's group.
pub const SETTINGS_ACCOUNT: &str = "Account";

/// The title of the app's group.
pub const SETTINGS_APP: &str = "App";

/// Settings' own name: its page's title, and what opens it.
pub const SETTINGS: &str = "Settings";

/// The organisations signed in to: [`SettingsKind::Organisations`]' title, and a list's or
/// a menu's of them anywhere else. The web page types it too where it asks which to open,
/// drawn before its wasm is loaded (`oss/web/src/App.tsx`).
pub const ORGANISATIONS: &str = "Organisations";

/// Deleting the account: [`SettingsKind::DeleteAccount`]'s title, and its page's.
pub const DELETE_ACCOUNT: &str = "Delete account";

/// The server a sign-in is to: [`SettingsKind::Server`]'s title, and the label of its
/// address wherever that shows. The web page types it too on its dev sign-in, drawn before
/// its wasm is loaded (`oss/web/src/App.tsx`).
pub const SERVER: &str = "Server";

/// Picking another picture, which opens the system's picker.
pub const CHANGE_PICTURE: &str = "Change picture…";

/// [`CHANGE_PICTURE`] on the profile's own button, without the ellipsis.
pub const CHANGE_PICTURE_NAME: &str = "Change picture";

/// Adding an organisation, which opens zirc.dev's sign-in.
pub const ADD_ORGANISATION: &str = "Add organisation…";

/// [`ADD_ORGANISATION`] where it's a button's name or asks nothing first, without the
/// ellipsis.
pub const ADD_ORGANISATION_NAME: &str = "Add organisation";

/// Signing out of every organisation and zirc.dev, which asks first.
pub const SIGN_OUT_EVERYTHING: &str = "Sign out of everything…";

/// The system's own notification settings, which open apart.
pub const SYSTEM_NOTIFICATIONS: &str = "Notification settings…";

/// The switch in an organisation's Status: whether people there see the person as away.
// words-guard: a switch named Away, not a person's status
pub const AWAY: &str = "Away";

/// The field under [`AWAY`]: why, which people there see after "away:".
pub const AWAY_WHY: &str = "Why (optional)";

/// The switch that mutes the organisation called `org`.
#[must_use]
pub fn mute_org(org: &str) -> String {
    format!("Mute {org}")
}

/// Under [`mute_org`]: what muting `org` does.
#[must_use]
pub fn mute_org_note(org: &str) -> String {
    format!(
        "Nothing from {org} notifies, and it leaves the app's badge. Its own count stays, in grey."
    )
}

/// What deleting the account does, naming the organisations `orgs` they'd leave: zirc.dev's
/// account page's words (web/src/app/account/page.tsx).
#[must_use]
pub fn deletion_words(orgs: &[String]) -> String {
    use std::fmt::Write as _;
    let mut words =
        "This deletes your account on zirc.dev and signs you out everywhere.".to_owned();
    if !orgs.is_empty() {
        let (its, servers) = if orgs.len() == 1 {
            ("its", "its server are")
        } else {
            ("their", "their servers are")
        };
        let _ = write!(
            words,
            " You leave {}: your machines stop working there, and your accounts on {servers} \
             deleted, so your names there are free for others. Messages you sent stay in {its} \
             history.",
            orgs.join(", ")
        );
    }
    words.push_str(" Signing up again with the same address makes a new account.");
    words
}

/// Whether `typed` is the person's `email`, as zirc.dev takes it to confirm deleting their
/// account: spaces around it and case don't matter. What enables Delete account.
#[must_use]
pub fn confirms_deletion(email: &str, typed: &str) -> bool {
    !email.is_empty() && typed.trim().eq_ignore_ascii_case(email.trim())
}

/// How the app looks, in Appearance: as the system is, or always light or dark, by the key
/// it's kept as and the words it's offered with. The system's is first, and what's kept when
/// nothing is.
pub const THEMES: [(&str, &str); 3] = [("system", "System"), ("light", "Light"), ("dark", "Dark")];

/// Whether the app is dark with `theme` kept ([`THEMES`]) and the system `dark` or not.
#[must_use]
pub fn theme_dark(theme: Option<&str>, dark: bool) -> bool {
    match theme {
        Some("light") => false,
        Some("dark") => true,
        _ => dark,
    }
}

/// Signing out of the organisation called `org`, which asks first.
#[must_use]
pub fn sign_out_of(org: &str) -> String {
    format!("Sign out of {org}…")
}

/// The title of a group, the organisation's by `org`, its name.
#[must_use]
pub fn settings_group_title(group: SettingsGroup, org: &str) -> String {
    match group {
        SettingsGroup::Account => SETTINGS_ACCOUNT.to_owned(),
        SettingsGroup::Organisation => org.to_owned(),
        SettingsGroup::App => SETTINGS_APP.to_owned(),
    }
}

impl SettingsKind {
    /// Every kind, in the order Settings lists them.
    pub const ALL: [Self; 13] = [
        Self::Profile,
        Self::Organisations,
        Self::DeleteAccount,
        Self::OrgProfile,
        Self::Status,
        Self::Mute,
        Self::Invite,
        Self::Notifications,
        Self::Appearance,
        Self::Agents,
        Self::Privacy,
        Self::Server,
        Self::About,
    ];

    /// The group it's in.
    #[must_use]
    pub fn group(self) -> SettingsGroup {
        match self {
            Self::Profile | Self::Organisations | Self::DeleteAccount => SettingsGroup::Account,
            Self::OrgProfile | Self::Status | Self::Mute | Self::Invite => {
                SettingsGroup::Organisation
            }
            Self::Notifications
            | Self::Appearance
            | Self::Agents
            | Self::Privacy
            | Self::Server
            | Self::About => SettingsGroup::App,
        }
    }

    /// Its title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Profile => "Name and picture",
            Self::Organisations => ORGANISATIONS,
            Self::DeleteAccount => DELETE_ACCOUNT,
            Self::OrgProfile => "Profile here",
            Self::Status => "Status",
            Self::Mute => "Mute",
            Self::Invite => INVITE_PEOPLE,
            Self::Notifications => "Notifications",
            Self::Appearance => "Appearance",
            // words-guard: the settings section for agents, not the sidebar's part
            Self::Agents => "Agents",
            Self::Privacy => "Privacy",
            Self::Server => SERVER,
            Self::About => "About",
        }
    }

    /// What scripts and tests find it by: stable whatever it's titled.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Organisations => "organisations",
            Self::DeleteAccount => "delete-account",
            Self::OrgProfile => "org-profile",
            Self::Status => "status",
            Self::Mute => "mute",
            Self::Invite => "invite",
            Self::Notifications => "notifications",
            Self::Appearance => "appearance",
            Self::Agents => "agents",
            Self::Privacy => "privacy",
            Self::Server => "server",
            Self::About => "about",
        }
    }

    /// Whether `app` shows it, `signed_in` to zirc.dev or not, to an `admin` of the
    /// organisation shown (its owner too) or a member.
    #[must_use]
    pub fn applies(self, app: SettingsApp, signed_in: bool, admin: bool) -> bool {
        match self {
            // zirc.dev's page for it, which only they may use (the user's choice, 13 Nov
            // 2026: a row here, no longer a button in a header).
            Self::Invite => signed_in && admin,
            // In a browser, the organisations are the rail's and the page has no version;
            // and it notifies of nothing, so there's nothing to choose.
            Self::Organisations | Self::Notifications | Self::About => app != SettingsApp::Web,
            // The account's: none before signing in. Web chat links to zirc.dev's page for it.
            Self::Profile => signed_in,
            // The account's and the organisation's, zirc.dev's own pages' in a browser.
            Self::DeleteAccount | Self::OrgProfile | Self::Status | Self::Mute => {
                signed_in && app != SettingsApp::Web
            }
            // Who may ask a machine's agents is its zircd's, which only a computer has (the
            // user's choice, 11 Nov 2026), as is a server of one's own.
            Self::Agents | Self::Server => app == SettingsApp::Desktop,
            Self::Appearance | Self::Privacy => true,
        }
    }
}

/// Settings' sections for `app`, in order: the account's, the organisation shown's, then
/// the app's, each that applies to it, `signed_in` to zirc.dev or not, as an `admin` of the
/// organisation shown or not.
#[must_use]
pub fn settings_sections(app: SettingsApp, signed_in: bool, admin: bool) -> Vec<SettingsSection> {
    SettingsKind::ALL
        .into_iter()
        .filter(|kind| kind.applies(app, signed_in, admin))
        .map(|kind| SettingsSection {
            kind,
            group: kind.group(),
            title: kind.title().to_owned(),
            id: format!("settings:{}", kind.id()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(app: SettingsApp, signed_in: bool) -> Vec<SettingsKind> {
        settings_sections(app, signed_in, false)
            .into_iter()
            .map(|section| section.kind)
            .collect()
    }

    #[test]
    fn every_app_lists_the_account_then_the_organisation_then_the_app() {
        use SettingsKind::*;
        assert_eq!(
            kinds(SettingsApp::Desktop, true),
            [
                Profile,
                Organisations,
                DeleteAccount,
                OrgProfile,
                Status,
                Mute,
                Notifications,
                Appearance,
                Agents,
                Privacy,
                Server,
                About
            ]
        );
        // The phones choose no server, their sign-in sets it, and have no zircd whose
        // agents anyone may ask.
        assert!(!kinds(SettingsApp::Phone, true).contains(&Server));
        assert!(!kinds(SettingsApp::Phone, true).contains(&Agents));
        // A browser: the name and picture zirc.dev's page edits, how it looks, and whom the
        // person blocks.
        assert_eq!(
            kinds(SettingsApp::Web, true),
            [Profile, Appearance, Privacy]
        );
        // Before signing in, nothing of zirc.dev's but signing in, with Organisations.
        assert_eq!(
            kinds(SettingsApp::Desktop, false),
            [
                Organisations,
                Notifications,
                Appearance,
                Agents,
                Privacy,
                Server,
                About
            ]
        );
        // Inviting people is an owner's or an admin's, in every app, after the organisation's
        // own settings.
        for app in [SettingsApp::Desktop, SettingsApp::Phone, SettingsApp::Web] {
            assert!(!kinds(app, true).contains(&Invite));
            let admins = settings_sections(app, true, true);
            let invite = admins.iter().find(|s| s.kind == Invite).unwrap();
            assert_eq!(
                (invite.title.as_str(), invite.id.as_str(), invite.group),
                (
                    "Invite people",
                    "settings:invite",
                    SettingsGroup::Organisation
                )
            );
            assert!(
                settings_sections(app, false, true)
                    .iter()
                    .all(|s| s.kind != Invite)
            );
        }
        let groups: Vec<SettingsGroup> = settings_sections(SettingsApp::Desktop, true, true)
            .into_iter()
            .map(|section| section.group)
            .collect();
        assert!(groups.is_sorted_by_key(|group| *group as u8));
    }

    #[test]
    fn deleting_says_which_organisations_are_left() {
        assert_eq!(
            deletion_words(&[]),
            "This deletes your account on zirc.dev and signs you out everywhere. Signing up \
             again with the same address makes a new account."
        );
        let words = deletion_words(&["Acme".to_owned(), "Zirc".to_owned()]);
        assert!(words.contains("You leave Acme, Zirc: "), "{words}");
        assert!(words.contains("on their servers are deleted"), "{words}");
        assert!(!words.contains("locked"), "{words}");
    }

    #[test]
    fn deleting_is_confirmed_by_the_address_however_typed() {
        assert!(confirms_deletion(
            "maya@acme.example",
            " Maya@Acme.example "
        ));
        assert!(!confirms_deletion("", ""));
        assert!(!confirms_deletion("maya@acme.example", "maya"));
    }

    #[test]
    fn a_group_is_titled_the_organisations_by_its_name() {
        assert_eq!(
            settings_group_title(SettingsGroup::Organisation, "Acme"),
            "Acme"
        );
        assert_eq!(settings_group_title(SettingsGroup::App, "Acme"), "App");
        assert_eq!(sign_out_of("Acme"), "Sign out of Acme…");
        // The system's, unless one is kept.
        assert!(theme_dark(None, true) && !theme_dark(Some("light"), true));
        assert!(theme_dark(Some("dark"), false) && theme_dark(Some("system"), true));
    }
}
