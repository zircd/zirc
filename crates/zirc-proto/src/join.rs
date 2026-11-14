//! A self-hosted server joining its organisation on zirc.dev. The owner makes the organisation
//! "on my own server" at zirc.dev, with its host name, which shows a join token for the
//! server's configuration; on its first start, before it lets anyone in, the server joins:
//!
//! - `POST /api/servers/join` ([`JOIN_PATH`]) with a [`JoinRequest`], answered with
//!   [`Joined`]: 401 for a token that isn't one, or is used; 409 when the server doesn't answer
//!   at `https://<host>/healthz`, which zirc.dev checks before it trusts it.
//!
//! From then on zirc.dev manages it as it does a server it hosts: through its sidecar's
//! endpoint, at [`SIDECAR_PATH`] on the server's HTTPS, with the tokens [`Joined`] gave it, as
//! zirc.dev's tenant controller made them. People sign in at zirc.dev; what they say, and
//! their files, stay on the server.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a server joins, under zirc.dev's address.
pub const JOIN_PATH: &str = "/api/servers/join";

/// Where a self-hosted server serves its sidecar's endpoint for zirc.dev, under its own
/// `https://<host>`: what a hosted server's sidecar serves on the cluster's network alone.
pub const SIDECAR_PATH: &str = "/sidecar";

/// What a server says as it joins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    /// The join token zirc.dev showed the owner: once only.
    pub token: String,
    /// The host name people connect to, such as `irc.example.com`: the one the organisation
    /// was made with.
    pub host: String,
    /// IRC's port there, over TLS.
    pub port: u16,
    /// The server's version, such as `zirc-server 0.1.45`.
    pub version: String,
}

/// What zirc.dev answers a server that's joined: which organisation it is, and the tokens its
/// sidecar takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Joined {
    /// The organisation's short name, such as `acme`.
    pub organisation: String,
    /// The token zirc.dev presents at the sidecar's endpoint, as to a hosted server's.
    pub accounts_token: String,
    /// The token zirc.dev's tenant controller presents to set teams and settings.
    pub controller_token: String,
    /// The organisation's files settings as zirc.dev has them (`zirc_proto::uploads::Settings`),
    /// which the server keeps until zirc.dev sends new ones to
    /// [`crate::media::SETTINGS_PATH`].
    #[serde(default)]
    #[ts(type = "MediaSettings")]
    pub media_settings: serde_json::Value,
}

/// The TypeScript the web app reads these as.
pub(crate) fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        JoinRequest::decl(config),
        Joined::decl(config),
        format!("const JOIN_PATH = {JOIN_PATH:?};"),
        format!("const SIDECAR_PATH = {SIDECAR_PATH:?};"),
    ]
}
