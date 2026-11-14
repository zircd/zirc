//! The server's TLS certificate, where it ends TLS itself: from Let's Encrypt (ACME), answered
//! over HTTP on port 80, and renewed a month before it runs out; or one Ergo makes for itself,
//! for trying the server out. Ergo reads `fullchain.pem` and `privkey.pem` in its data
//! directory, as its release's own listener does, and again when told to (`REHASH`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};
use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::routing::get;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};

use crate::secrets::write_private;

/// The certificate chain, as Ergo's listener names it.
pub const CHAIN: &str = "fullchain.pem";
/// Its key.
pub const KEY: &str = "privkey.pem";
/// The ACME account's credentials, kept so renewals use the same account.
const ACCOUNT: &str = "acme-account.json";

/// A certificate is renewed once it has fewer days left than this.
const RENEW_WITHIN_DAYS: i64 = 30;

/// Let's Encrypt's directory.
pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// Answers ACME's HTTP-01 challenges: the key authorization for each token being validated.
#[derive(Clone, Default)]
pub struct Challenges(Arc<Mutex<HashMap<String, String>>>);

impl Challenges {
    fn set(&self, token: String, answer: String) {
        self.0.lock().expect("challenges").insert(token, answer);
    }

    fn get(&self, token: &str) -> Option<String> {
        self.0.lock().expect("challenges").get(token).cloned()
    }

    /// Serves them on `listen` until the process ends, and sends anything else to HTTPS on
    /// `host`.
    ///
    /// # Errors
    ///
    /// When it can't listen there.
    pub async fn serve(self, listen: SocketAddr, host: String) -> std::io::Result<()> {
        async fn answer(
            State(challenges): State<Challenges>,
            UrlPath(token): UrlPath<String>,
        ) -> Result<String, StatusCode> {
            challenges.get(&token).ok_or(StatusCode::NOT_FOUND)
        }
        let to_https = move |uri: axum::http::Uri| async move {
            let path = uri.path_and_query().map_or("/", |p| p.as_str());
            axum::response::Redirect::permanent(&format!("https://{host}{path}"))
        };
        let app = Router::new()
            .route("/.well-known/acme-challenge/{token}", get(answer))
            .fallback(to_https)
            .with_state(self);
        let listener = tokio::net::TcpListener::bind(listen).await?;
        axum::serve(listener, app).await
    }
}

/// How ACME is reached, and who it may write to.
#[derive(Clone, Debug)]
pub struct Acme {
    /// The ACME directory's URL.
    pub directory: String,
    /// The address the account gives, for word about its certificates.
    pub email: Option<String>,
}

/// Days left on the certificate in `data`, if there's one that can be read.
#[must_use]
pub fn days_left(data: &Path) -> Option<i64> {
    let pem = std::fs::read(data.join(CHAIN)).ok()?;
    let (_, pem) = x509_parser::pem::parse_x509_pem(&pem).ok()?;
    let certificate = pem.parse_x509().ok()?;
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    Some((certificate.validity().not_after.timestamp() - now) / 86_400)
}

/// Whether the certificate in `data` is missing or due for renewal.
#[must_use]
pub fn due(data: &Path) -> bool {
    days_left(data).is_none_or(|days| days < RENEW_WITHIN_DAYS)
}

/// Gets a certificate for `host` into `data` from `acme`, answering its challenge through
/// `challenges`, which must be served on port 80 of `host` already.
///
/// # Errors
///
/// When ACME can't be reached, refuses, or `host` can't be validated.
pub async fn obtain(
    host: &str,
    data: &Path,
    acme: &Acme,
    challenges: &Challenges,
) -> anyhow::Result<()> {
    let account = account(data, acme).await?;
    let identifiers = [Identifier::Dns(host.to_owned())];
    let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
    let mut authorizations = order.authorizations();
    while let Some(authorization) = authorizations.next().await {
        let mut authorization = authorization?;
        match authorization.status {
            AuthorizationStatus::Pending => {}
            AuthorizationStatus::Valid => continue,
            status => bail!("{host}'s authorization is {status:?}"),
        }
        let mut challenge = authorization
            .challenge(ChallengeType::Http01)
            .context("ACME offered no HTTP-01 challenge")?;
        challenges.set(
            challenge.token.clone(),
            challenge.key_authorization().as_str().to_owned(),
        );
        challenge.set_ready().await?;
    }
    let status = order.poll_ready(&RetryPolicy::default()).await?;
    if status != OrderStatus::Ready {
        bail!(
            "Let's Encrypt couldn't validate {host} ({status:?}): does it point at this machine, \
             with port 80 open to the internet?"
        );
    }
    let key = order.finalize().await?;
    let chain = order.poll_certificate(&RetryPolicy::default()).await?;
    write_private(&data.join(KEY), key.as_bytes())?;
    write_private(&data.join(CHAIN), chain.as_bytes())?;
    tracing::info!("got a certificate for {host}");
    Ok(())
}

/// The ACME account in `data`, made there first if there's none.
async fn account(data: &Path, acme: &Acme) -> anyhow::Result<Account> {
    let path = data.join(ACCOUNT);
    if let Ok(saved) = std::fs::read(&path) {
        let credentials: AccountCredentials = serde_json::from_slice(&saved)?;
        return Ok(Account::builder()?.from_credentials(credentials).await?);
    }
    let contact = acme.email.as_ref().map(|email| format!("mailto:{email}"));
    let contact: Vec<&str> = contact.iter().map(String::as_str).collect();
    let (account, credentials) = Account::builder()?
        .create(
            &NewAccount {
                contact: &contact,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            acme.directory.clone(),
            None,
        )
        .await?;
    write_private(&path, &serde_json::to_vec(&credentials)?)?;
    Ok(account)
}

/// Makes a certificate for `host` in `data`, signed by itself, which clients must be told to
/// trust: for trying the server out.
///
/// # Errors
///
/// When it can't be made or written.
pub fn self_signed(host: &str, data: &Path) -> anyhow::Result<()> {
    let made = rcgen::generate_simple_self_signed(vec![host.to_owned()])?;
    write_private(&data.join(KEY), made.signing_key.serialize_pem().as_bytes())?;
    write_private(&data.join(CHAIN), made.cert.pem().as_bytes())?;
    tracing::info!("made a self-signed certificate for {host}");
    Ok(())
}

/// How often the certificate is looked at, to renew it when it's due.
pub const CHECK_EVERY: Duration = Duration::from_hours(12);

#[cfg(test)]
mod tests {
    #[test]
    fn a_self_signed_certificate_lasts_and_isn_t_due() {
        let data = tempfile::tempdir().unwrap();
        super::self_signed("irc.acme.test", data.path()).unwrap();
        assert!(super::days_left(data.path()).is_some_and(|days| days > 30));
        assert!(!super::due(data.path()));
    }

    #[test]
    fn without_a_certificate_one_is_due() {
        let data = tempfile::tempdir().unwrap();
        assert_eq!(super::days_left(data.path()), None);
        assert!(super::due(data.path()));
    }
}
