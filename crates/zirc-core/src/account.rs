//! The account an app holds: one [`OrgSession`] per organisation the person is signed in to,
//! one of them selected, as Slack holds several workspaces. It is the one container desktop,
//! phones and the TUI share, so what "sign out of Zirc" or "which organisation does this push
//! belong to" means exists once and the apps stay thin.
//!
//! Each organisation is a world of its own:
//!
//! - its [`SignIn`] (handle, server, its own zirc.dev `web` address), whose passphrase is kept
//!   under `handle@host` ([`crate::secrets::key`]) and is only ever read together with that
//!   host;
//! - its [`Chat`], connection [`generation`](OrgSession::generation) and
//!   [`state`](OrgSession::state);
//! - its push endpoint, the server's registration of the device key, its blocked list, its
//!   notification levels (in the chat) and badge, kept under the organisation ([`OrgStore`]),
//!   never under the handle, which can be renamed;
//! - whatever else the app keeps per organisation, the generic `T` (the desktop's session, a
//!   phone's media cache).
//!
//! Every call that needs a credential goes through [`OrgSession::call`], which pairs the
//! credential with the client of *that* sign-in's `web`, so a self-hosted server on another
//! hostname gets its own passphrase and zirc.dev never sees it. One organisation failing,
//! offline or revoked never blocks another: sessions share nothing.
//!
//! Several organisations are behind a rollout switch, [`Policy::several`], off by default:
//! with it off [`Account::add_organisation`] refuses a second organisation and an app behaves
//! as it did with one, but the storage is already per organisation.

use std::sync::Arc;

use zirc_chat::chat::Chat;
use zirc_chat::orgs::{Orgs, Unread};
use zirc_proto::login::Account as Credential;
use zirc_proto::report::BlockList;

use crate::login::{self, Client};
use crate::secrets::{self, Secrets};
use crate::signin::{self, SignIn, SignIns};

/// The most organisations an app holds.
pub const MAX_ORGANISATIONS: usize = 8;

/// Whether signing out of the last organisation also signs out of zirc.dev (the browser's
/// session used for signing in), as Mattias decided on 7 Nov 2026 ("Always also signs out of
/// zirc.dev, with no question and no checkbox"). A constant so a per-organisation variant can
/// come back: [`Removal::ends_zirc_dev`] and [`SignedOut::ends_zirc_dev`] read it, and the apps
/// act on those, never on this.
pub const LAST_SIGN_OUT_ENDS_ZIRC_DEV: bool = true;

/// What the person's apps keep for an organisation in the secret store, under
/// [`org_key`]. Before several organisations these were kept unkeyed ([`LEGACY_KEYS`]).
pub mod keys {
    /// How much each conversation notifies, as JSON.
    pub const NOTIFY: &str = "zirc-notify-settings";
    /// The count the app's icon showed last for it.
    pub const BADGE: &str = "zirc-badge";
    /// Whom the person blocked there, as JSON.
    pub const BLOCKED: &str = "zirc-blocked";
    /// The nick the push path shows the organisation's notifications for.
    pub const PUSH_NICK: &str = "zirc-push-nick";
    /// Whether the person muted the whole organisation (`1`).
    pub const MUTED: &str = "zirc-org-muted";
    /// How many pushes for it the app hasn't seen read: its count while not connected.
    pub const PUSHED: &str = "zirc-org-pushed";
    /// When (unix seconds) its server last took the push registration.
    pub const REGISTERED_AT: &str = "zirc-push-registered";
    /// The id zirc.dev gave its push endpoint, which every push for it carries as `ep`.
    pub const ENDPOINT: &str = "zirc-push-endpoint";
    /// Getting started's prompts the person put away there, which the apps kept until 27 Nov
    /// 2026: none writes it now, and signing out still deletes it.
    pub const PUT_AWAY: &str = "zirc-prompts-put-away";
}

/// The names the apps kept, one each for their only organisation, before several. They move
/// under the organisation on first start ([`Account::open`]); the iOS notification extension
/// still reads them, so while the rollout switch is off they are written as well
/// ([`OrgStore::set`]).
pub const LEGACY_KEYS: [&str; 4] = [keys::NOTIFY, keys::BADGE, keys::BLOCKED, keys::PUSH_NICK];

/// The name `name` is kept under for the organisation `org` (named as
/// [`crate::settings::org_name`] names it): never the handle.
#[must_use]
pub fn org_key(name: &str, org: &str) -> String {
    format!("{name}:{org}")
}

/// The secret store, per organisation.
#[derive(Clone)]
pub struct OrgStore {
    secrets: Arc<dyn Secrets>,
    /// Whether the unkeyed names are written too, for readers that don't know
    /// organisations yet: on while there's one organisation and several are off.
    mirror: bool,
}

impl std::fmt::Debug for OrgStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrgStore")
            .field("mirror", &self.mirror)
            .finish_non_exhaustive()
    }
}

impl OrgStore {
    /// The secret store itself, for what isn't per organisation: passphrases, the account id.
    #[must_use]
    pub fn secrets(&self) -> &Arc<dyn Secrets> {
        &self.secrets
    }

    /// What `org` kept as `name`.
    #[must_use]
    pub fn get(&self, org: &str, name: &str) -> Option<String> {
        self.secrets.get(&org_key(name, org))
    }

    /// Keeps `value` as `org`'s `name`; with mirroring, as the unkeyed name too.
    ///
    /// # Errors
    ///
    /// When the store refuses it.
    pub fn set(&self, org: &str, name: &str, value: &str) -> Result<(), secrets::Error> {
        self.secrets.set(&org_key(name, org), value)?;
        if self.mirror && LEGACY_KEYS.contains(&name) {
            self.secrets.set(name, value)?;
        }
        Ok(())
    }

    /// Forgets everything kept for `org`, and with mirroring the unkeyed names too.
    ///
    /// # Errors
    ///
    /// When the store refuses.
    pub fn forget(&self, org: &str) -> Result<(), secrets::Error> {
        for name in [
            keys::NOTIFY,
            keys::BADGE,
            keys::BLOCKED,
            keys::PUSH_NICK,
            keys::MUTED,
            keys::PUSHED,
            keys::REGISTERED_AT,
            keys::ENDPOINT,
            keys::PUT_AWAY,
        ] {
            self.secrets.delete(&org_key(name, org))?;
            if self.mirror && LEGACY_KEYS.contains(&name) {
                self.secrets.delete(name)?;
            }
        }
        Ok(())
    }

    /// The first start with organisation-keyed storage: whatever the unkeyed names hold moves
    /// under `org`, the organisation the old single-organisation app was signed in to (the
    /// first, when it had several), unless `org` has a value already. The unkeyed names stay
    /// while mirroring, and go otherwise. Safe to run on every start.
    ///
    /// # Errors
    ///
    /// When the store refuses.
    pub fn adopt_legacy(&self, org: &str) -> Result<(), secrets::Error> {
        for name in LEGACY_KEYS {
            let Some(value) = self.secrets.get(name) else {
                continue;
            };
            if self.get(org, name).is_none() {
                self.secrets.set(&org_key(name, org), &value)?;
            }
            if !self.mirror {
                self.secrets.delete(name)?;
            }
        }
        Ok(())
    }
}

/// Where an organisation's connection stands, as every app says it (zirc-chat's).
pub use zirc_chat::orgs::OrgState;

/// The push endpoint an organisation has: one device token is shared by every organisation,
/// but each server has its own registration of it. The phone-wide push keys are not here:
/// they go only when the last organisation does ([`SignedOut::delete_push_keys`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushEndpoint {
    /// The platform's device token.
    pub token: Option<String>,
    /// Whether this organisation's server (through its zirc.dev) has the token.
    pub registered: bool,
    /// When (unix seconds) it last took the registration: the phone's own, or zirc.dev's for
    /// it while another organisation is shown.
    pub registered_at: Option<i64>,
    /// The id of the endpoint zirc.dev made for it (the last part of the endpoint's URL):
    /// pushes carry only this, as nothing about organisations goes to Apple or Google, and
    /// the app looks the organisation up by it ([`Account::org_for_endpoint`]).
    pub endpoint_id: Option<String>,
}

/// What a call to an organisation's zirc.dev needs, always from the same sign-in: the client
/// of its own `web` address, and the credential for its own server.
#[derive(Debug, Clone)]
pub struct Call {
    /// A client of this sign-in's `web`.
    pub client: Client,
    /// The sign-in's account and passphrase.
    pub credential: Credential,
}

impl Call {
    /// The client and credential for `signin`, from `secrets`.
    ///
    /// # Errors
    ///
    /// When the passphrase is gone from `secrets`, there is none, or the client can't be made.
    pub fn of(signin: &SignIn, secrets: &dyn Secrets) -> Result<Self, signin::Error> {
        let client = Client::new(&signin.web)?;
        let key = signin.key();
        let password = secrets
            .get(&key)
            .filter(|_| signin.account)
            .ok_or(signin::Error::PassphraseGone { key })?;
        Ok(Self {
            client,
            credential: Credential {
                name: signin.handle.clone(),
                password,
                device_key: false,
            },
        })
    }
}

/// Everything held for one organisation.
#[derive(Debug)]
pub struct OrgSession<T = Chat> {
    signin: SignIn,
    generation: u64,
    state: OrgState,
    /// This organisation's push endpoint.
    pub push: PushEndpoint,
    /// Whom the person blocked here.
    pub blocked: BlockList,
    /// Whether the organisation is muted: its unread stays out of the total badge.
    pub muted: bool,
    /// The count the icon last showed for it, which pushes add to.
    pub badge: u32,
    /// Pushes for it not yet seen read: the notification extension adds one per push and
    /// takes one off on a MARKREAD push. It is the organisation's unread while it isn't
    /// connected; opening it recounts from the server ([`OrgSession::recounted`]).
    pub pushed: u32,
    /// What the app keeps for it: its chat with the organisation's server, inside whatever
    /// the app holds it in (zirc-app's session), or the chat itself.
    pub ext: T,
}

impl<T: AsRef<Chat>> AsRef<Chat> for OrgSession<T> {
    fn as_ref(&self) -> &Chat {
        self.ext.as_ref()
    }
}

impl<T: AsRef<Chat> + AsMut<Chat>> OrgSession<T> {
    /// A session for `signin`, its chat in `ext`, not connected yet.
    pub fn new(signin: SignIn, ext: T) -> Self {
        Self {
            signin,
            generation: 0,
            state: OrgState::Connecting,
            push: PushEndpoint::default(),
            blocked: BlockList::default(),
            muted: false,
            badge: 0,
            pushed: 0,
            ext,
        }
    }

    /// The chat with the organisation's server.
    #[must_use]
    pub fn chat(&self) -> &Chat {
        self.ext.as_ref()
    }

    /// The chat, to change.
    pub fn chat_mut(&mut self) -> &mut Chat {
        self.ext.as_mut()
    }

    /// The organisation, named as every client names it.
    #[must_use]
    pub fn org(&self) -> String {
        self.signin.org()
    }

    /// Who the person signed in as, and where.
    #[must_use]
    pub fn signin(&self) -> &SignIn {
        &self.signin
    }

    /// The connection's number: it grows with each connection made, and what a connection
    /// reports is only believed while it is [current](Self::is_current).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Starts a new connection: the number it should report with, the earlier one's now old.
    pub fn reconnect(&mut self) -> u64 {
        self.generation += 1;
        self.state = OrgState::Connecting;
        self.generation
    }

    /// Whether `generation` is the connection in use.
    #[must_use]
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Where the connection stands.
    #[must_use]
    pub fn state(&self) -> OrgState {
        self.state
    }

    /// Says where the connection stands, for a connection that is `generation`; an old one's
    /// word is ignored. Access ended stays until a new sign-in.
    pub fn set_state(&mut self, generation: u64, state: OrgState) {
        if self.is_current(generation) && self.state != OrgState::AccessEnded {
            self.state = state;
        }
    }

    /// Whether its server isn't one of zirc.dev's, so its `web` and push are its own.
    #[must_use]
    pub fn self_hosted(&self) -> bool {
        self.signin.org() == self.signin.server.host
    }

    /// The client and credential for a call to this organisation's zirc.dev: only ever
    /// together, and only for this sign-in.
    ///
    /// # Errors
    ///
    /// When the passphrase is gone from `secrets`, or the client can't be made.
    pub fn call(&self, secrets: &dyn Secrets) -> Result<Call, signin::Error> {
        Call::of(&self.signin, secrets)
    }

    /// Says where the connection stands, whatever connection said it: for the app's own
    /// knowledge, as that the account's membership is gone.
    pub(crate) fn set_state_now(&mut self, state: OrgState) {
        self.state = state;
    }

    /// Replaces the sign-in with a fresh one for the same organisation, after zirc.dev gave
    /// new credentials: the data stays, the connection is made again and access is back.
    pub fn replace_signin(&mut self, signin: SignIn) {
        self.signin = signin;
        self.reconnect();
    }

    /// Loads what [`OrgStore`] keeps for the organisation: its notification levels, blocked
    /// list, badge and mute. What can't be read is left at its default.
    pub fn load(&mut self, store: &OrgStore) {
        let org = self.org();
        if let Some(settings) = store
            .get(&org, keys::NOTIFY)
            .and_then(|json| serde_json::from_str(&json).ok())
        {
            self.chat_mut().set_notify_settings(settings);
        }
        if let Some(blocked) = store
            .get(&org, keys::BLOCKED)
            .and_then(|json| serde_json::from_str(&json).ok())
        {
            self.blocked = blocked;
        }
        self.badge = store
            .get(&org, keys::BADGE)
            .and_then(|count| count.parse().ok())
            .unwrap_or(0);
        self.muted = store.get(&org, keys::MUTED).as_deref() == Some("1");
        self.pushed = store
            .get(&org, keys::PUSHED)
            .and_then(|count| count.parse().ok())
            .unwrap_or(0);
        self.push.registered_at = store
            .get(&org, keys::REGISTERED_AT)
            .and_then(|at| at.parse().ok());
        self.push.endpoint_id = store.get(&org, keys::ENDPOINT);
    }

    /// Keeps the notification levels, blocked list, badge and mute under the organisation.
    ///
    /// # Errors
    ///
    /// When the store refuses.
    pub fn save(&self, store: &OrgStore) -> Result<(), secrets::Error> {
        let org = self.org();
        let notify = serde_json::to_string(self.chat().notify_settings()).unwrap_or_default();
        store.set(&org, keys::NOTIFY, &notify)?;
        let blocked = serde_json::to_string(&self.blocked).unwrap_or_default();
        store.set(&org, keys::BLOCKED, &blocked)?;
        store.set(&org, keys::BADGE, &self.badge.to_string())?;
        store.set(&org, keys::MUTED, if self.muted { "1" } else { "0" })?;
        store.set(&org, keys::PUSHED, &self.pushed.to_string())?;
        if let Some(id) = &self.push.endpoint_id {
            store.set(&org, keys::ENDPOINT, id)?;
        }
        if let Some(at) = self.push.registered_at {
            store.set(&org, keys::REGISTERED_AT, &at.to_string())?;
        }
        store.set(&org, keys::PUSH_NICK, &self.signin.nick)
    }

    /// What's unread here, as its badge counts it ([`Chat::org_badge`]): from the chat while
    /// connected, else the pushes not yet read. Nothing once the person is no longer in it.
    /// Muted, it still counts, its tile drawing it in grey: muting silences its notifications
    /// alone, and leaves it off the app's icon ([`Account::icon_badge`]), as decided ("let's
    /// show grey count, just mute notifications").
    #[must_use]
    pub fn unread(&self) -> Unread {
        if self.state == OrgState::AccessEnded {
            return Unread::default();
        }
        if self.state == OrgState::Connected {
            return self.chat().org_badge();
        }
        let pushed = self.pushed as usize;
        Unread {
            unread: pushed,
            mentions: pushed,
        }
    }

    /// A push for it arrived (`read` false) or a MARKREAD push did (`read` true): one more,
    /// or one less, never below none.
    pub fn pushed_one(&mut self, read: bool) {
        self.pushed = if read {
            self.pushed.saturating_sub(1)
        } else {
            self.pushed.saturating_add(1)
        };
    }

    /// Opening it counted its unread from the server: the stored count follows.
    pub fn recounted(&mut self) {
        let badge = self.chat().org_badge().mentions;
        self.pushed = u32::try_from(badge).unwrap_or(u32::MAX);
    }

    /// The server took the push registration at `now` (unix seconds).
    pub fn registered(&mut self, now: i64) {
        self.push.registered = true;
        self.push.registered_at = Some(now);
    }
}

/// One organisation as a switcher or rail shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgSummary {
    /// The organisation's name, as every client names it.
    pub org: String,
    /// The server's host.
    pub host: String,
    /// The person's handle there.
    pub handle: String,
    /// Where its connection stands.
    pub state: OrgState,
    /// What's unread there (nothing while muted).
    pub unread: Unread,
    /// Whether its server isn't one of zirc.dev's.
    pub self_hosted: bool,
    /// Whether it's the one shown.
    pub selected: bool,
    /// Its own name, as its chat's header has it (zirc.dev's, else the server's network
    /// name), once known: what [`OrgSummary::name`] says before the one made from `org`.
    pub title: Option<String>,
}

/// What's unread in each organisation and in total.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Badges {
    /// Per organisation, in the order signed in to.
    pub orgs: Vec<(String, Unread)>,
    /// The sum, without muted organisations: the app icon's.
    pub total: Unread,
}

/// A notification and the organisation it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification<N> {
    /// The organisation.
    pub org: String,
    /// What it says.
    pub notification: N,
}

/// A notification's title with its organisation first, so lock-screen groups sort by it and
/// two `#general` never merge: `Zirc · #api`. With one organisation there is nothing to tell
/// apart and the title is the conversation alone.
#[must_use]
pub fn notification_title(org: &str, conversation: &str, several: bool) -> String {
    if several {
        format!("{org} · {conversation}")
    } else {
        conversation.to_owned()
    }
}

/// What to do with the rollout and the limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The rollout switch: whether a second organisation can be added. Off by default.
    pub several: bool,
    /// The most organisations ([`MAX_ORGANISATIONS`]).
    pub max: usize,
    /// Which organisations are connected.
    pub connections: Connections,
    /// Whether the app uses zirc.dev's account-level device credential
    /// ([`crate::account_keys`]): off, as it is on zirc.dev (`ZIRC_ACCOUNT_KEYS`), and
    /// nothing of it is sent.
    pub account_keys: bool,
}

/// Which organisations an app keeps connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connections {
    /// Every one, while in the foreground: desktop and web.
    All,
    /// Only the organisation shown, as on a phone (decided 7 Nov 2026): switching closes one
    /// and opens the next, catching up from history. The others get their unread from push,
    /// zirc.dev registering the phone with them as it registers with the one shown.
    Selected,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            several: false,
            max: MAX_ORGANISATIONS,
            connections: Connections::All,
            account_keys: false,
        }
    }
}

impl Policy {
    /// A phone's: one connection, to the organisation shown.
    #[must_use]
    pub fn phone() -> Self {
        Self {
            connections: Connections::Selected,
            ..Self::default()
        }
    }

    /// With the account-level device credential on or off.
    #[must_use]
    pub fn with_account_keys(self, account_keys: bool) -> Self {
        Self {
            account_keys,
            ..self
        }
    }

    /// With several organisations on or off.
    #[must_use]
    pub fn with_several(self, several: bool) -> Self {
        Self { several, ..self }
    }
}

/// Why an organisation couldn't be added.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AddError {
    /// Several organisations are switched off, and there is one already.
    #[error("only one organisation can be signed in to")]
    NotEnabled,
    /// The most are signed in to.
    #[error("at most {0} organisations")]
    TooMany(usize),
}

/// What signing out of one organisation did locally, and what is left for the app to do over
/// the network, with the sign-in's own `web`: those calls need zirc.dev, and may fail without
/// the organisation staying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// The sign-in that was removed (its passphrase is already forgotten).
    pub signin: SignIn,
    /// Its push endpoint, to unregister at its own server.
    pub push: PushEndpoint,
    /// Whether the server holds a registration of the device key to remove: the key itself
    /// stays for the others, and goes only with the last organisation.
    pub unregister_device_key: bool,
    /// Whether this was the last organisation, so the app returns to the welcome screen.
    pub last: bool,
}

impl Removal {
    /// Whether the app also ends the zirc.dev session: the last organisation's removal does
    /// ([`LAST_SIGN_OUT_ENDS_ZIRC_DEV`]), unless the organisation is self-hosted and has none.
    #[must_use]
    pub fn ends_zirc_dev(&self) -> bool {
        LAST_SIGN_OUT_ENDS_ZIRC_DEV && self.last && !self.self_hosted()
    }

    fn self_hosted(&self) -> bool {
        self.signin.org() == self.signin.server.host
    }
}

/// Signing out of everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedOut {
    /// Each organisation's removal.
    pub removed: Vec<Removal>,
    /// The phone-wide push keys go, as does the device key ([`Account::remove_organisation`]
    /// deletes it with the last organisation).
    pub delete_push_keys: bool,
    /// The device key goes.
    pub delete_device_key: bool,
}

impl SignedOut {
    /// Whether the app also ends the zirc.dev session ([`LAST_SIGN_OUT_ENDS_ZIRC_DEV`]): when
    /// any organisation on zirc.dev was removed.
    #[must_use]
    pub fn ends_zirc_dev(&self) -> bool {
        LAST_SIGN_OUT_ENDS_ZIRC_DEV && self.removed.iter().any(|r| !r.self_hosted())
    }
}

/// One session per organisation, one selected.
#[derive(Debug)]
pub struct Account<T = Chat> {
    orgs: Orgs<OrgSession<T>>,
    store: OrgStore,
    policy: Policy,
    /// The account's organisations zirc.dev listed last that aren't on the device, offered as
    /// "Join …" ([`Account::joinable`], in `account_keys`).
    pub(crate) joinable: Vec<crate::account_keys::Joinable>,
}

impl<T: AsRef<Chat> + AsMut<Chat>> Account<T> {
    /// An account on `secrets` with `sessions`, in the order signed in to: their data loads
    /// from the per-organisation store, after the unkeyed data of the old single-organisation
    /// shape moved under the first ([`OrgStore::adopt_legacy`]). Sessions beyond
    /// `policy.max` are left out.
    ///
    /// # Errors
    ///
    /// When the secret store refuses the move.
    pub fn open(
        secrets: Arc<dyn Secrets>,
        policy: Policy,
        sessions: impl IntoIterator<Item = OrgSession<T>>,
    ) -> Result<Self, secrets::Error> {
        let mut sessions: Vec<_> = sessions.into_iter().take(policy.max).collect();
        let store = OrgStore {
            secrets,
            mirror: !policy.several && sessions.len() <= 1,
        };
        if let Some(first) = sessions.first() {
            store.adopt_legacy(&first.org())?;
        }
        let mut orgs = Orgs::new();
        for session in &mut sessions {
            session.load(&store);
        }
        for session in sessions {
            orgs.insert(session.org(), session);
        }
        Ok(Self {
            orgs,
            store,
            policy,
            joinable: Vec::new(),
        })
    }

    /// The per-organisation store.
    #[must_use]
    pub fn store(&self) -> &OrgStore {
        &self.store
    }

    /// The rollout and limits.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Switches several organisations on or off: off, no second organisation can be added.
    pub fn set_several(&mut self, several: bool) {
        self.policy.several = several;
        self.store.mirror = !several && self.orgs.len() <= 1;
    }

    /// Switches the account-level device credential on or off ([`Policy::account_keys`]): off,
    /// nothing asks zirc.dev for the account's organisations or credentials.
    pub fn set_account_keys(&mut self, on: bool) {
        self.policy.account_keys = on;
    }

    /// Whether "Add organisation" is offered.
    #[must_use]
    pub fn can_add(&self) -> bool {
        self.policy.several && self.orgs.len() < self.policy.max
    }

    /// How many organisations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.orgs.len()
    }

    /// Whether there are none: the welcome screen.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orgs.is_empty()
    }

    /// The organisation's session.
    #[must_use]
    pub fn get(&self, org: &str) -> Option<&OrgSession<T>> {
        self.orgs.get(org)
    }

    /// The organisation's session, to change.
    pub fn get_mut(&mut self, org: &str) -> Option<&mut OrgSession<T>> {
        self.orgs.get_mut(org)
    }

    /// The organisation shown, and its session.
    #[must_use]
    pub fn selected(&self) -> Option<(&str, &OrgSession<T>)> {
        self.orgs.selected()
    }

    /// The organisation shown, to change.
    pub fn selected_mut(&mut self) -> Option<(&str, &mut OrgSession<T>)> {
        self.orgs.selected_mut()
    }

    /// Shows `org`; false when it isn't signed in to.
    pub fn select(&mut self, org: &str) -> bool {
        self.orgs.select(org)
    }

    /// Every session, in the order signed in to.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &OrgSession<T>)> {
        self.orgs.iter()
    }

    /// Every session, to change.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut OrgSession<T>)> {
        self.orgs.iter_mut()
    }

    /// The sign-ins, in order, as the app's sign-in file keeps them.
    #[must_use]
    pub fn signins(&self) -> SignIns {
        let mut signins = SignIns::default();
        for (_, session) in self.orgs.iter() {
            signins.insert(session.signin.clone());
        }
        signins
    }

    /// The servers' hosts signed in to, for `LoginRequest.connected` when adding an
    /// organisation: zirc.dev marks them "Connected on this device".
    #[must_use]
    pub fn connected_hosts(&self) -> Vec<String> {
        self.orgs
            .iter()
            .map(|(_, s)| s.signin.server.host.clone())
            .collect()
    }

    /// What a switcher or rail lists.
    #[must_use]
    pub fn organisations(&self) -> Vec<OrgSummary> {
        let selected = self.orgs.selected().map(|(org, _)| org);
        self.orgs
            .iter()
            .map(|(org, session)| OrgSummary {
                org: org.to_owned(),
                host: session.signin.server.host.clone(),
                handle: session.signin.handle.clone(),
                state: session.state,
                unread: session.unread(),
                self_hosted: session.self_hosted(),
                selected: Some(org) == selected,
                title: Some(session.chat().org_header().name).filter(|name| !name.is_empty()),
            })
            .collect()
    }

    /// What's unread in each organisation and in total (muted ones count for nothing).
    #[must_use]
    pub fn badges(&self) -> Badges {
        self.badges_showing(None)
    }

    /// The count on the app's icon, as every app with one counts it: each organisation's
    /// badge ([`OrgSession::unread`]), the selected one's from `shown`, the chat the app shows,
    /// when its own entry here is parked and has none.
    #[must_use]
    pub fn icon_badge(&self, shown: Option<&Chat>) -> usize {
        // A muted organisation's count stays on its tile, not on the icon.
        self.badges_showing(shown)
            .orgs
            .iter()
            .filter(|(org, _)| self.get(org).is_none_or(|session| !session.muted))
            .map(|(_, unread)| unread.mentions)
            .sum()
    }

    /// [`Self::badges`], the selected organisation's counted from `shown` when given.
    fn badges_showing(&self, shown: Option<&Chat>) -> Badges {
        let selected = self.selected().map(|(org, _)| org.to_owned());
        let orgs: Vec<_> = self
            .orgs
            .iter()
            .map(|(org, session)| {
                let here = shown.filter(|_| Some(org) == selected.as_deref());
                let unread = match here {
                    Some(chat) => chat.org_badge_as(session.state),
                    None => session.unread(),
                };
                (org.to_owned(), unread)
            })
            .collect();
        let total = orgs
            .iter()
            .map(|(_, unread)| *unread)
            .fold(Unread::default(), std::ops::Add::add);
        Badges { orgs, total }
    }

    /// Adds the organisation `session` is for, never replacing another; the same organisation
    /// again (signing in once more, after a rename) replaces that one's session, its data
    /// kept. The first added is selected. Its stored data loads.
    ///
    /// # Errors
    ///
    /// When several organisations are off and there is one already, or there are
    /// [`Policy::max`].
    pub fn add_organisation(&mut self, mut session: OrgSession<T>) -> Result<(), AddError> {
        let org = session.org();
        if self.orgs.get(&org).is_none() {
            if !self.policy.several && !self.orgs.is_empty() {
                return Err(AddError::NotEnabled);
            }
            if self.orgs.len() >= self.policy.max {
                return Err(AddError::TooMany(self.policy.max));
            }
        }
        if self.orgs.is_empty() {
            let _ = self.store.adopt_legacy(&org);
        }
        session.load(&self.store);
        self.orgs.insert(org, session);
        self.store.mirror = !self.policy.several && self.orgs.len() <= 1;
        Ok(())
    }

    /// Adds every sign-in of `signins` that `make` gives a session for, as a handshake that
    /// returned several organisations: each merges, none replaces another. Answers the ones
    /// that couldn't be added and why.
    pub fn add_signins(
        &mut self,
        signins: &SignIns,
        mut make: impl FnMut(&SignIn) -> OrgSession<T>,
    ) -> Vec<(String, AddError)> {
        signins
            .iter()
            .filter_map(|signin| {
                self.add_organisation(make(signin))
                    .err()
                    .map(|err| (signin.org(), err))
            })
            .collect()
    }

    /// Signs out of `org` alone: its passphrase is forgotten, its levels, blocks, badge, push
    /// nick and sealing keys deleted, its session (and so its connection) dropped; the others
    /// carry on. The last one's going takes the device's key pair too.
    /// What is left to do over the network is in the answer. `None` when not signed in to it.
    ///
    /// # Errors
    ///
    /// When the secret store refuses; then it stays signed in.
    pub fn remove_organisation(&mut self, org: &str) -> Result<Option<Removal>, signin::Error> {
        let Some(session) = self.orgs.get(org) else {
            return Ok(None);
        };
        session.signin.forget(self.store.secrets.as_ref())?;
        self.store.forget(org)?;
        let last = self.orgs.len() == 1;
        crate::sealed::forget_at_sign_out(self.store.secrets.as_ref(), org, last)?;
        let Some(session) = self.orgs.remove(org) else {
            return Ok(None);
        };
        if self.orgs.is_empty() {
            // Nothing of zirc.dev is left to sign in to.
            self.store
                .secrets
                .delete(&crate::account_keys::account_id_key(&session.signin.web))?;
        }
        self.store.mirror = !self.policy.several && self.orgs.len() <= 1;
        Ok(Some(Removal {
            unregister_device_key: session.signin.account && session.signin.device_key,
            push: session.push,
            last: self.orgs.is_empty(),
            signin: session.signin,
        }))
    }

    /// Signs out of every organisation, as [`Self::remove_organisation`] does for each, and
    /// says the phone-wide push keys and the device key go too.
    ///
    /// # Errors
    ///
    /// When the secret store refuses; the organisations signed out before it are gone.
    pub fn sign_out_everything(&mut self) -> Result<SignedOut, signin::Error> {
        let names: Vec<String> = self.orgs.iter().map(|(org, _)| org.to_owned()).collect();
        let mut removed = Vec::new();
        for org in names {
            removed.extend(self.remove_organisation(&org)?);
        }
        Ok(SignedOut {
            removed,
            delete_push_keys: true,
            delete_device_key: true,
        })
    }

    /// Which organisations should be connected now, in the order to connect them: the
    /// selected first, then for [`Connections::All`] the rest as signed in to (not those whose
    /// access ended). In the background a phone has none.
    #[must_use]
    pub fn live(&self, foreground: bool) -> Vec<String> {
        if !foreground {
            return Vec::new();
        }
        let selected = self.orgs.selected().map(|(org, _)| org.to_owned());
        let mut live: Vec<String> = selected.iter().cloned().collect();
        if self.policy.connections == Connections::All {
            live.extend(
                self.orgs
                    .iter()
                    .filter(|(org, session)| {
                        Some(*org) != selected.as_deref() && session.state != OrgState::AccessEnded
                    })
                    .map(|(org, _)| org.to_owned()),
            );
        }
        live
    }

    /// The organisation whose push endpoint has the id `ep`, as a push carries it.
    #[must_use]
    pub fn org_for_endpoint(&self, ep: &str) -> Option<&str> {
        self.orgs
            .iter()
            .find(|(_, s)| s.push.endpoint_id.as_deref() == Some(ep))
            .map(|(org, _)| org)
    }

    /// Tags a notification with the organisation it is for.
    pub fn tag<N>(&self, org: &str, notification: N) -> Notification<N> {
        Notification {
            org: org.to_owned(),
            notification,
        }
    }

    /// A notification's title, the organisation first once there are several.
    #[must_use]
    pub fn title(&self, org: &str, conversation: &str) -> String {
        notification_title(org, conversation, self.orgs.len() > 1)
    }
}

/// The error a failed call reports, for apps that show it per organisation.
pub type CallError = login::Error;

#[cfg(test)]
mod tests;
