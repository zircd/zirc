//! How zirc's apps reach an organisation's IRC server through zirc.dev, and the paths of
//! zirc.dev's organisation API that they call, said once for the web app that answers and
//! every client that asks:
//!
//! - `GET` [`ORGS_PATH`]: the person's organisations, [`Orgs`].
//! - `POST` [`TOKEN_PATH`] with a [`TokenRequest`]: a token for an organisation's server,
//!   [`IrcToken`].
//!
//! A refusal is an error status with an [`ApiError`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The person's organisations, under the web app's address.
pub const ORGS_PATH: &str = "/api/irc/orgs";

/// A token for an organisation's server.
pub const TOKEN_PATH: &str = "/api/irc/token";

/// What [`ORGS_PATH`] answers: 401 when nobody's signed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "IrcOrgs")]
pub struct Orgs {
    /// Sorted by name.
    pub orgs: Vec<Org>,
}

/// One of the person's organisations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "IrcOrg")]
pub struct Org {
    /// Its short name.
    pub slug: String,
    /// Its name.
    pub name: String,
}

/// What an app sends [`TOKEN_PATH`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "IrcTokenRequest")]
pub struct TokenRequest {
    /// The organisation's short name.
    pub org: String,
}

/// What [`TOKEN_PATH`] answers: 401 when nobody's signed in, 404 for no such organisation,
/// 409 while its server is still being set up, 503 when zirc.dev can't sign tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields)]
pub struct IrcToken {
    /// The server's address, `wss://<host>`.
    pub server: String,
    /// The person's nick there.
    pub nick: String,
    /// The token to log in with.
    pub token: String,
    /// Their name, as their profile gives it, to connect with; none when they have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realname: Option<String>,
    /// Whether they're one of the organisation's owners or admins: left out when they
    /// aren't.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub admin: bool,
}

/// What zirc.dev answers when it refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "IrcError")]
pub struct ApiError {
    /// Why, for people.
    pub error: String,
}

/// The organisation API's path for the organisation `org` (its short name):
/// `/api/organisations/<org>`.
#[must_use]
pub fn organisation_path(org: &str) -> String {
    format!("/api/organisations/{org}")
}

/// The handle API's path for the organisation `org`: `POST` a [`crate::profile::HandleChange`]
/// to change the caller's own handle (their nick) there. See [`crate::profile`].
#[must_use]
pub fn handle_path(org: &str) -> String {
    format!("{}/handle", organisation_path(org))
}

/// The members API's path for the member `handle` of the organisation `org`.
#[must_use]
pub fn member_path(org: &str, handle: &str) -> String {
    format!("{}/members/{handle}", organisation_path(org))
}

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        format!("const IRC_ORGS_PATH = {ORGS_PATH:?};"),
        format!("const IRC_TOKEN_PATH = {TOKEN_PATH:?};"),
        Orgs::decl(config),
        Org::decl(config),
        TokenRequest::decl(config),
        IrcToken::decl(config),
        ApiError::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_zirc_dev_answers() {
        let token: IrcToken = serde_json::from_str(
            r#"{"server":"wss://zirc.irc.zirc.dev","nick":"ada","token":"t"}"#,
        )
        .unwrap();
        assert!(!token.admin && token.realname.is_none());
        assert_eq!(
            serde_json::to_string(&ApiError { error: "no".into() }).unwrap(),
            r#"{"error":"no"}"#
        );
    }

    #[test]
    fn makes_the_organisation_apis_paths() {
        assert_eq!(handle_path("zirc"), "/api/organisations/zirc/handle");
        assert_eq!(
            member_path("zirc", "ada"),
            "/api/organisations/zirc/members/ada"
        );
    }
}
