//! A guest's terminal: a public channel's guest lets zircd on their computer in as them, so
//! an agent there can talk in their room as they do in the browser.
//!
//! 1. `zirc login acme/web` starts a login ([`crate::login::LoginRequest::guest`]) naming a
//!    [`Target`], with the machine's device key. The page to open is the channel's chat
//!    ([`chat_url`]) with `&terminal=<code>`.
//! 2. The guest confirms the code in that chat, which zirc.dev knows them by. The poll then
//!    answers [`crate::login::Credentials::guest`]: the gateway's address, the guest, their
//!    room and a [`grant`](GuestCredentials::grant).
//! 3. zircd connects to the gateway as the browser does, its first line [`pass`]: the grant
//!    and a fresh device-key token ([`crate::device_key`]) for the guest, whose `aud` is the
//!    grant's id. The gateway checks both by itself: the grant with the organisation's token
//!    keys, the token with the key the grant names, each token once.
//! 4. A grant is short ([`GRANT_LIFETIME`]). zircd renews it at [`RENEW_PATH`] with such a
//!    token, which zirc.dev refuses once the guest is banned or forgotten, so a terminal lasts
//!    as long as its guest.
//!
//! A grant's claims ([`Grant`]) name the guest `sub` and their room `room`, never `guest` and
//! `channel` as a browser's token does: a gateway from before terminals reads neither, so it
//! never takes a grant for a guest's token without its device's proof.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where zircd renews a terminal's grant, under the web app's address: POST a
/// [`RenewRequest`], answered with [`GuestCredentials`], or 403 once the guest is banned or
/// forgotten, or the channel takes terminals no more.
pub const RENEW_PATH: &str = "/api/irc/guest/terminal";

/// How long a grant is good for, in seconds: a day, renewed before then.
pub const GRANT_LIFETIME: u64 = 24 * 60 * 60;

/// The longest a machine's name may be in a grant.
pub const MACHINE_MAX: usize = 32;

/// The public channel a terminal asks to join as its guest ([`crate::login::LoginRequest::guest`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct GuestTarget {
    /// The organisation, by its slug on zirc.dev.
    pub org: String,
    /// The public channel, such as `#web`.
    pub channel: String,
}

/// What a terminal connects with ([`crate::login::Credentials::guest`]), and what renewing its
/// grant answers. Its `Debug` leaves the grant out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct GuestCredentials {
    /// The guest gateway's WebSocket, such as `wss://acme.irc.zirc.dev/guest`.
    pub url: String,
    /// The organisation's slug.
    pub org: String,
    /// The guest, such as `guest-21042`: the terminal is them.
    pub guest: String,
    /// The channel to join: the guest's room, such as `#web/guest-21042`, or the public channel.
    pub channel: String,
    /// The public channel it's in, such as `#web`, as people name it.
    pub public: String,
    /// The grant, a JWT zirc.dev signed with the organisation's key ([`Grant`]).
    pub grant: String,
    /// Until when it's good, in seconds since the epoch.
    pub expires: u64,
}

impl std::fmt::Debug for GuestCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestCredentials")
            .field("url", &self.url)
            .field("org", &self.org)
            .field("guest", &self.guest)
            .field("channel", &self.channel)
            .field("public", &self.public)
            .field("grant", &"…")
            .field("expires", &self.expires)
            .finish()
    }
}

/// What zircd sends to renew a grant ([`RENEW_PATH`]).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RenewRequest {
    /// The grant it has, even expired: it says whose terminal this is.
    pub grant: String,
    /// A device-key token for the guest whose `aud` is the grant's id, from the key it names.
    pub proof: String,
}

impl std::fmt::Debug for RenewRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenewRequest").finish_non_exhaustive()
    }
}

/// A grant's claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The guest, such as `guest-21042`.
    pub sub: String,
    /// The channel they may be in: their room, or the public channel.
    pub room: String,
    /// The machine's name, as the guest was shown it.
    pub machine: String,
    /// The device key the terminal proves it has, as text ([`crate::device_key::key_text`]).
    pub key: String,
    /// The grant's own random id, which each connection's token names as its `aud`.
    pub gid: String,
    /// Until when it's good, in seconds since the epoch.
    pub exp: u64,
}

/// A grant's claims read from the grant, not checked: what zircd and the gateway learn of it
/// before checking it.
///
/// # Errors
///
/// When it isn't a grant.
pub fn grant_claims(grant: &str) -> Result<Grant, &'static str> {
    use base64::Engine as _;
    let claims = grant.split('.').nth(1).ok_or("not a grant")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(claims)
        .map_err(|_| "not a grant")?;
    serde_json::from_slice(&bytes).map_err(|_| "not a grant")
}

/// A terminal's first line's password: its grant, then the connection's token.
#[must_use]
pub fn pass(grant: &str, proof: &str) -> String {
    format!("{grant} {proof}")
}

/// A [`pass`] taken apart: the grant and the token, if it is one.
#[must_use]
pub fn split_pass(pass: &str) -> Option<(&str, &str)> {
    let (grant, proof) = pass.split_once(' ')?;
    (!grant.is_empty() && !proof.is_empty() && !proof.contains(' ')).then_some((grant, proof))
}

/// Where a terminal asks to go: `zirc login acme/web`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The organisation's slug.
    pub org: String,
    /// The public channel, with its `#`.
    pub channel: String,
    /// The web app, when given by the chat's own address; zirc.dev's otherwise.
    pub web: Option<String>,
}

/// What `zirc login` takes: `acme/web` or `acme/#web`, the organisation's slug and its public
/// channel, or the channel's chat address,
/// `https://zirc.dev/chat?org=acme&channel=%23web&guest`.
///
/// # Errors
///
/// Why it isn't one.
pub fn parse_target(text: &str) -> Result<Target, &'static str> {
    const HOW: &str = "name an organisation and its public channel, such as acme/web";
    let text = text.trim();
    if let Some(rest) = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
    {
        let (host, query) = rest.split_once('?').ok_or(HOW)?;
        let host = host.trim_end_matches('/');
        let path = host.find('/').map_or("", |i| &host[i..]);
        if path != "/chat" {
            return Err(HOW);
        }
        let (mut org, mut channel) = (None, None);
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = value
                .replace("%23", "#")
                .replace("%2F", "/")
                .replace("%2f", "/");
            match key {
                "org" => org = Some(value),
                "channel" => channel = Some(value),
                _ => {}
            }
        }
        let scheme = if text.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        let site = &host[..host.len() - path.len()];
        return Ok(Target {
            org: org_slug(&org.ok_or(HOW)?)?,
            channel: channel_name(&channel.ok_or(HOW)?)?,
            web: Some(format!("{scheme}://{site}")),
        });
    }
    let (org, channel) = text.split_once('/').ok_or(HOW)?;
    Ok(Target {
        org: org_slug(org)?,
        channel: channel_name(channel)?,
        web: None,
    })
}

fn org_slug(org: &str) -> Result<String, &'static str> {
    let org = org.to_lowercase();
    let good = !org.is_empty()
        && org.len() <= 64
        && org
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    good.then_some(org)
        .ok_or("that isn't an organisation's name")
}

fn channel_name(channel: &str) -> Result<String, &'static str> {
    let bare = channel.strip_prefix('#').unwrap_or(channel).to_lowercase();
    let good = !bare.is_empty()
        && bare.len() < 64
        && !bare.starts_with('/')
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'));
    good.then(|| format!("#{bare}"))
        .ok_or("that isn't a channel's name")
}

/// A public channel's chat on the web app at `web`, where its guest confirms a terminal's
/// code.
#[must_use]
pub fn chat_url(web: &str, org: &str, channel: &str, code: &str) -> String {
    format!(
        "{}/chat?org={org}&channel={}&guest&terminal={code}",
        web.trim_end_matches('/'),
        channel.replace('#', "%23"),
    )
}

/// A machine's name as a grant may carry it: a host name's first label, lower case, letters,
/// digits and `-`, at most [`MACHINE_MAX`]; `terminal` when nothing's left.
#[must_use]
pub fn machine_name(host: &str) -> String {
    let name: String = host
        .split('.')
        .next()
        .unwrap_or_default()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(MACHINE_MAX)
        .collect();
    let name = name.trim_matches('-').to_owned();
    if name.is_empty() {
        "terminal".into()
    } else {
        name
    }
}

/// The TypeScript declarations of these types, for the web app.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        GuestTarget::decl(config),
        GuestCredentials::decl(config),
        RenewRequest::decl(config),
        format!("const GUEST_GRANT_LIFETIME = {GRANT_LIFETIME};"),
        format!("const GUEST_MACHINE_MAX = {MACHINE_MAX};"),
        format!("const GUEST_RENEW_PATH = {RENEW_PATH:?};"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_an_organisation_and_its_channel_or_the_chats_address() {
        let acme = |web: Option<&str>| Target {
            org: "acme".into(),
            channel: "#web".into(),
            web: web.map(str::to_owned),
        };
        assert_eq!(parse_target("acme/web"), Ok(acme(None)));
        assert_eq!(parse_target("Acme/#Web"), Ok(acme(None)));
        assert_eq!(
            parse_target("https://zirc.dev/chat?org=acme&channel=%23web&guest"),
            Ok(acme(Some("https://zirc.dev")))
        );
        assert_eq!(
            parse_target("http://localhost:3000/chat/?org=acme&channel=%23web"),
            Ok(acme(Some("http://localhost:3000")))
        );
        assert_eq!(
            parse_target("acme/web/agents").map(|t| t.channel),
            Ok("#web/agents".into())
        );
        for bad in [
            "acme",
            "/web",
            "acme/",
            "ac me/web",
            "acme/we b",
            "https://zirc.dev/x?org=a&channel=b",
        ] {
            assert!(parse_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_chat_url_round_trips() {
        let url = chat_url("https://zirc.dev/", "acme", "#web", "ABCD-1234");
        assert_eq!(
            url,
            "https://zirc.dev/chat?org=acme&channel=%23web&guest&terminal=ABCD-1234"
        );
        let target = parse_target(&url).unwrap();
        assert_eq!(
            (target.org.as_str(), target.channel.as_str()),
            ("acme", "#web")
        );
    }

    #[test]
    fn a_pass_is_a_grant_and_a_token() {
        assert_eq!(
            split_pass(&pass("a.b.c", "d.e.f")),
            Some(("a.b.c", "d.e.f"))
        );
        assert_eq!(split_pass("a.b.c"), None);
        assert_eq!(split_pass("a.b.c d e"), None);
        assert_eq!(split_pass(" d.e.f"), None);
    }

    #[test]
    fn machine_names_are_tame() {
        assert_eq!(machine_name("Mattias-MBP.local"), "mattias-mbp");
        assert_eq!(machine_name("..."), "terminal");
        assert_eq!(machine_name("a b_c!"), "abc");
        assert_eq!(machine_name(&"x".repeat(99)).len(), MACHINE_MAX);
    }

    #[test]
    fn a_grants_claims_are_read_from_its_middle() {
        use base64::Engine as _;
        let claims = Grant {
            sub: "guest-21042".into(),
            room: "#web/guest-21042".into(),
            machine: "mbp".into(),
            key: "k".into(),
            gid: "g".into(),
            exp: 9,
        };
        let middle = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        assert_eq!(grant_claims(&format!("h.{middle}.s")), Ok(claims));
        assert!(grant_claims("nope").is_err());
    }
}
