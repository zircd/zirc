//! The client half of zirc.dev's account-level device credential (`ZIRC_ACCOUNT_KEYS`, off
//! there by default): the device's own key ([`crate::device_key`]) signs a short token for
//! zirc.dev itself, and with it the app, without a browser,
//!
//! - lists the organisations the account has ([`Client::my_organisations`]), to end access to
//!   removed ones and offer the others not on the device as "Join …"
//!   ([`Account::refresh_organisations`], [`Account::joinable`]): an app holds only the
//!   organisations the person ticked when signing in, and joins another only when asked
//!   ([`Account::join_organisation`]), as the user chose on 20 Nov 2026;
//! - gets fresh credentials for one ([`Client::my_credentials`]), to join it, or after a
//!   handle rename or a refused sign-in, so "Sign in again" is only the fallback
//!   ([`Account::renew_organisation`]);
//! - signs out of one organisation, or of zirc.dev ([`Client::sign_out_of`],
//!   [`Client::sign_out_device`]), which the `_online` forms of removing do.
//!
//! The wire is `Authorization: Device <ES256 JWT>`: `sub` the account id
//! ([`zirc_proto::login::Credentials::account_id`], kept in the secret store by
//! [`SignIn::from_credentials`]), `aud` zirc.dev's host name, at most 60 seconds, a fresh `jti`
//! per call. See `docs/several-organisations-api.md`.
//!
//! Everything here is behind [`Policy::account_keys`], off in the core by default as it is on
//! zirc.dev: with it off, nothing is sent. Removing one organisation's registration of the
//! device key works without the switch, with that organisation's own passphrase
//! ([`Client::remove_key_registration`]).

use std::sync::Arc;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use zirc_proto::login::{Account as Credential, Credentials, OrgCredentials};

use zirc_chat::chat::Chat;

use crate::account::{
    Account, AddError, LAST_SIGN_OUT_ENDS_ZIRC_DEV, OrgSession, OrgState, Removal, SignedOut,
};
use crate::device_key::{self, DeviceKey};
use crate::login::{self, Client};
use crate::secrets::Secrets;
use crate::signin::{self, SignIn};

/// How long an account-level token lasts: zirc.dev takes at most this.
pub const TOKEN_LIFETIME: u64 = 60;

/// Where the account id of the zirc.dev at `web` is kept in the secret store.
#[must_use]
pub fn account_id_key(web: &str) -> String {
    format!("zirc-account-id@{}", host_of(web))
}

/// The host name of a web address, as the token's `aud`.
#[must_use]
pub fn host_of(web: &str) -> String {
    reqwest::Url::parse(web)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| web.to_owned())
}

/// One organisation of the account, as zirc.dev lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// Its slug.
    pub slug: String,
    /// Its name.
    pub name: String,
    /// Its server's host.
    pub host: String,
    /// The person's handle there.
    pub handle: String,
    /// Their role.
    pub role: String,
    /// Whether its server is ready: a new organisation takes a short while.
    pub ready: bool,
}

/// An invitation waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    /// The organisation's slug.
    pub slug: String,
    /// Its name.
    pub name: String,
}

/// What `GET /api/me/organisations` answers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Organisations {
    /// The account's memberships.
    pub organisations: Vec<Listed>,
    /// Invitations waiting.
    #[serde(default)]
    pub invites: Vec<Invite>,
}

/// Why zirc.dev refused a token (or the call).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// It doesn't know the key: sign in again.
    DeviceUnknown,
    /// The device was revoked, as from the account page: sign in again.
    DeviceRevoked,
    /// The token wasn't a valid one.
    TokenInvalid,
    /// It had expired by the time it arrived: a fresh one may work.
    TokenExpired,
    /// Its id was used before: a fresh one may work.
    TokenReplayed,
    /// It was made for another host.
    WrongAudience,
    /// A code this client doesn't know.
    Other,
}

impl Code {
    fn of(code: &str) -> Self {
        match code {
            "device_unknown" => Self::DeviceUnknown,
            "device_revoked" => Self::DeviceRevoked,
            "token_invalid" => Self::TokenInvalid,
            "token_expired" => Self::TokenExpired,
            "token_replayed" => Self::TokenReplayed,
            "wrong_audience" => Self::WrongAudience,
            _ => Self::Other,
        }
    }

    /// Whether a fresh token may be accepted where this one wasn't.
    #[must_use]
    pub fn retryable(self) -> bool {
        matches!(self, Self::TokenExpired | Self::TokenReplayed)
    }
}

/// Why a call with the device credential didn't work.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// zirc.dev refused the token. Except for the [retryable](Code::retryable) ones, which
    /// are retried once with a fresh token first, the person has to sign in again.
    #[error("zirc.dev refused the device ({code:?}): sign in again")]
    Refused {
        /// Why.
        code: Code,
    },
    /// Too many calls; try again after this many seconds.
    #[error("zirc.dev asks for a pause of {retry_after} s")]
    RateLimited {
        /// Seconds to wait.
        retry_after: u64,
    },
    /// zirc.dev doesn't take the device credential (the switch is off there).
    #[error("zirc.dev doesn't take the account credential")]
    Disabled,
    /// The call didn't go through: the account id or the key is missing here.
    #[error("no account credential on this device")]
    NoCredential,
    /// The organisation asked to join isn't one zirc.dev listed for the account, or its
    /// server isn't ready yet.
    #[error("{0} can't be joined from this device now")]
    NotJoinable(String),
    /// The organisation couldn't be added: several are off, or the most are signed in to.
    #[error(transparent)]
    Add(#[from] AddError),
    /// Anything else.
    #[error(transparent)]
    Login(#[from] login::Error),
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::Login(err.into())
    }
}

impl Error {
    /// Whether the fallback is the browser sign-in: the device isn't one zirc.dev takes.
    #[must_use]
    pub fn needs_sign_in(&self) -> bool {
        matches!(self, Self::Refused { .. } | Self::NoCredential)
    }
}

/// The device's account credential: its key and the account's id.
#[derive(Clone)]
pub struct Device {
    key: Arc<dyn DeviceKey>,
    account_id: String,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

impl Device {
    /// The credential for the zirc.dev at `web`: `key` with the account id the sign-in kept in
    /// `secrets`. `None` when this device signed in before there was one.
    #[must_use]
    pub fn new(key: Arc<dyn DeviceKey>, secrets: &dyn Secrets, web: &str) -> Option<Self> {
        Some(Self {
            key,
            account_id: secrets.get(&account_id_key(web))?,
        })
    }

    /// A fresh token for the zirc.dev at `web`.
    fn token(&self, web: &str) -> Option<String> {
        device_key::token_lasting(&*self.key, &self.account_id, &host_of(web), TOKEN_LIFETIME)
    }
}

#[derive(Serialize)]
struct Org<'a> {
    org: &'a str,
}

#[derive(Serialize)]
struct OrgKey<'a> {
    org: &'a str,
    key: &'a str,
}

impl Client {
    /// Sends the call `method path` with a body, `Authorization: Device <token>`, a fresh
    /// token each try and one more try when it was expired or replayed.
    async fn device_call(
        &self,
        device: &Device,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, Error> {
        let mut tries = 0;
        loop {
            tries += 1;
            let token = device.token(&self.web).ok_or(Error::NoCredential)?;
            let mut request = self
                .http
                .request(method.clone(), format!("{}{path}", self.web))
                .header("Authorization", format!("Device {token}"));
            if let Some(body) = &body {
                request = request.json(body);
            }
            match self.account_answer(request.send().await?).await {
                Err(Error::Refused { code }) if code.retryable() && tries < 2 => {}
                other => return other,
            }
        }
    }

    /// The response if it succeeded, else why not.
    async fn account_answer(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, Error> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .unwrap_or(60);
        let text = response.text().await.unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let code = json.get("code").and_then(|code| code.as_str());
        match (status, code) {
            (StatusCode::UNAUTHORIZED, code) => Err(Error::Refused {
                code: Code::of(code.unwrap_or_default()),
            }),
            (StatusCode::TOO_MANY_REQUESTS, _) => Err(Error::RateLimited { retry_after }),
            // 404 for every route while the switch is off; `no_such_organisation` is a 404
            // that says what it is.
            (StatusCode::NOT_FOUND, None) => Err(Error::Disabled),
            _ => Err(Error::Login(login::Error::Status {
                web: self.web.clone(),
                status,
                message: json
                    .get("error")
                    .and_then(|message| message.as_str())
                    .map_or_else(|| text.chars().take(200).collect(), str::to_owned),
            })),
        }
    }

    /// The account's organisations and invitations: `GET /api/me/organisations`.
    ///
    /// # Errors
    ///
    /// As [`Error`].
    pub async fn my_organisations(&self, device: &Device) -> Result<Organisations, Error> {
        let response = self
            .device_call(device, reqwest::Method::GET, "/api/me/organisations", None)
            .await?;
        Ok(response.json().await.map_err(login::Error::from)?)
    }

    /// Fresh credentials for the organisation `org` (its slug), under the handle now:
    /// `POST /api/me/credentials`.
    ///
    /// # Errors
    ///
    /// As [`Error`]; a 409 (its server being set up) is a [`login::Error::Status`].
    pub async fn my_credentials(
        &self,
        device: &Device,
        org: &str,
    ) -> Result<OrgCredentials, Error> {
        let body = serde_json::to_value(Org { org }).unwrap_or_default();
        let response = self
            .device_call(
                device,
                reqwest::Method::POST,
                "/api/me/credentials",
                Some(body),
            )
            .await?;
        Ok(response.json().await.map_err(login::Error::from)?)
    }

    /// Signs the device out of the organisation `org`: its key leaves that server; the device
    /// stays signed in to zirc.dev. `DELETE /api/me/credentials`.
    ///
    /// # Errors
    ///
    /// As [`Error`].
    pub async fn sign_out_of(&self, device: &Device, org: &str) -> Result<(), Error> {
        let body = serde_json::to_value(Org { org }).unwrap_or_default();
        self.device_call(
            device,
            reqwest::Method::DELETE,
            "/api/me/credentials",
            Some(body),
        )
        .await
        .map(drop)
    }

    /// Signs the device out of zirc.dev: it is revoked and its key leaves every server.
    /// `DELETE /api/me/devices/current`.
    ///
    /// # Errors
    ///
    /// As [`Error`].
    pub async fn sign_out_device(&self, device: &Device) -> Result<(), Error> {
        self.device_call(
            device,
            reqwest::Method::DELETE,
            "/api/me/devices/current",
            None,
        )
        .await
        .map(drop)
    }

    /// Removes this device's key (`key`, its public half as text) from the server of the
    /// organisation `org`, as the organisation's own `credential`: for an app without the
    /// account credential, and it works while the switch is off.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses.
    pub async fn remove_key_registration(
        &self,
        credential: &Credential,
        org: &str,
        key: &str,
    ) -> Result<(), Error> {
        let body = serde_json::to_value(OrgKey { org, key }).unwrap_or_default();
        let response = self
            .http
            .delete(format!("{}/api/me/credentials", self.web))
            .basic_auth(&credential.name, Some(&credential.password))
            .json(&body)
            .send()
            .await?;
        self.account_answer(response).await.map(drop)
    }
}

/// One organisation of the account that isn't on the device: the app offers it as "Join …"
/// ([`Account::joinable`]) and adds it only when the person asks
/// ([`Account::join_organisation`]), never by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joinable {
    /// Its slug, which zirc.dev's credentials are asked for by.
    pub slug: String,
    /// Its name, as zirc.dev has it (may be empty).
    pub name: String,
    /// Its server's host.
    pub host: String,
    /// The person's handle there.
    pub handle: String,
    /// Whether its server is ready: a new organisation takes a short while.
    pub ready: bool,
    /// The zirc.dev that listed it, which its credentials come from.
    pub web: String,
}

impl Joinable {
    /// The organisation's name as every client names it once joined (by its server's host),
    /// so its tile's colour is the same before and after.
    #[must_use]
    pub fn org(&self) -> String {
        crate::settings::org_name(&zirc_proto::login::Server {
            host: self.host.clone(),
            port: None,
            tls: None,
            password: None,
        })
    }
}

/// What a refresh found.
#[derive(Debug, Default)]
pub struct Refreshed {
    /// Organisations whose membership is gone: marked access ended, nothing else touched.
    pub ended: Vec<String>,
    /// The account's organisations not on the device, to offer as "Join …"
    /// ([`Account::joinable`]).
    pub joinable: Vec<Joinable>,
    /// Invitations waiting. Not offered yet: the client has no call to accept one, so an
    /// invitation is accepted on zirc.dev, after which it is listed and joinable.
    pub invites: Vec<Invite>,
    /// Whether anything was asked: false with the switch off, and when zirc.dev has it off.
    pub asked: bool,
}

/// What the network half of signing out did.
#[derive(Debug, Default)]
pub struct Revoked {
    /// What couldn't be done, per organisation (`zirc.dev` for the device), which doesn't stop
    /// the sign-out here: the person is signed out locally regardless.
    pub failed: Vec<(String, String)>,
}

/// What a sign-out tells zirc.dev, one rule for every app ([`Account::leaving`]): each
/// organisation it leaves has this device's key revoked there; signing out of the last, or of
/// everything, also ends zirc.dev on the device, the browser session that let the app in
/// included. Taken while the account is at hand, so the calls run without it
/// ([`Leaving::tell`]), as the phones make them outside their lock.
#[derive(Debug, Clone)]
pub struct Leaving {
    /// Each organisation left, with the device credential when account keys are on.
    orgs: Vec<(SignIn, Option<Device>)>,
    /// zirc.dev's address and the device's credential there (none for a self-hosted server),
    /// when this ends zirc.dev.
    zirc_dev: Option<(String, Option<Device>)>,
}

impl Leaving {
    /// What signing out of `signins` tells zirc.dev, `last` when no organisation is left after
    /// it, each with its device credential from `device` (by zirc.dev's address; none without
    /// account keys). [`Account::leaving`] makes it from the account; an app that keeps only
    /// its sign-ins, as the terminal client does, from those.
    #[must_use]
    pub fn of(signins: Vec<SignIn>, last: bool, device: impl Fn(&str) -> Option<Device>) -> Self {
        // zirc.dev's own sign-in when there is one; a self-hosted server's has no device there,
        // but a browser session that let the app in is zirc.dev's all the same.
        let hosted = signins.iter().find(|s| s.org() != s.server.host);
        let zirc_dev = hosted
            .or(signins.first())
            .filter(|_| LAST_SIGN_OUT_ENDS_ZIRC_DEV && last)
            .map(|s| (s.web.clone(), hosted.and_then(|s| device(&s.web))));
        Self {
            orgs: signins
                .into_iter()
                .map(|signin| {
                    let credential = device(&signin.web);
                    (signin, credential)
                })
                .collect(),
            zirc_dev,
        }
    }

    /// Whether it ends zirc.dev on the device: the last organisation, or everything.
    #[must_use]
    pub fn ends_zirc_dev(&self) -> bool {
        self.zirc_dev.is_some()
    }

    /// Tells zirc.dev: each organisation's key revoked ([`leave`]); then, when it ends
    /// zirc.dev, the device signed out there (with account keys on) and `web_session`, the
    /// account and the browser session's hash that let the app in, ended. What failed is in
    /// [`Revoked::failed`] (`zirc.dev` for the last two); none of it keeps anyone signed in.
    pub async fn tell(
        &self,
        key: &Arc<dyn DeviceKey>,
        secrets: &dyn Secrets,
        web_session: Option<(&Credential, &str)>,
    ) -> Revoked {
        let mut revoked = Revoked::default();
        for (signin, device) in &self.orgs {
            if let Err(err) = leave(key, signin, secrets, device.as_ref()).await {
                revoked.failed.push((signin.org(), err.to_string()));
            }
        }
        let Some((web, device)) = &self.zirc_dev else {
            return revoked;
        };
        let client = match Client::new(web) {
            Ok(client) => client,
            Err(err) => {
                revoked.failed.push(("zirc.dev".into(), err.to_string()));
                return revoked;
            }
        };
        if let Some(device) = device
            && let Err(err) = client.sign_out_device(device).await
        {
            revoked.failed.push(("zirc.dev".into(), err.to_string()));
        }
        if let Some((account, session)) = web_session
            && let Err(err) = client.end_web_session(account, session).await
        {
            revoked.failed.push(("zirc.dev".into(), err.to_string()));
        }
        revoked
    }
}

/// The credential for `listed` as a sign-in would take it, from `credentials`.
fn credentials_of(org: &OrgCredentials, account_id: &str) -> Credentials {
    Credentials {
        server: org.server.clone(),
        account: org.account.clone(),
        nick: org.nick.clone(),
        owner: org.owner.clone(),
        realname: org.realname.clone(),
        account_id: Some(account_id.to_owned()),
        organisation_id: org.organisation_id.clone(),
        media: org.media.clone(),
        web_session: None,
        orgs: Vec::new(),
    }
}

impl<T: AsRef<Chat> + AsMut<Chat>> Account<T> {
    /// The device credential for the zirc.dev at `web`, when account keys are on and this
    /// device has the account id.
    pub fn device(&self, key: &Arc<dyn DeviceKey>, web: &str) -> Option<Device> {
        if !self.policy().account_keys {
            return None;
        }
        Device::new(key.clone(), self.store().secrets().as_ref(), web)
    }

    /// Whether the app uses the account-level device credential ([`Policy::account_keys`]).
    #[must_use]
    pub fn account_keys_on(&self) -> bool {
        self.policy().account_keys
    }

    /// The zirc.dev address `org` was signed in at, which a fetch for it asks.
    #[must_use]
    pub fn web_of(&self, org: &str) -> Option<String> {
        self.get(org).map(|s| s.signin().web.clone())
    }

    /// Asks zirc.dev at `web` which organisations the account has: ones on that zirc.dev
    /// that are gone are marked [access ended](OrgState::AccessEnded), and the others not on
    /// the device are kept to offer as "Join …" ([`Self::joinable`]), never added by
    /// themselves. The other organisations aren't touched, and no request is made with
    /// account keys off. This holds the account across the await; an app that mustn't
    /// ([`fetch_listing`] outside its lock, then [`Self::apply_listing`] under it) does the
    /// two halves itself.
    ///
    /// # Errors
    ///
    /// When the call is refused ([`Error::needs_sign_in`]) or can't be made.
    pub async fn refresh_organisations(
        &mut self,
        key: &Arc<dyn DeviceKey>,
        web: &str,
    ) -> Result<Refreshed, Error> {
        if !self.account_keys_on() {
            return Ok(Refreshed::default());
        }
        let secrets = self.store().secrets().clone();
        match fetch_listing(key, secrets.as_ref(), web).await? {
            Some(fetched) => Ok(self.apply_listing(&fetched)),
            None => Ok(Refreshed::default()),
        }
    }

    /// Takes in a [fetched](fetch_listing) listing: access ended for the organisations on
    /// that zirc.dev that are gone, and the ones not on the device kept to offer as
    /// "Join …" ([`Self::joinable`]). No waiting, so an app does it under its lock.
    pub fn apply_listing(&mut self, fetched: &Fetched) -> Refreshed {
        let web = fetched.web.as_str();
        let known = self.connected_hosts();
        let joinable: Vec<Joinable> = fetched
            .listing
            .organisations
            .iter()
            .filter(|listed| {
                !known
                    .iter()
                    .any(|host| host.eq_ignore_ascii_case(&listed.host))
            })
            .map(|listed| Joinable {
                slug: listed.slug.clone(),
                name: listed.name.clone(),
                host: listed.host.clone(),
                handle: listed.handle.clone(),
                ready: listed.ready,
                web: web.to_owned(),
            })
            .collect();
        // Another zirc.dev's are kept; this one's are as it lists them now.
        self.joinable.retain(|j| j.web != web);
        self.joinable.extend(joinable.iter().cloned());
        let mut refreshed = Refreshed {
            asked: true,
            invites: fetched.listing.invites.clone(),
            joinable,
            ..Refreshed::default()
        };
        let gone: Vec<String> = self
            .iter()
            .filter(|(_, s)| {
                s.signin().web == web
                    && !s.self_hosted()
                    && !fetched
                        .listing
                        .organisations
                        .iter()
                        .any(|l| l.host.eq_ignore_ascii_case(&s.signin().server.host))
            })
            .map(|(org, _)| org.to_owned())
            .collect();
        for org in gone {
            if let Some(session) = self.get_mut(&org) {
                session.end_access();
                // Its sealing keys go from the device as its access does; another
                // organisation's stay.
                if let Err(err) = self
                    .store()
                    .secrets()
                    .delete(&crate::sealed::Keys::name(&org))
                {
                    tracing::warn!("couldn't forget {org}'s sealing keys: {err}");
                }
                refreshed.ended.push(org);
            }
        }
        refreshed
    }

    /// The account's organisations zirc.dev listed last that aren't on the device, in its
    /// order, to offer as "Join …" beside "Add organisation": none while that isn't offered
    /// ([`Self::can_add`]), as joining one adds it.
    #[must_use]
    pub fn joinable(&self) -> Vec<Joinable> {
        if !self.can_add() {
            return Vec::new();
        }
        let known = self.connected_hosts();
        self.joinable
            .iter()
            .filter(|j| !known.iter().any(|host| host.eq_ignore_ascii_case(&j.host)))
            .cloned()
            .collect()
    }

    /// The joinable organisation `slug` ([`Self::joinable`]), to join, when its server is
    /// ready.
    ///
    /// # Errors
    ///
    /// [`Error::NotJoinable`] when it isn't listed, or isn't ready yet.
    pub fn to_join(&self, slug: &str) -> Result<Joinable, Error> {
        self.joinable()
            .into_iter()
            .find(|j| j.slug == slug && j.ready)
            .ok_or_else(|| Error::NotJoinable(slug.to_owned()))
    }

    /// Joins the organisation `slug`, one the account has that isn't on the device
    /// ([`Self::joinable`]): fresh credentials from zirc.dev with the device's credential,
    /// then it is added, `make` giving it a session. Its name as every client names it. An
    /// app that mustn't hold the account across the await does [`Self::to_join`],
    /// [`fetch_credentials`] and [`Self::apply_joined`] itself.
    ///
    /// # Errors
    ///
    /// When it isn't joinable ([`Error::NotJoinable`]), zirc.dev refuses or can't be reached,
    /// or it can't be added ([`Error::Add`]).
    pub async fn join_organisation(
        &mut self,
        key: &Arc<dyn DeviceKey>,
        slug: &str,
        make: impl FnOnce(&SignIn) -> OrgSession<T>,
    ) -> Result<String, Error> {
        if !self.account_keys_on() {
            return Err(Error::NoCredential);
        }
        let joining = self.to_join(slug)?;
        let secrets = self.store().secrets().clone();
        match fetch_credentials(key, secrets.as_ref(), &joining.web, slug).await? {
            Some(fetched) => self.apply_joined(&fetched, make),
            None => Err(Error::NoCredential),
        }
    }

    /// Adds the organisation whose credentials were [fetched](fetch_credentials) to join it,
    /// `make` giving it a session: the one way an organisation zirc.dev lists comes onto the
    /// device without the browser. No waiting, so an app does it under its lock. Its name as
    /// every client names it; already here, nothing changes.
    ///
    /// # Errors
    ///
    /// When it can't be added ([`Error::Add`], its passphrase forgotten again) or its
    /// passphrase can't be kept.
    pub fn apply_joined(
        &mut self,
        fetched: &Renewed,
        make: impl FnOnce(&SignIn) -> OrgSession<T>,
    ) -> Result<String, Error> {
        let credentials = credentials_of(&fetched.credentials, &fetched.account_id);
        let secrets = self.store().secrets().clone();
        if let Some((org, _)) = self.iter().find(|(_, s)| {
            s.signin()
                .server
                .host
                .eq_ignore_ascii_case(&credentials.server.host)
        }) {
            return Ok(org.to_owned());
        }
        let signin = SignIn::from_credentials(&credentials, &fetched.web, secrets.as_ref())
            .map_err(|err| signin_error(&err))?;
        if let Err(err) = self.add_organisation(make(&signin)) {
            let _ = signin.forget(secrets.as_ref());
            return Err(err.into());
        }
        let host = &signin.server.host;
        self.joinable.retain(|j| !j.host.eq_ignore_ascii_case(host));
        Ok(signin.org())
    }

    /// Fresh credentials for the organisation `org` from zirc.dev, after the server refused
    /// its sign-in (a handle rename, say): its sign-in is replaced with the new handle, its
    /// passphrase moved, its data kept (it is keyed by the organisation). `Ok(false)` when
    /// account keys are off or this device has none, and the person signs in again in the
    /// browser; an error that [needs a sign-in](Error::needs_sign_in) means the same. An app
    /// that mustn't hold the account across the await does [`fetch_credentials`] and
    /// [`Self::apply_credentials`] itself.
    ///
    /// # Errors
    ///
    /// As [`Error`].
    pub async fn renew_organisation(
        &mut self,
        key: &Arc<dyn DeviceKey>,
        org: &str,
    ) -> Result<bool, Error> {
        let Some(web) = self.web_of(org) else {
            return Ok(false);
        };
        if !self.account_keys_on() {
            return Ok(false);
        }
        let secrets = self.store().secrets().clone();
        match fetch_credentials(key, secrets.as_ref(), &web, org).await? {
            Some(renewed) => self.apply_credentials(org, &renewed),
            None => Ok(false),
        }
    }

    /// Replaces `org`'s sign-in with [fetched](fetch_credentials) credentials: no waiting, so
    /// an app does it under its lock. `Ok(false)` when it isn't signed in to `org` any more.
    ///
    /// # Errors
    ///
    /// When the passphrase can't be kept.
    pub fn apply_credentials(&mut self, org: &str, renewed: &Renewed) -> Result<bool, Error> {
        let Some(session) = self.get(org) else {
            return Ok(false);
        };
        let credentials = credentials_of(&renewed.credentials, &renewed.account_id);
        let secrets = self.store().secrets().clone();
        let earlier = session.signin().clone();
        let signin = SignIn::from_credentials(&credentials, &renewed.web, secrets.as_ref())
            .map_err(|err| signin_error(&err))?;
        if signin.key() != earlier.key() {
            let _ = earlier.forget(secrets.as_ref());
        }
        if let Some(session) = self.get_mut(org) {
            session.replace_signin(signin);
        }
        Ok(true)
    }

    /// What signing out of `org`, or with `None` of everything, tells zirc.dev ([`Leaving`]),
    /// taken now, while the account is at hand: each organisation with its device credential,
    /// and, when it ends zirc.dev on the device ([`Removal::ends_zirc_dev`]: the last
    /// organisation, or everything), zirc.dev's address and credential, which the local
    /// sign-out forgets.
    #[must_use]
    pub fn leaving(&self, key: &Arc<dyn DeviceKey>, org: Option<&str>) -> Leaving {
        let signins: Vec<SignIn> = match org {
            Some(org) => self
                .get(org)
                .map(|s| s.signin().clone())
                .into_iter()
                .collect(),
            None => self.iter().map(|(_, s)| s.signin().clone()).collect(),
        };
        let last = !signins.is_empty() && (org.is_none() || self.len() == 1);
        Leaving::of(signins, last, |web| self.device(key, web))
    }

    /// Signs out of `org` as [`Account::remove_organisation`] does, after telling zirc.dev
    /// ([`Leaving::tell`]): its key revoked, and, for the last organisation, zirc.dev ended on
    /// the device, `web_session` included. A failed call is in [`Revoked::failed`] and doesn't
    /// keep the person signed in.
    ///
    /// # Errors
    ///
    /// When the secret store refuses the local sign-out.
    pub async fn remove_organisation_online(
        &mut self,
        key: &Arc<dyn DeviceKey>,
        org: &str,
        web_session: Option<(&Credential, &str)>,
    ) -> Result<Option<(Removal, Revoked)>, signin::Error> {
        let leaving = self.leaving(key, Some(org));
        let secrets = self.store().secrets().clone();
        let revoked = leaving.tell(key, secrets.as_ref(), web_session).await;
        Ok(self
            .remove_organisation(org)?
            .map(|removal| (removal, revoked)))
    }

    /// Signs out of everything as [`Account::sign_out_everything`] does, after telling zirc.dev
    /// as [`Self::remove_organisation_online`] does for each organisation, and ending zirc.dev
    /// on the device.
    ///
    /// # Errors
    ///
    /// When the secret store refuses the local sign-out.
    pub async fn sign_out_everything_online(
        &mut self,
        key: &Arc<dyn DeviceKey>,
        web_session: Option<(&Credential, &str)>,
    ) -> Result<(SignedOut, Revoked), signin::Error> {
        let leaving = self.leaving(key, None);
        let secrets = self.store().secrets().clone();
        let revoked = leaving.tell(key, secrets.as_ref(), web_session).await;
        Ok((self.sign_out_everything()?, revoked))
    }
}

/// Tells the zirc.dev of `signin` the device leaves its organisation: with the device
/// credential `device` when account keys are on ([`Account::device`]), else, where the server
/// holds this device's key, with the organisation's own passphrase from `secrets`. Nothing for
/// a sign-in without the device key. Apps that can't hold their account while zirc.dev answers
/// call it before removing the organisation ([`Account::remove_organisation`]).
///
/// # Errors
///
/// When zirc.dev can't be reached or refuses.
pub async fn leave(
    key: &Arc<dyn DeviceKey>,
    signin: &SignIn,
    secrets: &dyn Secrets,
    device: Option<&Device>,
) -> Result<(), Error> {
    if !(signin.account && signin.device_key) {
        return Ok(());
    }
    let org = signin.org();
    let client = Client::new(&signin.web)?;
    if let Some(device) = device {
        return client.sign_out_of(device, &org).await;
    }
    let call = crate::account::Call::of(signin, secrets).map_err(|err| {
        Error::Login(login::Error::Status {
            web: signin.web.clone(),
            status: StatusCode::UNAUTHORIZED,
            message: err.to_string(),
        })
    })?;
    let public = key.public_key().ok_or(Error::NoCredential)?;
    client
        .remove_key_registration(&call.credential, &org, &public)
        .await
}

/// What [`fetch_listing`] brought from zirc.dev, to apply with [`Account::apply_listing`].
#[derive(Debug, Clone)]
pub struct Fetched {
    web: String,
    listing: Organisations,
}

/// Fresh credentials for one organisation, from [`fetch_credentials`], to apply with
/// [`Account::apply_credentials`] after a rename, or [`Account::apply_joined`] to join it.
#[derive(Debug, Clone)]
pub struct Renewed {
    web: String,
    account_id: String,
    credentials: OrgCredentials,
}

/// Asks the zirc.dev at `web` which organisations the account has, without an [`Account`], so
/// an app can do it outside its lock: nothing more is asked, as the app joins one only when
/// the person asks ([`Account::join_organisation`]). `None` when this device has no account
/// credential, or zirc.dev has it off. Check [`Account::account_keys_on`] first: this doesn't.
///
/// # Errors
///
/// When the call is refused ([`Error::needs_sign_in`]) or can't be made.
pub async fn fetch_listing(
    key: &Arc<dyn DeviceKey>,
    secrets: &dyn Secrets,
    web: &str,
) -> Result<Option<Fetched>, Error> {
    let Some(device) = Device::new(key.clone(), secrets, web) else {
        return Ok(None);
    };
    let client = Client::new(web)?;
    let listing = match client.my_organisations(&device).await {
        Ok(listing) => listing,
        Err(Error::Disabled) => return Ok(None),
        Err(err) => return Err(err),
    };
    Ok(Some(Fetched {
        web: web.to_owned(),
        listing,
    }))
}

/// Asks the zirc.dev at `web` for fresh credentials for the organisation `org` (its slug),
/// without an [`Account`]. `None` when this device has no account credential. Check
/// [`Account::account_keys_on`] first: this doesn't.
///
/// # Errors
///
/// As [`Error`].
pub async fn fetch_credentials(
    key: &Arc<dyn DeviceKey>,
    secrets: &dyn Secrets,
    web: &str,
    org: &str,
) -> Result<Option<Renewed>, Error> {
    let Some(device) = Device::new(key.clone(), secrets, web) else {
        return Ok(None);
    };
    let credentials = Client::new(web)?.my_credentials(&device, org).await?;
    Ok(Some(Renewed {
        web: web.to_owned(),
        account_id: device.account_id,
        credentials,
    }))
}

fn signin_error(err: &signin::Error) -> Error {
    Error::Login(login::Error::Status {
        web: String::new(),
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: err.to_string(),
    })
}

impl<T: AsRef<Chat> + AsMut<Chat>> OrgSession<T> {
    /// Its access ended: the server no longer takes the sign-in, and its chat holds none of
    /// its sealing keys.
    pub fn end_access(&mut self) {
        self.set_state_now(OrgState::AccessEnded);
        let org = self.signin().org();
        self.chat_mut().set_keys(&org, Vec::new());
    }
}

#[cfg(test)]
mod tests;
