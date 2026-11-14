//! Joining the organisation on zirc.dev, once, before the server lets anyone in
//! (`zirc_proto::join`): it says where it is, and zirc.dev, once it has reached it there, gives
//! it the tokens its sidecar takes, which become the server's own. `joined.json` in the data
//! directory then says it has, and it's never asked again.

use std::path::Path;
use std::time::Duration;

use anyhow::bail;
use reqwest::StatusCode;
use zirc_proto::join::{JOIN_PATH, JoinRequest, Joined};

use crate::secrets::{Secrets, write_private};

/// The file that says the server has joined, in the data directory.
pub const JOINED: &str = "joined.json";

/// How long it waits before trying again.
const AGAIN_AFTER: Duration = Duration::from_secs(30);

/// What's noted once the server has joined: the tokens stay in the secrets alone.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Noted {
    /// The organisation's short name.
    pub organisation: String,
    /// zirc.dev's address, as it joined it.
    pub web: String,
}

/// The organisation the server in `data` has joined, if it has.
#[must_use]
pub fn joined(data: &Path) -> Option<Noted> {
    let text = std::fs::read(data.join(JOINED)).ok()?;
    serde_json::from_slice(&text).ok()
}

/// Joins zirc.dev at `web` with `request`, unless the server in `data` has already, trying
/// until zirc.dev takes it; then keeps the tokens it gave in the server's secrets.
///
/// # Errors
///
/// When zirc.dev refuses the token, which trying again won't change, or what it gave can't be
/// kept.
pub async fn join(web: &str, request: &JoinRequest, data: &Path) -> anyhow::Result<()> {
    if let Some(noted) = joined(data) {
        tracing::info!("joined to {} on {}", noted.organisation, noted.web);
        return Ok(());
    }
    let http = zirc_sidecar::http();
    let url = format!("{}{JOIN_PATH}", web.trim_end_matches('/'));
    loop {
        match http.post(&url).json(request).send().await {
            Ok(answer) if answer.status().is_success() => match answer.json::<Joined>().await {
                Ok(given) => return keep(web, &given, data),
                Err(e) => tracing::warn!("{web} answered the join with something else: {e}"),
            },
            Ok(answer) if answer.status() == StatusCode::UNAUTHORIZED => bail!(
                "{web} doesn't take this join token: it's wrong, or used already. Make a new one \
                 there, put it in ZIRC_JOIN_TOKEN, and start the server again."
            ),
            Ok(answer) if answer.status() == StatusCode::CONFLICT => tracing::warn!(
                "{web} couldn't reach this server at https://{}/healthz: is port 443 open to the \
                 internet? Trying again.",
                request.host
            ),
            Ok(answer) => {
                tracing::warn!("{web} answered the join {}: trying again", answer.status());
            }
            Err(e) => tracing::warn!("couldn't reach {web} to join: {e}"),
        }
        tokio::time::sleep(AGAIN_AFTER).await;
    }
}

/// Keeps what zirc.dev gave as the server's own tokens, then notes that it has joined.
fn keep(web: &str, given: &Joined, data: &Path) -> anyhow::Result<()> {
    let mut secrets = Secrets::read(data)?;
    secrets.accounts_token.clone_from(&given.accounts_token);
    secrets.controller_token.clone_from(&given.controller_token);
    secrets.write(data)?;
    crate::media::keep_settings(data, &given.media_settings)?;
    let noted = Noted {
        organisation: given.organisation.clone(),
        web: web.to_owned(),
    };
    write_private(&data.join(JOINED), &serde_json::to_vec(&noted)?)?;
    tracing::info!("joined {} on {web}", given.organisation);
    Ok(())
}
