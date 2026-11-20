//! The team's own hints, kept on its server ([`zirc_core::team_hints`]): read when zircd starts,
//! and again whenever zirc's own account says they changed ([`Changed`]), in the background,
//! and kept on this machine for `zirc watch`, `zirc hints` and `zirc playbook`. Only zirc's
//! account's word counts, as the server says who sent it: anyone else saying so is ignored.

use zirc_core::ipc::Delivery;
use zirc_proto::hints::Changed;

use super::Daemon;

impl Daemon {
    /// Reads the team's hints from the server in the background and keeps them, when the
    /// server has this machine's key.
    pub(super) fn read_team_hints(&self) {
        let (name, first) = (self.org.name.clone(), self.org.first);
        let Some(org) = self
            .config
            .ask(|settings| crate::config::find(settings, &name, first))
        else {
            return;
        };
        let Some(server) = zirc_core::team_hints::Server::of(&self.home, &org) else {
            return;
        };
        let home = self.home.clone();
        tokio::spawn(async move {
            match server.list().await {
                Ok(listing) => {
                    if let Err(err) = zirc_core::team_hints::keep(&home, &name, &listing) {
                        tracing::warn!("couldn't keep the team's hints: {err}");
                    }
                }
                Err(why) => tracing::debug!("couldn't read the team's hints: {why}"),
            }
        });
    }

    /// Reads the team's hints again if `delivery` is zirc's own account saying they changed
    /// since the version kept here.
    pub(super) fn note_hints_changed(&self, delivery: &Delivery) {
        let zircs = delivery
            .account
            .as_deref()
            .is_some_and(|account| account.eq_ignore_ascii_case(zirc_proto::names::SYSTEM));
        let Some(changed) = Changed::of(&delivery.message.event).filter(|_| zircs) else {
            return;
        };
        let kept = zirc_core::team_hints::kept(&self.home, &self.org.name);
        if changed.version > kept.version {
            self.read_team_hints();
        }
    }
}
