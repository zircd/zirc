//! Who we are in the organisation, as zirc.dev tells the app (an owner, an admin or a
//! member), and so which of a card's buttons we're shown.

use super::Chat;
use super::conversation::{EntryKind, fold};

impl Chat {
    /// Says whether we're one of the organisation's owners or admins, as zirc.dev tells the
    /// app. Until it's told, nobody is.
    pub fn set_admin(&mut self, admin: bool) {
        self.admin = admin;
    }

    /// Sets [`Chat::set_admin`] from our role in the organisation, as zirc.dev's `/api/me`
    /// gives it: `owner`, `admin` or `member`.
    pub fn set_role(&mut self, role: &str) {
        let role = zirc_proto::role::Role::named(role);
        self.admin = role.is_some_and(zirc_proto::role::Role::is_admin);
        self.owner = role == Some(zirc_proto::role::Role::Owner);
    }

    /// Whether we're one of the organisation's owners or admins: see [`Chat::set_admin`].
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    /// Whether apps show us `card`'s button `action`, as `docs/cards.md` says who may press
    /// it: `members` and `anyone` everyone; `about` whoever the card is about (the owner of
    /// its `about` session, or that nick); `admins` owners and admins; `owners` owners
    /// alone. That only spares a pointless press; the integration decides.
    #[must_use]
    pub fn shows_action(
        &self,
        card: &zirc_proto::card::Card,
        action: &zirc_proto::card::Action,
    ) -> bool {
        use zirc_proto::card::Who;
        match action.who {
            Who::Anyone | Who::Members => true,
            Who::Admins => self.admin,
            Who::Owners => self.owner,
            Who::About => card
                .about
                .as_deref()
                .is_some_and(|about| self.is_about_us(about)),
        }
    }

    /// Whether `about`, a card's agent session or nick, is us: a session one of our
    /// machines has spoken as, our nick, or one of our machines.
    fn is_about_us(&self, about: &str) -> bool {
        let ours = |machine: &str| {
            self.machine_owner(machine)
                .is_some_and(|owner| fold(owner) == fold(&self.me))
        };
        let spoke_as = self
            .conversations
            .iter()
            .flat_map(|conversation| &conversation.entries)
            .find_map(|entry| match &entry.kind {
                EntryKind::Zirc(message) if message.session.eq_ignore_ascii_case(about) => {
                    Some(entry.from.as_str())
                }
                _ => None,
            });
        match spoke_as {
            Some(machine) => ours(machine),
            None => fold(about) == fold(&self.me) || ours(about),
        }
    }
}
