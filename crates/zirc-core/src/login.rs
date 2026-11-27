//! The client side of `zirc login`, for the CLI and the apps alike. See
//! [`zirc_proto::login`] for the handshake.

use std::time::Duration;

use reqwest::StatusCode;
use zirc_proto::login::{
    self, Account, ContentAccess, Credentials, LogPage, LoginRequest, LoginStarted, LoginStatus,
};

/// zirc's web app, where logins happen unless told otherwise.
pub const WEB: &str = "https://zirc.dev";

/// Why a login didn't work.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The web app couldn't be reached, or answered something that isn't the handshake.
    #[error("talking to the web app")]
    Http(#[from] reqwest::Error),
    /// The web app doesn't take logins: an older one, or one without accounts.
    #[error("{web} doesn't take logins yet: set the server up with `zirc init --server <host>`")]
    Unavailable {
        /// The web app's address.
        web: String,
    },
    /// The web app answered an error.
    #[error("{web} answered {status}: {message}")]
    Status {
        /// The web app's address.
        web: String,
        /// The HTTP status.
        status: StatusCode,
        /// What it said was wrong.
        message: String,
    },
    /// A file's link points somewhere other than the web app, so the account isn't sent
    /// there: anyone can share a file with a link of their choosing.
    #[error("{url} isn't on {web}")]
    Foreign {
        /// The web app's address.
        web: String,
        /// The link.
        url: String,
    },
    /// The web app doesn't serve what was asked for yet: an older one.
    #[error("{web} doesn't serve {what} yet")]
    NotYet {
        /// The web app's address.
        web: String,
        /// What was asked for, in words.
        what: &'static str,
    },
    /// The person turned the login down in the browser.
    #[error("the login was turned down in the browser")]
    Denied,
    /// Nobody confirmed the login in time.
    #[error("the login expired before it was confirmed: start it again")]
    Expired,
    /// The web app named an organisation by a name zirc can't keep its files under.
    #[error("{web} named an organisation {name:?}, which zirc can't keep")]
    OrgName {
        /// The web app's address.
        web: String,
        /// The name.
        name: String,
    },
}

/// One organisation a login signs in to, as [`each_org`] gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgLogin {
    /// What the web app named it, when it named it: `None` for a login that asked for one
    /// organisation, from a web app that names none.
    pub name: Option<String>,
    /// Its own credentials, as a login for it alone would give.
    pub credentials: Credentials,
}

/// Each organisation the `credentials` a login at `web` gave sign in to, the first first,
/// with credentials of its own: the top level alone when the web app named none, as for a
/// login that asked for one; else every one it named, the first included.
///
/// # Errors
///
/// When the web app named one by a name zirc can't keep ([`crate::settings::valid_org_name`]),
/// and then none is given.
pub fn each_org(credentials: &Credentials, web: &str) -> Result<Vec<OrgLogin>, Error> {
    if credentials.orgs.is_empty() {
        return Ok(vec![OrgLogin {
            name: None,
            credentials: credentials.clone(),
        }]);
    }
    if let Some(org) = credentials
        .orgs
        .iter()
        .find(|org| !crate::settings::valid_org_name(&org.name))
    {
        return Err(Error::OrgName {
            web: web.to_owned(),
            name: org.name.clone(),
        });
    }
    Ok(credentials
        .orgs
        .iter()
        .map(|org| OrgLogin {
            name: Some(org.name.clone()),
            credentials: Credentials {
                server: org.server.clone(),
                account: org.account.clone(),
                nick: org.nick.clone(),
                owner: org.owner.clone(),
                realname: org.realname.clone(),
                account_id: credentials.account_id.clone(),
                organisation_id: org.organisation_id.clone(),
                media: org.media.clone(),
                web_session: None,
                orgs: Vec::new(),
            },
        })
        .collect())
}

/// How long a started login's link works, for people: `10 minutes`, or `1 minute`.
#[must_use]
pub fn link_lifetime(started: &LoginStarted) -> String {
    match started.expires_in.div_ceil(60) {
        1 => "1 minute".to_owned(),
        minutes => format!("{minutes} minutes"),
    }
}

/// Opens `url` in the browser, if this machine has one to open: print it as well.
pub fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// A client of the web app's login API.
#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) web: String,
    /// What zirc.dev said of each account's server's content API ([`Client::history`]).
    content: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Content>>>,
}

/// Whether an account's history is read straight from its server, as zirc.dev last said.
#[derive(Debug, Clone)]
enum Content {
    /// From the server, with this token while it's good.
    Server(ContentAccess),
    /// Through zirc.dev, until then: its server doesn't answer content itself.
    Web(std::time::Instant),
}

/// How long an account whose server doesn't answer content itself is read through zirc.dev
/// before zirc.dev is asked again, so a server that turns it on is soon read directly.
const ASK_AGAIN: Duration = Duration::from_mins(10);

/// How much of a token's life must be left to use it, in milliseconds.
const TOKEN_MARGIN_MS: u64 = 30_000;

/// The milliseconds since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Client {
    /// A client of the web app at `web`, such as [`WEB`].
    ///
    /// # Errors
    ///
    /// When the HTTP client can't be set up, such as without the system's certificates.
    pub fn new(web: &str) -> Result<Self, Error> {
        // reqwest's rustls comes without a crypto provider: use ring, as zirc-irc does.
        // Installing it fails harmlessly when one is installed already.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(zirc_proto::version::user_agent())
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            web: web.trim_end_matches('/').to_owned(),
            content: std::sync::Arc::default(),
        })
    }

    /// Starts a login: step 1 of the handshake.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached, doesn't take logins, or refuses this one.
    pub async fn start(&self, request: &LoginRequest) -> Result<LoginStarted, Error> {
        let url = format!("{}{}", self.web, login::START_PATH);
        let response = self.http.post(url).json(request).send().await?;
        if matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::NOT_IMPLEMENTED
        ) {
            return Err(Error::Unavailable {
                web: self.web.clone(),
            });
        }
        Ok(self.checked(response).await?.json().await?)
    }

    /// Where the login stands: step 3 of the handshake, once.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers an error.
    pub async fn status(&self, id: &str) -> Result<LoginStatus, Error> {
        let url = format!("{}{}", self.web, login::status_path(id));
        let response = self.http.get(url).send().await?;
        // The web app forgets logins that are done or too old.
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(LoginStatus::Expired);
        }
        Ok(self.checked(response).await?.json().await?)
    }

    /// Polls the login until it's confirmed, and answers what to connect with. The web app
    /// going away for a while, as it does when it restarts, only means polling on: the login
    /// waits for it, until it expires.
    ///
    /// # Errors
    ///
    /// When the login is turned down or expires, or the web app refuses the polls.
    pub async fn wait(&self, started: &LoginStarted) -> Result<Credentials, Error> {
        let interval = Duration::from_secs(started.interval.max(1).into());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(started.expires_in.into());
        loop {
            // The last poll is at the deadline itself, not an interval before it: a login let in
            // in its last seconds is still collected, as the web app answers a decided login
            // even once it has lapsed. This clock's deadline is after the web app's, as it
            // starts once the web app has answered.
            let last = tokio::time::Instant::now() >= deadline;
            match self.status(&started.id).await {
                Ok(LoginStatus::Done(credentials)) => return Ok(credentials),
                Ok(LoginStatus::Denied) => return Err(Error::Denied),
                Ok(LoginStatus::Expired) => return Err(Error::Expired),
                Err(err) if last || !passing(&err) => return Err(err),
                // Still waiting, or the web app away for a moment: poll again.
                Ok(LoginStatus::Pending) | Err(_) => {}
            }
            if last {
                return Err(Error::Expired);
            }
            tokio::time::sleep_until((tokio::time::Instant::now() + interval).min(deadline)).await;
        }
    }

    /// A machine's current credentials, asked for as `account` with the passphrase it has,
    /// as zircd does when the server refuses that account: after a rename, they're the new
    /// name's. `None` when the web app no longer takes that account and passphrase, as for a
    /// revoked machine.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn machine(&self, account: &Account) -> Result<Option<Credentials>, Error> {
        let url = format!("{}{}", self.web, login::MACHINE_PATH);
        let response = self
            .http
            .post(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// A person's app's current credentials, given the `account` and passphrase it has, as an
    /// app does when the server refuses its login: after its person's handle changed
    /// elsewhere, the new handle's, with the same passphrase. `None` when the web app takes
    /// those from no one, as once the person was removed.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn person(&self, account: &Account) -> Result<Option<Credentials>, Error> {
        let url = format!("{}{}", self.web, login::PERSON_PATH);
        let response = self
            .http
            .post(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// The web app's address, without a trailing slash.
    #[must_use]
    pub fn web(&self) -> &str {
        &self.web
    }

    /// Makes the organisation of the machine `account` is a CI token called `name`
    /// ([`login::CI_TOKEN_PATH`]), with which its CI says how a build went. `None` if the web
    /// app doesn't know that account and passphrase.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else, such as that the machine's
    /// person is neither owner nor admin, or the name is in use.
    pub async fn ci_token(
        &self,
        account: &Account,
        name: &str,
    ) -> Result<Option<login::CiTokenMade>, Error> {
        let request = login::CiTokenRequest {
            name: name.to_owned(),
        };
        let response = self
            .http
            .post(format!("{}{}", self.web, login::CI_TOKEN_PATH))
            .basic_auth(&account.name, Some(&account.password))
            .json(&request)
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// The CI tokens of the organisation of the machine `account` is, oldest first. `None` if
    /// the web app doesn't know that account and passphrase.
    ///
    /// # Errors
    ///
    /// As [`Client::ci_token`].
    pub async fn ci_tokens(&self, account: &Account) -> Result<Option<login::CiTokens>, Error> {
        let response = self
            .http
            .get(format!("{}{}", self.web, login::CI_TOKEN_PATH))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// How the person whose machine `account` is stands with getting started, and which step
    /// is next ([`zirc_proto::onboarding::STEPS_PATH`]). `None` if the web app doesn't know
    /// that account and passphrase, or doesn't serve steps at all, as a self-hosted server's
    /// or an older one (404).
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn steps(
        &self,
        account: &Account,
    ) -> Result<Option<zirc_proto::onboarding::Progress>, Error> {
        self.read_me(account, zirc_proto::onboarding::STEPS_PATH)
            .await
    }

    /// What one of the web app's `/api/me/*` reads at `path` answers `account`, its machine or
    /// app: `None` when the web app doesn't know that account and passphrase (401), or doesn't
    /// serve `path` at all, as a self-hosted server's or an older one (404).
    pub(crate) async fn read_me<T: serde::de::DeserializeOwned>(
        &self,
        account: &Account,
        path: &str,
    ) -> Result<Option<T>, Error> {
        let response = self
            .http
            .get(format!("{}{path}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ) {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// Sends `body` with `method` to one of the web app's `/api/me/*` routes at `path`, as
    /// `account`, its machine or app: the response, or `None` as [`Client::read_me`] answers
    /// it (401, 404).
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or refuses it otherwise.
    pub async fn write_me<B: serde::Serialize + ?Sized>(
        &self,
        account: &Account,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<Option<reqwest::Response>, Error> {
        let response = self
            .http
            .request(method, format!("{}{path}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .json(body)
            .send()
            .await?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ) {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?))
    }

    /// Says a prompt about a step was shown, followed or put away
    /// ([`zirc_proto::onboarding::PROMPTS_PATH`]).
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or refuses it.
    pub async fn prompt(
        &self,
        account: &Account,
        seen: &zirc_proto::onboarding::PromptSeen,
    ) -> Result<(), Error> {
        let response = self
            .http
            .post(format!(
                "{}{}",
                self.web,
                zirc_proto::onboarding::PROMPTS_PATH
            ))
            .basic_auth(&account.name, Some(&account.password))
            .json(seen)
            .send()
            .await?;
        self.checked(response).await?;
        Ok(())
    }

    /// Revokes the CI token called `name`. `None` if the web app doesn't know that account and
    /// passphrase.
    ///
    /// # Errors
    ///
    /// As [`Client::ci_token`], and when no token in use has that name.
    pub async fn revoke_ci_token(
        &self,
        account: &Account,
        name: &str,
    ) -> Result<Option<()>, Error> {
        let request = login::CiTokenRequest {
            name: name.to_owned(),
        };
        let response = self
            .http
            .post(format!("{}{}", self.web, login::CI_TOKEN_REVOKE_PATH))
            .basic_auth(&account.name, Some(&account.password))
            .json(&request)
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        self.checked(response).await?;
        Ok(Some(()))
    }

    /// A page of the organisation's server log, asked for as `account`, an owner's machine:
    /// see [`zirc_proto::login`] for the query. `None` when the web app doesn't take that
    /// account and passphrase, as for a revoked machine.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else, such as that the
    /// machine's person isn't an owner.
    pub async fn log(&self, account: &Account, query: &LogQuery) -> Result<Option<LogPage>, Error> {
        let url = format!("{}{}", self.web, login::LOG_PATH);
        let mut params = vec![("since", query.since.as_str())];
        if let Some(text) = &query.text {
            params.push(("q", text));
        }
        if let Some(before) = &query.before {
            params.push(("before", before));
        }
        if query.all {
            params.push(("all", "1"));
        }
        let url =
            reqwest::Url::parse_with_params(&url, &params).map_err(|_| Error::Unavailable {
                web: self.web.clone(),
            })?;
        let response = self
            .http
            .get(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// A page of the organisation's history that `account`'s owner may see, asking for
    /// `query`, or `None` when the web app doesn't take the account.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or refuses, such as for a conversation the owner
    /// may not read, saying why.
    pub async fn history(
        &self,
        account: &Account,
        query: &HistoryQuery,
    ) -> Result<Option<HistoryPage>, Error> {
        let mut params: Vec<(&str, String)> = vec![("since", query.since.clone())];
        let optional = [
            ("conversation", &query.conversation),
            ("sender", &query.sender),
            ("q", &query.text),
            ("until", &query.until),
            ("cursor", &query.cursor),
        ];
        params.extend(
            optional
                .into_iter()
                .filter_map(|(key, value)| Some((key, value.clone()?))),
        );
        params.push(("limit", query.limit.to_string()));
        if query.events {
            params.push(("kinds", "all".to_owned()));
        }
        self.get_as(account, login::HISTORY_PATH, &params).await
    }

    /// The organisation's history as a search asks for it, with its query string's pairs as
    /// they are: what every app's search screen asks (zirc-chat's `ServerSearch::params`).
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached, or refuses.
    pub async fn search_history(
        &self,
        account: &Account,
        params: &[(&'static str, String)],
    ) -> Result<Option<HistoryPage>, Error> {
        self.get_as(account, login::HISTORY_PATH, params).await
    }

    /// The conversations with messages since `since` (RFC 3339) that `account`'s owner may
    /// see, or `None` when the web app doesn't take the account.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or refuses, saying why.
    pub async fn conversations(
        &self,
        account: &Account,
        since: &str,
    ) -> Result<Option<Conversations>, Error> {
        let params = [("since", since.to_owned())];
        self.get_as(account, login::CONVERSATIONS_PATH, &params)
            .await
    }

    /// The token to read `account`'s history straight from its server, if its server answers
    /// content itself: kept while it's good, and asked of zirc.dev otherwise. `None` to read
    /// through zirc.dev: its server doesn't, zirc.dev is an older one, or it can't be reached
    /// (then the read through zirc.dev says why).
    async fn content_access(&self, account: &Account) -> Option<ContentAccess> {
        let known = self.lock_content().get(&account.name).cloned();
        match known {
            Some(Content::Server(access)) if access.expires > now_ms() + TOKEN_MARGIN_MS => {
                return Some(access);
            }
            Some(Content::Web(until)) if std::time::Instant::now() < until => return None,
            _ => {}
        }
        let response = self
            .http
            .post(format!("{}{}", self.web, login::CONTENT_TOKEN_PATH))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await
            .ok()?;
        let access = if response.status().is_success() {
            response.json::<ContentAccess>().await.ok()
        } else {
            None
        };
        let known = match &access {
            Some(access) => Content::Server(access.clone()),
            None => Content::Web(std::time::Instant::now() + ASK_AGAIN),
        };
        self.lock_content().insert(account.name.clone(), known);
        access
    }

    fn forget_content(&self, account: &Account) {
        self.lock_content().remove(&account.name);
    }

    fn lock_content(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Content>> {
        self.content
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the server's content API answers at `path` with `params`, or `None` when it
    /// refuses the token.
    async fn get_from_server<T: serde::de::DeserializeOwned>(
        &self,
        access: &ContentAccess,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Option<T>, Error> {
        let url = format!("{}{path}", access.base.trim_end_matches('/'));
        let url =
            reqwest::Url::parse_with_params(&url, params).map_err(|_| Error::Unavailable {
                web: access.base.clone(),
            })?;
        let response = self.http.get(url).bearer_auth(&access.token).send().await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        // The organisation's own server answered, not zirc.dev: its error says so.
        let server = reqwest::Url::parse(&access.base).ok().map_or_else(
            || access.base.clone(),
            |url| url.origin().ascii_serialization(),
        );
        Ok(Some(
            Self::checked_from(response, &server).await?.json().await?,
        ))
    }

    /// What the web app answers at `path` with `params`, signed in as `account`. History is
    /// read straight from the organisation's server where it answers content itself
    /// ([`login::CONTENT_TOKEN_PATH`]), so it never passes through zirc.dev; through zirc.dev
    /// where it doesn't.
    async fn get_as<T: serde::de::DeserializeOwned>(
        &self,
        account: &Account,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Option<T>, Error> {
        let on_server = match path {
            login::HISTORY_PATH => Some(login::CONTENT_HISTORY_PATH),
            login::CONVERSATIONS_PATH => Some(login::CONTENT_CONVERSATIONS_PATH),
            _ => None,
        };
        if let Some(on_server) = on_server {
            // A token refused (it ended early, or the server's key changed) is asked for again once.
            for _ in 0..2 {
                let Some(access) = self.content_access(account).await else {
                    break;
                };
                match self.get_from_server(&access, on_server, params).await? {
                    Some(answer) => return Ok(Some(answer)),
                    None => self.forget_content(account),
                }
            }
        }
        let url = format!("{}{path}", self.web);
        let url =
            reqwest::Url::parse_with_params(&url, params).map_err(|_| Error::Unavailable {
                web: self.web.clone(),
            })?;
        let response = self
            .http
            .get(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Ok(None),
            // Not there: an older web app, unless it says why, as for an organisation
            // whose server doesn't serve it yet.
            StatusCode::NOT_FOUND => {
                let said = response
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|json| json.get("error")?.as_str().map(str::to_owned));
                return Err(match said {
                    Some(message) => Error::Status {
                        web: self.web.clone(),
                        status: StatusCode::NOT_FOUND,
                        message,
                    },
                    None => Error::NotYet {
                        web: self.web.clone(),
                        what: "the team's history",
                    },
                });
            }
            _ => {}
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// The response, or what the web app said was wrong: its JSON `error`, or its text.
    pub(crate) async fn checked(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, Error> {
        Self::checked_from(response, &self.web).await
    }

    /// The response, or what `web`, whichever answered it, said was wrong.
    async fn checked_from(
        response: reqwest::Response,
        web: &str,
    ) -> Result<reqwest::Response, Error> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            // The server's content API says why in `message`, beside a code in `error`;
            // zirc.dev in `error` alone.
            .and_then(|json| {
                let said = json.get("message").or_else(|| json.get("error"))?;
                said.as_str().map(str::to_owned)
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        Err(Error::Status {
            web: web.to_owned(),
            status,
            message,
        })
    }
}

/// What [`Client::log`] asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogQuery {
    /// How far back, such as `30m`, `12h`, `3d`, `2w` or `1mo`.
    pub since: String,
    /// Text the lines must have.
    pub text: Option<String>,
    /// Only lines before this time, for the page before one.
    pub before: Option<String>,
    /// Every line of the log, not only the messages people send.
    pub all: bool,
}

/// What `zirc search` asks of the organisation's history.
#[derive(Debug, Clone, Default)]
pub struct HistoryQuery {
    /// From when, as RFC 3339.
    pub since: String,
    /// Until when, as RFC 3339: now when absent.
    pub until: Option<String>,
    /// One channel, or a person's account for direct messages with them.
    pub conversation: Option<String>,
    /// Who said it: an account or a nick.
    pub sender: Option<String>,
    /// Text the messages must have.
    pub text: Option<String>,
    /// How many, at most, in this page (the server takes up to 200).
    pub limit: usize,
    /// Where the page before ends, from the last page's `next`.
    pub cursor: Option<String>,
    /// zircd's events too (working sets, commits, pushes), not only what's said.
    pub events: bool,
}

pub use zirc_proto::login::{HistoryMessage, HistoryPage, HistorySender};

/// The conversations with messages lately, newest first.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Conversations {
    /// Whose conversations the organisation lets its people read, as in [`HistoryPage`].
    pub policy: String,
    /// The conversations, the one with the newest message first.
    pub conversations: Vec<ConversationSeen>,
}

/// A conversation with messages lately.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ConversationSeen {
    /// The channel, or the account a direct message is with.
    pub conversation: String,
    /// `channel` or `dm`.
    pub kind: String,
    /// When its newest message was sent, as RFC 3339.
    pub last: String,
    /// How many messages it had since.
    pub count: usize,
}

/// Whether `err` is the web app being away for a moment, rather than refusing: unreachable
/// (its name not resolving included, as on a laptop changing networks), or answering 502,
/// 503 or 504 while the edge waits for it.
pub(crate) fn passing(err: &Error) -> bool {
    match err {
        Error::Http(err) => err.is_connect() || err.is_timeout() || err.is_request(),
        Error::Status { status, .. } => matches!(
            *status,
            StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_name_that_doesnt_resolve_is_passing() {
        // `.invalid` never resolves (RFC 6761), online or not.
        let client = Client::new("http://zirc.invalid").unwrap();
        let err = client.status("abc").await.unwrap_err();
        assert!(passing(&err), "{err:?}");
    }

    fn started(expires_in: u32) -> LoginStarted {
        LoginStarted {
            id: "L1".into(),
            code: "ABCD-EFGH".into(),
            url: "https://zirc.dev/login".into(),
            interval: 1,
            expires_in,
        }
    }

    const PENDING: &str = r#"{"status":"pending"}"#;

    #[tokio::test]
    async fn waits_through_the_web_app_restarting_until_the_login_is_done() {
        let done = r#"{"status":"done","server":{"host":"zirc.irc.zirc.dev"},"nick":"mattias"}"#;
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (503, "starting".into()),
            (200, PENDING.into()),
            (200, done.into()),
        ]);
        let client = Client::new(&web).unwrap();
        let credentials = client.wait(&started(60)).await.unwrap();
        assert_eq!(credentials.nick.as_deref(), Some("mattias"));
        assert_eq!(served.join().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn stops_waiting_when_the_web_app_refuses() {
        let (web, _served) =
            zirc_test_ergo::web::serve(vec![(500, r#"{"error":"broken"}"#.into())]);
        let client = Client::new(&web).unwrap();
        let err = client.wait(&started(60)).await.unwrap_err();
        assert!(!passing(&err));
        assert!(
            err.to_string()
                .ends_with("500 Internal Server Error: broken"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn the_last_poll_is_at_the_deadline_and_ends_the_wait() {
        // Let in in its last second: collected by the poll at the deadline.
        let done = r#"{"status":"done","server":{"host":"zirc.irc.zirc.dev"},"nick":"mattias"}"#;
        let (web, served) =
            zirc_test_ergo::web::serve(vec![(200, PENDING.into()), (200, done.into())]);
        let client = Client::new(&web).unwrap();
        let credentials = client.wait(&started(1)).await.unwrap();
        assert_eq!(credentials.nick.as_deref(), Some("mattias"));
        assert_eq!(served.join().unwrap().len(), 2);
        // Still pending at the deadline: expired.
        let (web, _served) =
            zirc_test_ergo::web::serve(vec![(200, PENDING.into()), (200, PENDING.into())]);
        let client = Client::new(&web).unwrap();
        let err = client.wait(&started(1)).await.unwrap_err();
        assert!(matches!(err, Error::Expired), "{err:?}");
        // Away on the last poll: what it answered, rather than another try.
        let (web, _served) =
            zirc_test_ergo::web::serve(vec![(503, "starting".into()), (503, "starting".into())]);
        let client = Client::new(&web).unwrap();
        let err = client.wait(&started(1)).await.unwrap_err();
        assert!(passing(&err), "{err:?}");
    }

    fn ada_laptop() -> Account {
        Account {
            name: "ada-laptop".into(),
            password: "machine-passphrase".into(),
            device_key: false,
        }
    }

    const PAGE: &str = r#"{"policy":"participant","messages":[],"next":null}"#;

    fn access(base: &str, token: &str) -> String {
        serde_json::json!({"base": base, "token": token, "expires": now_ms() + 300_000}).to_string()
    }

    fn query() -> HistoryQuery {
        HistoryQuery {
            since: "2026-11-07T00:00:00Z".into(),
            limit: 20,
            ..HistoryQuery::default()
        }
    }

    #[tokio::test]
    async fn a_servers_own_refusal_names_the_server_not_zirc_dev() {
        let refused = r#"{"error":"not-found","message":"this server doesn't serve content here"}"#;
        let (server, _asked) = zirc_test_ergo::web::serve(vec![(404, refused.into())]);
        let (web, _given) =
            zirc_test_ergo::web::serve(vec![(200, access(&format!("{server}/api/v1"), "tok-1"))]);
        let client = Client::new(&web).unwrap();
        let err = client.history(&ada_laptop(), &query()).await.unwrap_err();
        let said = err.to_string();
        let origin = reqwest::Url::parse(&server)
            .unwrap()
            .origin()
            .ascii_serialization();
        assert!(
            said.starts_with(&format!("{origin} answered 404")),
            "{said}"
        );
        assert!(said.contains("doesn't serve content here"), "{said}");
    }

    #[tokio::test]
    async fn history_is_read_from_the_server_itself_where_it_answers_content() {
        let conversations = r#"{"policy":"participant","conversations":[],"complete":true}"#;
        let (server, asked) =
            zirc_test_ergo::web::serve(vec![(200, PAGE.into()), (200, conversations.into())]);
        let (web, given) =
            zirc_test_ergo::web::serve(vec![(200, access(&format!("{server}/api/v1"), "tok-1"))]);
        let client = Client::new(&web).unwrap();

        assert!(
            client
                .history(&ada_laptop(), &query())
                .await
                .unwrap()
                .is_some()
        );
        // The token is kept while it's good: zirc.dev is asked once.
        assert!(
            client
                .conversations(&ada_laptop(), "2026-11-07T00:00:00Z")
                .await
                .unwrap()
                .is_some()
        );

        let given = given.join().unwrap();
        assert_eq!(given.len(), 1);
        assert!(
            given[0].starts_with("POST /api/cli/content-token "),
            "{}",
            given[0]
        );
        assert!(
            given[0].to_lowercase().contains("authorization: basic "),
            "{}",
            given[0]
        );
        let asked = asked.join().unwrap();
        assert!(
            asked[0].starts_with("GET /api/v1/history?since="),
            "{}",
            asked[0]
        );
        assert!(
            asked[1].starts_with("GET /api/v1/conversations?since="),
            "{}",
            asked[1]
        );
        assert!(
            asked
                .iter()
                .all(|r| r.to_lowercase().contains("authorization: bearer tok-1"))
        );
    }

    #[tokio::test]
    async fn where_the_server_doesnt_it_goes_through_zirc_dev_without_asking_each_time() {
        let off = r#"{"error":"This server's content is read through zirc.dev."}"#;
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (404, off.into()),
            (200, PAGE.into()),
            (200, PAGE.into()),
        ]);
        let client = Client::new(&web).unwrap();

        assert!(
            client
                .history(&ada_laptop(), &query())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            client
                .history(&ada_laptop(), &query())
                .await
                .unwrap()
                .is_some()
        );

        let served = served.join().unwrap();
        assert!(served[0].starts_with("POST /api/cli/content-token "));
        assert!(served[1].starts_with("GET /api/cli/history?"));
        assert!(served[2].starts_with("GET /api/cli/history?"));
    }

    #[tokio::test]
    async fn a_token_the_server_refuses_is_asked_for_again_once_and_its_reason_is_said() {
        let (server, asked) = zirc_test_ergo::web::serve(vec![
            (
                401,
                r#"{"error":"unauthorized","message":"expired"}"#.into(),
            ),
            (
                403,
                r##"{"error":"visibility","message":"#secret isn't one you may read here"}"##
                    .into(),
            ),
        ]);
        let base = format!("{server}/api/v1");
        let (web, given) = zirc_test_ergo::web::serve(vec![
            (200, access(&base, "old")),
            (200, access(&base, "new")),
        ]);
        let client = Client::new(&web).unwrap();

        let err = client.history(&ada_laptop(), &query()).await.unwrap_err();

        assert!(
            err.to_string()
                .ends_with("#secret isn't one you may read here"),
            "{err}"
        );
        assert_eq!(given.join().unwrap().len(), 2);
        let asked = asked.join().unwrap();
        assert!(asked[0].to_lowercase().contains("bearer old"));
        assert!(asked[1].to_lowercase().contains("bearer new"));
    }

    #[test]
    fn says_how_long_a_link_works() {
        assert_eq!(link_lifetime(&started(60)), "1 minute");
        assert_eq!(link_lifetime(&started(30)), "1 minute");
        assert_eq!(link_lifetime(&started(600)), "10 minutes");
    }
}
