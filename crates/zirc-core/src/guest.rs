//! This machine as a guest's terminal ([`zirc_proto::guest_terminal`]): `zirc login acme/web`
//! saves an organisation whose [`Organisation::guest`] says how to reach the gateway, and
//! zircd connects with it, renewing its grant before it ends.

use std::sync::Arc;

use zirc_proto::guest_terminal::{self, GuestCredentials, RenewRequest};
use zirc_proto::login::Credentials;

use crate::device_key::{self, DeviceKey};
use crate::settings::Organisation;

/// How long before its grant ends zircd renews it, in seconds: hours ahead, so a machine
/// asleep for a while still has a good one when it wakes.
pub const RENEW_WITHIN: u64 = 6 * 60 * 60;

/// Whether `guest`'s grant is to be renewed at `now`, in seconds since the epoch.
#[must_use]
pub fn renew_due(guest: &GuestCredentials, now: u64) -> bool {
    guest.expires <= now.saturating_add(RENEW_WITHIN)
}

/// A fresh token from `key` that proves this is `guest`'s terminal: for the guest, its `aud`
/// the grant's id. `None` when the grant isn't one or the key can't sign.
#[must_use]
pub fn proof(key: &dyn DeviceKey, guest: &GuestCredentials) -> Option<String> {
    let gid = guest_terminal::grant_claims(&guest.grant).ok()?.gid;
    device_key::token(key, &guest.guest, &gid)
}

/// What renewing `guest`'s grant sends ([`guest_terminal::RENEW_PATH`]), or `None` as for
/// [`proof`].
#[must_use]
pub fn renew_request(key: &dyn DeviceKey, guest: &GuestCredentials) -> Option<RenewRequest> {
    Some(RenewRequest {
        grant: guest.grant.clone(),
        proof: proof(key, guest)?,
    })
}

/// The connection to `guest`'s gateway: over its WebSocket, its first line's password the
/// grant and a token made by `key` for that connection alone. The gateway names the
/// terminal, so the nick asked for is only the guest's.
///
/// # Errors
///
/// When the gateway's address isn't a WebSocket's.
pub fn connection(
    guest: &GuestCredentials,
    key: Arc<dyn DeviceKey>,
) -> Result<zirc_irc::Config, &'static str> {
    const NOT: &str = "a guest gateway's address is a ws:// or wss:// one";
    let (tls, rest) = if let Some(rest) = guest.url.strip_prefix("wss://") {
        (true, rest)
    } else {
        (false, guest.url.strip_prefix("ws://").ok_or(NOT)?)
    };
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            (host, port.parse().map_err(|_| NOT)?)
        }
        _ => (authority, if tls { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(NOT);
    }
    let mut config = zirc_irc::Config::new(host, port, guest.guest.clone());
    config.tls = tls;
    config.websocket = Some(guest.url.clone());
    let credentials = guest.clone();
    config.password_maker = Some(Arc::new(move || {
        let proof = proof(&*key, &credentials)?;
        Some(guest_terminal::pass(&credentials.grant, &proof))
    }));
    Ok(config)
}

/// The organisation a guest login's `credentials` from the web app at `web` save: named by
/// the organisation's slug, without an account. `None` when they aren't a guest's.
#[must_use]
pub fn organisation(credentials: &Credentials, web: &str) -> Option<Organisation> {
    let guest = credentials.guest.clone()?;
    Some(Organisation {
        name: Some(guest.org.clone()),
        nick: None,
        server: credentials.server.clone(),
        account: None,
        owner: None,
        web: Some(web.to_owned()),
        guest: Some(guest),
    })
}

/// Which public channel a guest's terminal is in, as `zirc login` takes it and every line
/// about it says it: `acme/web`.
#[must_use]
pub fn of(guest: &GuestCredentials) -> String {
    format!("{}/{}", guest.org, guest.public.trim_start_matches('#'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest(url: &str, grant: String, expires: u64) -> GuestCredentials {
        GuestCredentials {
            url: url.into(),
            org: "acme".into(),
            guest: "guest-21042".into(),
            channel: "#web/guest-21042".into(),
            public: "#web".into(),
            grant,
            expires,
        }
    }

    fn grant(gid: &str) -> String {
        let claims = format!(
            r##"{{"sub":"guest-21042","room":"#web/guest-21042","machine":"mbp","key":"k","gid":"{gid}","exp":9}}"##
        );
        zirc_test_ergo::jwt::unsigned(&claims)
    }

    #[test]
    fn renews_hours_before_the_grant_ends() {
        let at = |expires| guest("wss://x/guest", String::new(), expires);
        assert!(!renew_due(&at(100_000), 100_000 - RENEW_WITHIN - 1));
        assert!(renew_due(&at(100_000), 100_000 - RENEW_WITHIN));
        assert!(renew_due(&at(100), 1_000));
    }

    #[test]
    fn connects_to_the_gateway_with_its_grant_and_a_fresh_token_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let key: Arc<dyn DeviceKey> =
            Arc::new(device_key::Software::in_file(dir.path().join("device-key")));
        let acme = guest("wss://acme.irc.zirc.dev/guest", grant("g-1"), 9);
        let config = connection(&acme, key.clone()).unwrap();
        assert_eq!(
            (config.host.as_str(), config.port, config.tls),
            ("acme.irc.zirc.dev", 443, true)
        );
        assert_eq!(
            config.websocket.as_deref(),
            Some("wss://acme.irc.zirc.dev/guest")
        );
        assert!(config.sasl.is_none() && config.bearer.is_none());
        let make = config.password_maker.clone().unwrap();
        let (one, two) = (make().unwrap(), make().unwrap());
        assert_ne!(one, two, "a token is used once");
        let (sent, token) = guest_terminal::split_pass(&one).unwrap();
        assert_eq!(sent, acme.grant);
        let claims = zirc_proto::device_key::parse(token).unwrap().claims;
        assert_eq!(
            (claims.sub.as_str(), claims.aud.as_str()),
            ("guest-21042", "g-1")
        );
        assert!(!format!("{config:?}").contains(&acme.grant));

        let renewing = renew_request(&*key, &acme).unwrap();
        assert_eq!(renewing.grant, acme.grant);

        let local = guest("ws://127.0.0.1:8080/guest", grant("g"), 9);
        let config = connection(&local, key.clone()).unwrap();
        assert_eq!(
            (config.host.as_str(), config.port, config.tls),
            ("127.0.0.1", 8080, false)
        );
        assert!(connection(&guest("https://x/guest", grant("g"), 9), key.clone()).is_err());
        // Not a grant: no password, which the gateway refuses.
        let bad = connection(&guest("wss://x/guest", "nope".into(), 9), key).unwrap();
        assert!((bad.password_maker.unwrap())().is_none());
    }

    #[test]
    fn names_its_public_channel_as_zirc_login_takes_it() {
        let guest = guest("wss://x/guest", String::new(), 9);
        assert_eq!(of(&guest), "acme/web");
    }
}
