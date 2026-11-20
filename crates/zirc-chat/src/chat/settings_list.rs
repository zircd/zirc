//! Settings as every app lists it (the user's choice, 11 Nov 2026): one ordered list of
//! sections in three groups, the account's, then the organisation shown's, then the app's,
//! each with its title. Every app draws it its own way, pages in the desktop's sheet and
//! sections on the Mac, the phones and the web, but in this order with these titles, and
//! shows only the sections that apply to it ([`settings_sections`]). What a section holds,
//! its rows, is each app's to draw from the words the core gives it; the words several apps
//! say alike are here.
//!
//! A change applies as it's made, in every app; only typed fields, a server's, have a Save of
//! their own.
//!
//! The account is zirc.dev's alone (the user's choice, 20 Nov 2026): its one name and one
//! picture, the handle in each organisation and deleting it are edited on zirc.dev's account
//! page, which every app's [`SettingsKind::Profile`] opens with [`super::EDIT_PROFILE`]. Apps
//! keep what concerns the device, and whether the person is away and whom they block.

use serde::Serialize;
use ts_rs::TS;

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
    /// Web chat, in a browser: how it looks and whom the person blocks. It shows no
    /// notifications to choose among.
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
    /// The account, zirc.dev's: one row, [`super::EDIT_PROFILE`], that opens its page there,
    /// where its name, picture and handles are set and it's deleted.
    Profile,
    /// The organisations signed in to, each with the person's handle there, its connection
    /// and signing out of it; adding one, and signing out of everything; before signing in,
    /// signing in.
    Organisations,
    /// Whether the person is away there, and why.
    Status,
    /// Muting the organisation: nothing of it notifies.
    Mute,
    /// The organisation's keys for sealed parts of messages this device holds, and adding
    /// one a teammate handed over ([`super::sealing`]).
    Sealed,
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

/// The title of the organisation's group while it has no name: before the first sign-in, or
/// on a server of one's own.
pub const SETTINGS_ORGANISATION: &str = "Organisation";

/// Settings' own name: its page's title, and what opens it.
pub const SETTINGS: &str = "Settings";

/// The organisations signed in to: [`SettingsKind::Organisations`]' title, and a list's or
/// a menu's of them anywhere else. The web page types it too where it asks which to open,
/// drawn before its wasm is loaded (`oss/web/src/App.tsx`).
pub const ORGANISATIONS: &str = "Organisations";

/// The server a sign-in is to: [`SettingsKind::Server`]'s title, and the label of its
/// address wherever that shows. The web page types it too on its dev sign-in, drawn before
/// its wasm is loaded (`oss/web/src/App.tsx`).
pub const SERVER: &str = "Server";

/// Adding an organisation, which opens zirc.dev's sign-in.
pub const ADD_ORGANISATION: &str = "Add organisation…";

/// [`ADD_ORGANISATION`] where it's a button's name or asks nothing first, without the
/// ellipsis.
pub const ADD_ORGANISATION_NAME: &str = "Add organisation";

/// Signing out of every organisation and zirc.dev, which asks first: a menu's item, a
/// button that opens the question. The question's title and its button are
/// [`crate::orgs::SIGN_OUT_EVERYTHING_NAME`], the same words without the ellipsis.
pub const SIGN_OUT_EVERYTHING: &str = "Sign out of everything…";

/// The system's own notification settings, which open apart.
pub const SYSTEM_NOTIFICATIONS: &str = "Notification settings…";

/// The switch in an organisation's Status: whether people there see the person as away.
// words-guard: a switch named Away, not a person's status
pub const AWAY: &str = "Away";

/// The field under [`AWAY`]: why, which people there see after "away:".
pub const AWAY_WHY: &str = "Why (optional)";

/// Under [`AWAY`] and [`AWAY_WHY`]: what they do, in every app (the user's choice, 17 Nov
/// 2026).
pub const AWAY_NOTE: &str = "Others see you as away, and why, until you're back.";

/// [`AWAY_NOTE`] while this app says away but the server shows the person here.
pub const AWAY_ELSEWHERE_NOTE: &str =
    "Shown as here while another of your apps is connected and not away.";

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

/// How the app looks, in Appearance: as the system is, or always light or dark, by the key
/// it's kept as and the words it's offered with. The system's is first, and what's kept when
/// nothing is.
pub const THEMES: [(&str, &str); 3] = [("system", "System"), ("light", "Light"), ("dark", "Dark")];

/// One of Appearance's looks ([`THEMES`]), as every app offers it: the phones read this
/// record through `UniFFI`, the web chat as JSON.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct ThemeChoice {
    /// What it's kept as: `system`, `light` or `dark`.
    pub key: String,
    /// What Appearance offers it as.
    pub label: String,
}

/// Appearance's looks, the system's first ([`THEMES`]).
#[must_use]
pub fn themes() -> Vec<ThemeChoice> {
    THEMES
        .iter()
        .map(|(key, label)| ThemeChoice {
            key: (*key).to_owned(),
            label: (*label).to_owned(),
        })
        .collect()
}

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

/// The title of a group, the organisation's by `org`, its name, or [`SETTINGS_ORGANISATION`]
/// while it has none, never a blank heading.
#[must_use]
pub fn settings_group_title(group: SettingsGroup, org: &str) -> String {
    match group {
        SettingsGroup::Account => SETTINGS_ACCOUNT.to_owned(),
        SettingsGroup::Organisation if org.trim().is_empty() => SETTINGS_ORGANISATION.to_owned(),
        SettingsGroup::Organisation => org.to_owned(),
        SettingsGroup::App => SETTINGS_APP.to_owned(),
    }
}

impl SettingsKind {
    /// Every kind, in the order Settings lists them.
    pub const ALL: [Self; 11] = [
        Self::Profile,
        Self::Organisations,
        Self::Status,
        Self::Mute,
        Self::Sealed,
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
            Self::Profile | Self::Organisations => SettingsGroup::Account,
            Self::Status | Self::Mute | Self::Sealed => SettingsGroup::Organisation,
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
            Self::Profile => super::PROFILE,
            Self::Organisations => ORGANISATIONS,
            Self::Status => "Status",
            Self::Mute => "Mute",
            Self::Sealed => super::sealing::SEALED_MESSAGES,
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
            Self::Status => "status",
            Self::Mute => "mute",
            Self::Sealed => "sealed",
            Self::Notifications => "notifications",
            Self::Appearance => "appearance",
            Self::Agents => "agents",
            Self::Privacy => "privacy",
            Self::Server => "server",
            Self::About => "about",
        }
    }

    /// Whether `app` shows it, `signed_in` to zirc.dev or not.
    #[must_use]
    pub fn applies(self, app: SettingsApp, signed_in: bool) -> bool {
        match self {
            // In a browser, the organisations are the rail's, and it notifies of nothing, so
            // there's nothing to choose.
            Self::Organisations | Self::Notifications => app != SettingsApp::Web,
            // The account's link to zirc.dev: none before signing in. Away and why, in every
            // app's Settings (the user's choice, 17 Nov 2026).
            Self::Profile | Self::Status => signed_in,
            // What notifies of the organisation, which a browser doesn't.
            Self::Mute => signed_in && app != SettingsApp::Web,
            // Who may ask a machine's agents is its zircd's, which only a computer has (the
            // user's choice, 11 Nov 2026), as is a server of one's own.
            Self::Agents | Self::Server => app == SettingsApp::Desktop,
            // Keys are any device's, for any server; the phones' page comes with their
            // sealed parts.
            Self::Sealed => app != SettingsApp::Phone,
            // Every app's. A browser's About has no version, only the typefaces' licences,
            // which go wherever their files do.
            Self::Appearance | Self::Privacy | Self::About => true,
        }
    }
}

/// Settings' sections for `app`, in order: the account's, the organisation shown's, then
/// the app's, each that applies to it, `signed_in` to zirc.dev or not.
#[must_use]
pub fn settings_sections(app: SettingsApp, signed_in: bool) -> Vec<SettingsSection> {
    SettingsKind::ALL
        .into_iter()
        .filter(|kind| kind.applies(app, signed_in))
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
        settings_sections(app, signed_in)
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
                Status,
                Mute,
                Sealed,
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
        // A browser: the link to zirc.dev's account page, away and why, how it looks, and
        // whom the person blocks.
        assert_eq!(
            kinds(SettingsApp::Web, true),
            [Profile, Status, Sealed, Appearance, Privacy, About]
        );
        // Before signing in, nothing of zirc.dev's but signing in, with Organisations.
        assert_eq!(
            kinds(SettingsApp::Desktop, false),
            [
                Organisations,
                Sealed,
                Notifications,
                Appearance,
                Agents,
                Privacy,
                Server,
                About
            ]
        );
        let groups: Vec<SettingsGroup> = settings_sections(SettingsApp::Desktop, true)
            .into_iter()
            .map(|section| section.group)
            .collect();
        assert!(groups.is_sorted_by_key(|group| *group as u8));
        // The account is one row that opens zirc.dev, in every app (20 Nov 2026).
        let profile = &settings_sections(SettingsApp::Phone, true)[0];
        assert_eq!(
            (profile.title.as_str(), profile.id.as_str()),
            ("Profile", "settings:profile")
        );
    }

    #[test]
    fn a_group_is_titled_the_organisations_by_its_name() {
        assert_eq!(
            settings_group_title(SettingsGroup::Organisation, "Acme"),
            "Acme"
        );
        assert_eq!(settings_group_title(SettingsGroup::App, "Acme"), "App");
        // Unnamed, as on the desktop's first run: a word, never a blank heading.
        assert_eq!(
            settings_group_title(SettingsGroup::Organisation, ""),
            SETTINGS_ORGANISATION
        );
        assert_eq!(sign_out_of("Acme"), "Sign out of Acme…");
        // The system's, unless one is kept.
        assert!(theme_dark(None, true) && !theme_dark(Some("light"), true));
        assert!(theme_dark(Some("dark"), false) && theme_dark(Some("system"), true));
    }
}
