//! Signing a machine out, as `zirc logout` and `zirc orgs remove` do it: what leaves, what
//! stays, what the sheet asks (the apps' words, [`crate::account_text`]), and the key the
//! machine tells zirc.dev it no longer holds.
//!
//! A machine's sign-in is `config.toml`'s organisations ([`Settings`]): its own, each with
//! the machine's account on that server and its password, and guests' terminals, which have
//! no account. The meaning is the apps' ([`crate::account::Removal::ends_zirc_dev`]): signing
//! out of the last of one's own organisations also signs out of zirc.dev
//! ([`LAST_SIGN_OUT_ENDS_ZIRC_DEV`]).

use crate::account::LAST_SIGN_OUT_ENDS_ZIRC_DEV;
use crate::account_text::{SignOutSheet, sign_out_everything_sheet, sign_out_sheet};
use crate::device_key::DeviceKey;
use crate::login::Client;
use std::path::Path;

use crate::settings::{Organisation, Settings};

/// What to sign out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every organisation, guest terminals and zirc.dev.
    Everything,
    /// One organisation, or one guest's terminal, by its name.
    One(String),
}

/// One organisation of the machine's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// What zirc calls it.
    pub org: String,
    /// The public channel, for a guest's terminal.
    pub guest: Option<String>,
    /// Whether it has no account on zirc.dev: a server of one's own, or a guest's terminal.
    pub self_hosted: bool,
}

impl Held {
    fn of(org: &Organisation) -> Self {
        Self {
            org: org.name(),
            guest: org.guest.as_ref().map(|guest| guest.public.clone()),
            self_hosted: org.web.is_none() || org.guest.is_some(),
        }
    }
}

/// Why nothing can be signed out of.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// The machine has no organisation named so.
    #[error("this machine isn't signed in to {0}")]
    NotSignedIn(String),
}

/// What signing out of a scope does to a machine's [`Settings`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// What goes.
    pub leaving: Vec<Held>,
    /// What stays.
    pub staying: Vec<Held>,
    /// Whether the scope was everything.
    pub everything: bool,
}

impl Plan {
    /// What signing out of `scope` does to `settings`.
    ///
    /// # Errors
    ///
    /// When `scope` names an organisation the machine isn't signed in to.
    pub fn new(settings: &Settings, scope: &Scope) -> Result<Self, Error> {
        let all: Vec<Held> = settings.organisations().iter().map(Held::of).collect();
        match scope {
            Scope::Everything => Ok(Self {
                leaving: all,
                staying: Vec::new(),
                everything: true,
            }),
            Scope::One(name) => {
                if !all.iter().any(|held| &held.org == name) {
                    return Err(Error::NotSignedIn(name.clone()));
                }
                let (leaving, staying) = all.into_iter().partition(|held| &held.org == name);
                Ok(Self {
                    leaving,
                    staying,
                    everything: false,
                })
            }
        }
    }

    /// Whether nothing stays: the machine is signed out of everything.
    #[must_use]
    pub fn leaves_nothing(&self) -> bool {
        self.staying.is_empty()
    }

    /// Whether this also signs the machine out of zirc.dev: the last of its own
    /// organisations on it goes ([`LAST_SIGN_OUT_ENDS_ZIRC_DEV`]), as in the apps.
    #[must_use]
    pub fn ends_zirc_dev(&self) -> bool {
        let own = |held: &&Held| held.guest.is_none();
        LAST_SIGN_OUT_ENDS_ZIRC_DEV
            && self
                .leaving
                .iter()
                .filter(own)
                .any(|held| !held.self_hosted)
            && !self.staying.iter().any(|held| own(&held))
    }

    /// The sheet it asks: the apps' words.
    #[must_use]
    pub fn sheet(&self) -> SignOutSheet {
        match self.leaving.as_slice() {
            [one] if !self.everything => {
                let others: Vec<String> =
                    self.staying.iter().map(|held| held.org.clone()).collect();
                sign_out_sheet(&one.org, &others, !self.ends_zirc_dev())
            }
            leaving => sign_out_everything_sheet(
                &leaving
                    .iter()
                    .map(|held| (held.org.clone(), held.self_hosted))
                    .collect::<Vec<_>>(),
            ),
        }
    }
}

/// Tells zirc.dev this machine's key no longer signs in to its server of `org`, as the
/// machine's own account there (`DELETE /api/me/credentials`; it works while zirc.dev's
/// account credential is off). `Ok(false)` when there is nothing to tell: no account, no key
/// registered, or a server of one's own.
///
/// # Errors
///
/// What went wrong, in words, when zirc.dev can't be reached or refuses.
pub async fn revoke_key(org: &Organisation, key: &dyn DeviceKey) -> Result<bool, String> {
    let (Some(account), Some(web)) = (&org.account, &org.web) else {
        return Ok(false);
    };
    if !account.device_key || org.guest.is_some() {
        return Ok(false);
    }
    let public = key
        .public_key()
        .ok_or_else(|| "this machine's key can't be read".to_owned())?;
    let client = Client::new(web).map_err(|err| err.to_string())?;
    client
        .remove_key_registration(account, &org.name(), &public)
        .await
        .map(|()| true)
        .map_err(|err| err.to_string())
}

/// Takes what `plan` leaves out of `settings`, saved at `path`: the organisations it names,
/// or, when it leaves nothing, `config.toml` itself and the machine's device key, as a
/// machine never signed in has neither. `zirc logout` does it, and zircd for a guest's
/// terminal zirc.dev forgot.
///
/// # Errors
///
/// When `config.toml` can't be written or removed.
pub fn forget(
    home: &Path,
    path: &Path,
    settings: &mut Settings,
    plan: &Plan,
) -> std::io::Result<()> {
    if plan.leaves_nothing() {
        std::fs::remove_file(path)?;
        let _ = std::fs::remove_file(crate::paths::device_key(home));
        return Ok(());
    }
    for held in &plan.leaving {
        settings.take_organisation(&held.org);
    }
    settings.save(path)
}

#[cfg(test)]
mod tests;
