//! How zirc's apps reach an organisation's IRC server through zirc.dev, and the paths of
//! zirc.dev's organisation API that they call, said once for the web app that answers and
//! every client that asks:
//!
//! - `GET` [`ORGS_PATH`]: the person's organisations, [`Orgs`].
//! - `POST` [`TOKEN_PATH`] with a [`TokenRequest`]: a token for an organisation's server,
//!   [`IrcToken`].
//! - `GET` [`GUEST_PATH`]`?org=&channel=`: whether a public channel takes guests,
//!   [`PublicChannel`]; `POST` it with a [`GuestJoin`] to join as one, [`GatewayToken`].
//!
//! A refusal is an error status with an [`ApiError`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The person's organisations, under the web app's address.
pub const ORGS_PATH: &str = "/api/irc/orgs";

/// A token for an organisation's server.
pub const TOKEN_PATH: &str = "/api/irc/token";

/// A public channel's guests: whether it takes them, and a token to join as one.
pub const GUEST_PATH: &str = "/api/irc/guest";

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
    /// Whether they're one of the organisation's owners or admins, who manage its guests:
    /// left out when they aren't.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub admin: bool,
}

/// What `GET` [`GUEST_PATH`] answers for a public channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct PublicChannel {
    /// The captcha's site key guests pass, if there's one.
    pub turnstile: Option<String>,
    /// Whether it takes guests now.
    pub open: bool,
}

/// What an app sends `POST` [`GUEST_PATH`] to join a public channel as a guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields)]
pub struct GuestJoin {
    /// The organisation's short name.
    pub org: String,
    /// The channel, `#support`.
    pub channel: String,
    /// The captcha's answer, the first time; later joins have zirc.dev's guest cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turnstile: Option<String>,
    /// The name the guest typed, if any ([`crate::profile::name`]): their real name on the
    /// server, which staff see. Their nick stays `guest-…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What `POST` [`GUEST_PATH`] answers: the guest gateway to connect to, as whom, and the
/// token it takes as the server password. 400 for a failed captcha, 403 for a banned guest,
/// 404 for no such public channel, 409 (with [`ApiError::member`]) for one of the
/// organisation's members, 429 at the network's guest limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayToken {
    /// The gateway's address.
    pub url: String,
    /// How it takes the token: `pass`, as the server password.
    pub login: String,
    /// The guest's name, [`crate::GUEST_PREFIX`] and an id ([`crate::names::is_guest_account`]).
    pub guest: String,
    /// Whether the guest is signed in to zirc.dev.
    pub signed_in: bool,
    /// The token.
    pub token: String,
    /// The channel to join, lower-cased: the one asked for, or for `#support` the guest's room
    /// of their own beside it ([`crate::guest_room`]).
    pub channel: String,
    /// When the name they gave, or their profile's, would pass for someone on the team, the
    /// name they appear as instead, such as `Guest 1a2b`: the chat says so, in
    /// [`NAME_REPLACED_NOTE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub replaced: Option<String>,
}

/// What a guest's chat says before the name they appear as ([`GatewayToken::replaced`]).
pub const NAME_REPLACED_NOTE: &str = "That name belongs to someone on the team, so you appear as";

/// What zirc.dev answers when it refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "IrcError")]
pub struct ApiError {
    /// Why, for people.
    pub error: String,
    /// For a guest who's one of the organisation's members, who'd join as themselves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub member: bool,
}

/// The organisation API's path for the organisation `org` (its short name):
/// `/api/organisations/<org>`.
#[must_use]
pub fn organisation_path(org: &str) -> String {
    format!("/api/organisations/{org}")
}

/// The Guests API's path for the guest `nick` of the organisation `org`.
#[must_use]
pub fn guest_path(org: &str, nick: &str) -> String {
    format!("{}/guests/{nick}", organisation_path(org))
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
        format!("const IRC_GUEST_PATH = {GUEST_PATH:?};"),
        format!("const IRC_NAME_REPLACED_NOTE = {NAME_REPLACED_NOTE:?};"),
        Orgs::decl(config),
        Org::decl(config),
        TokenRequest::decl(config),
        IrcToken::decl(config),
        PublicChannel::decl(config),
        GuestJoin::decl(config),
        GatewayToken::decl(config),
        ApiError::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_zirc_dev_answers() {
        let token: IrcToken = serde_json::from_str(
            r#"{"server":"wss://acme.irc.zirc.dev","nick":"ada","token":"t"}"#,
        )
        .unwrap();
        assert!(!token.admin && token.realname.is_none());
        let gateway: GatewayToken = serde_json::from_str(
            r##"{"url":"wss://acme.irc.zirc.dev/guest","login":"pass","guest":"guest-1a2b3c4d",
                "signedIn":true,"token":"t","channel":"#support"}"##,
        )
        .unwrap();
        assert!(gateway.signed_in && crate::names::is_guest_account(&gateway.guest));
        assert!(gateway.replaced.is_none());
        let replaced: GatewayToken = serde_json::from_str(
            r##"{"url":"u","login":"pass","guest":"guest-1a2b3c4d","signedIn":false,"token":"t",
                "channel":"#support","replaced":"Guest 1a2b"}"##,
        )
        .unwrap();
        assert_eq!(replaced.replaced.as_deref(), Some("Guest 1a2b"));
        let refused: ApiError =
            serde_json::from_str(r#"{"error":"You're a member","member":true}"#).unwrap();
        assert!(refused.member);
        assert_eq!(
            serde_json::to_string(&ApiError {
                error: "no".into(),
                member: false
            })
            .unwrap(),
            r#"{"error":"no"}"#
        );
    }

    #[test]
    fn makes_the_organisation_apis_paths() {
        assert_eq!(
            guest_path("acme", "guest-7k2q"),
            "/api/organisations/acme/guests/guest-7k2q"
        );
        assert_eq!(handle_path("acme"), "/api/organisations/acme/handle");
        assert_eq!(
            member_path("acme", "ada"),
            "/api/organisations/acme/members/ada"
        );
    }
}
