//! The words and tile bits of several organisations, said once for every app: what the
//! design (`multi-org-ux.md`, sections 1 to 4 and 9) puts on the sign-out sheets, in the
//! toasts, as the composer's placeholder, and on an organisation's tile (its initials and
//! colour). The apps only lay them out.
//!
//! With one organisation nothing here changes what someone sees: the rail, the organisation
//! in titles and in the composer appear with two or more, and the callers say so. The tile's
//! colour and initials and the composer's placeholder are zirc-chat's `orgs` (`org_colour`,
//! `org_initials`, `composer_placeholder`), which the web chat draws too; this adds what only
//! the core knows: organisations' names and the sheets and toasts about them.

use zirc_chat::chat::Chat;
pub use zirc_chat::orgs::{
    Card, CardAction, SignOutSheet, display_name, org_card, sign_out_label, sign_out_sheet,
    signed_out,
};
use zirc_chat::orgs::{
    SignOutPlace, org_badge_label, org_colour, org_dot, org_initials, tile_a11y,
};

use crate::account::{Account, OrgState, OrgSummary};

/// What is said when the most organisations are signed in to.
#[must_use]
pub fn too_many(max: usize) -> String {
    format!(
        "You can have up to {max} organisations on this device. Sign out of one to add another."
    )
}

/// How the person switches, for the toast after adding an organisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchHint {
    /// A rail, with the key that opens the new one's place, such as `⌘2`.
    Rail(Option<String>),
    /// A phone's menu under the organisation shown, such as `Acme`.
    Menu(String),
}

/// The toast the first time another organisation is added.
#[must_use]
pub fn added_toast(added: &str, hint: &SwitchHint) -> String {
    let added = display_name(added);
    match hint {
        SwitchHint::Rail(Some(key)) => format!("Added {added}. Switch with the rail or {key}."),
        SwitchHint::Rail(None) => format!("Added {added}. Switch with the rail."),
        SwitchHint::Menu(current) => {
            format!(
                "Added {added}. Switch from the {} ▾ menu.",
                display_name(current)
            )
        }
    }
}

/// The toast after a rename was followed without asking the person to sign in again.
#[must_use]
pub fn renamed_toast(org: &str, handle: &str) -> String {
    format!("Your name in {} is now @{handle}", display_name(org))
}

impl OrgSummary {
    /// The name people read: the one its header shows ([`OrgSummary::title`]), else one made
    /// from `org` ([`display_name`]), so the tile's initials and the header agree even on a
    /// server known only by its address.
    #[must_use]
    pub fn name(&self) -> String {
        self.title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| display_name(&self.org))
    }

    /// The tile's initials, zirc-chat's ([`org_initials`]).
    #[must_use]
    pub fn initials(&self) -> String {
        org_initials(&self.name())
    }

    /// The tile's colour, its place in zirc-chat's [`zirc_chat::orgs::ORG_PALETTE`] ([`org_colour`]).
    #[must_use]
    pub fn colour(&self) -> u8 {
        org_colour(&self.org)
    }
}

/// The card an organisation shows in place of its conversations when it can't be used
/// (section 4), as every app says it: zirc-chat's [`org_card`], for `org` as people read it.
#[must_use]
pub fn card(state: OrgState, org: &str) -> Option<Card> {
    org_card(state, &display_name(org))
}

/// What notifications from `conversation` in the organisation `org` are grouped by, so two
/// `#general` never merge (section 6): `acme/#general`.
#[must_use]
pub fn thread_id(org: &str, conversation: &str) -> String {
    format!("{org}/{conversation}")
}

impl OrgSummary {
    /// Its second line in a switcher: the handle there, and the state when it isn't
    /// connected, `@sam · Offline`.
    #[must_use]
    pub fn line(&self) -> String {
        if self.state == OrgState::Connected {
            format!("@{}", self.handle)
        } else {
            format!("@{} · {}", self.handle, self.state.label())
        }
    }

    /// What a screen reader says of its tile, zirc-chat's ([`tile_a11y`]):
    /// `Acme, 3 mentions, connecting`.
    #[must_use]
    pub fn a11y_label(&self) -> String {
        tile_a11y(&self.name(), self.unread, self.state)
    }

    /// What its badge says, zirc-chat's ([`org_badge_label`]): `3`, `99+`; none for nothing,
    /// nor once access ended.
    #[must_use]
    pub fn badge(&self) -> Option<String> {
        org_badge_label(self.state, self.unread.mentions)
    }

    /// Whether it shows a dot in place of a badge, zirc-chat's ([`org_dot`]): something
    /// unread, nothing the badge counts.
    #[must_use]
    pub fn dot(&self) -> bool {
        org_dot(self.state, self.unread)
    }

    /// Its card, if it shows one ([`card`]).
    #[must_use]
    pub fn card(&self) -> Option<Card> {
        org_card(self.state, &self.name())
    }
}

/// What the sheet for signing out of everything says of organisations `orgs`, each with
/// whether it is self-hosted: zirc-chat's [`zirc_chat::orgs::sign_out_everything_sheet`],
/// on this device, naming zirc.dev when any of them is on it.
#[must_use]
pub fn sign_out_everything_sheet(orgs: &[(String, bool)]) -> SignOutSheet {
    let names: Vec<String> = orgs.iter().map(|(org, _)| display_name(org)).collect();
    let zirc_dev = orgs.iter().any(|(_, self_hosted)| !self_hosted);
    zirc_chat::orgs::sign_out_everything_sheet(&names, zirc_dev, SignOutPlace::Device)
}

impl<T: AsRef<Chat> + AsMut<Chat>> Account<T> {
    /// What the sheet for signing out of `org` says ([`sign_out_sheet`]). `None` when not
    /// signed in to it.
    #[must_use]
    pub fn sign_out_text(&self, org: &str) -> Option<SignOutSheet> {
        let session = self.get(org)?;
        let others: Vec<String> = self
            .iter()
            .filter(|(other, _)| *other != org)
            .map(|(other, _)| other.to_owned())
            .collect();
        Some(sign_out_sheet(org, &others, session.self_hosted()))
    }

    /// What the sheet for signing out of everything says ([`sign_out_everything_sheet`]).
    #[must_use]
    pub fn sign_out_everything_text(&self) -> SignOutSheet {
        let orgs: Vec<(String, bool)> = self
            .iter()
            .map(|(org, session)| (org.to_owned(), session.self_hosted()))
            .collect();
        sign_out_everything_sheet(&orgs)
    }

    /// What the toast after adding says, the first time: `None` unless another organisation
    /// is signed in to as well as `added`.
    #[must_use]
    pub fn added_text(&self, added: &str, hint: &SwitchHint) -> Option<String> {
        (self.len() > 1 && self.get(added).is_some()).then(|| added_toast(added, hint))
    }
}

#[cfg(test)]
mod tests;
