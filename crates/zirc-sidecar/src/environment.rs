//! Ergo's environment: the release's own `default.yaml`, changed only through `ERGO__`
//! overrides, for zirc's hosted servers (the controller's pod) and self-hosted ones
//! (`zirc-server`) alike. What changes while Ergo runs is in its config file instead
//! ([`crate::config`]).
//!
//! Three values are secrets, which each runner keeps its own way ([`Secret`]): the controller
//! as Kubernetes secrets, `zirc-server` in a file on its disk.

use serde_json::json;

/// How long a person's always-on account stays on the server with none of their apps
/// connected, as Ergo writes durations.
pub const ALWAYS_ON_EXPIRATION: &str = "90d";

/// Ergo's HTTP API, on loopback only, for the sidecar, unless its runner says otherwise
/// ([`Server::api_listener`]).
pub const API_LISTENER: &str = "127.0.0.1:8089";

/// Ergo's log types whose lines name channels: `channels` (joins, parts, "initializing
/// registered channel"), `services` (`ChanServ`'s "registered channel"), `opers` (the sidecar's
/// SAJOIN and SAMODE, which name the channel). A hosted server's log goes to Google, where
/// nothing about a conversation may go; a self-hosted one's says no more.
const NAMES_CHANNELS: [&str; 3] = ["channels", "services", "opers"];

/// How every server keeps history, in [`ergo`]'s overrides and in tests that need history
/// as servers keep it (`zirc_test_ergo::Ergo::start_with`).
pub const HISTORY: &[(&str, &str)] = &[
    // History survives restarts, in ergo_history.db next to ircd.db. A team's channels
    // aren't registered with ChanServ, so theirs is kept too.
    ("ERGO__DATASTORE__SQLITE__ENABLED", "true"),
    // How long Ergo waits to write a message to its history while something else holds the
    // database: the sidecar's scrub rewrites it whole (crate::trim, VACUUM), which took 15s
    // on zirc's own, and Ergo's 5s let each message said meanwhile go unkept (SQLITE_BUSY:
    // about 40 in the week to 16 Nov 2026). It has delivered the message by then; only
    // whoever sent it waits.
    ("ERGO__DATASTORE__SQLITE__BUSY_TIMEOUT", "2m"),
    ("ERGO__HISTORY__PERSISTENT__ENABLED", "true"),
    ("ERGO__HISTORY__PERSISTENT__UNREGISTERED_CHANNELS", "true"),
    // People remove their own messages, and team ops any in their channels, with REDACT;
    // owners any, through the sidecar's operator (crate::trim). Without this Ergo also
    // stops telling clients what was removed.
    ("ERGO__HISTORY__RETENTION__ALLOW_INDIVIDUAL_DELETE", "true"),
    // Reactions, and taking them back, so a reload shows what's left; and the team's
    // events in a conversation with a guest (crate::gateway::SUPPORT_TAG).
    (
        "ERGO__HISTORY__TAGMSG_STORAGE__WHITELIST",
        r#"["+draft/react", "+react", "+draft/unreact", "+zirc/support"]"#,
    ),
];

/// A value Ergo reads that is a secret, which the runner fills in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secret {
    /// The gate password's bcrypt hash (`server.password`).
    GateHash,
    /// The operator's password's bcrypt hash, which the sidecar logs in with.
    OperHash,
    /// The API's bearer tokens, a JSON list of the one the sidecar presents.
    ApiBearerTokens,
}

/// One override's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// The value itself.
    Plain(String),
    /// A secret, which the runner fills in.
    Secret(Secret),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Plain(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Plain(value)
    }
}

/// What a server's Ergo is told about it.
#[derive(Clone, Debug)]
pub struct Server<'a> {
    /// The network's name: the organisation's.
    pub network: &'a str,
    /// The host name people connect to.
    pub name: &'a str,
    /// Ergo's `server.listeners`, as JSON. The release's own are only changed by it, not
    /// removed: an override merges with `default.yaml`'s map.
    pub listeners: String,
    /// Who may send a PROXY header or `X-Forwarded-For`.
    pub proxy_allowed_from: Vec<String>,
    /// Whether every connection must log in to an account.
    pub require_sasl: bool,
    /// The web pages that may open a WebSocket here.
    pub web_origins: Vec<String>,
    /// The public keys of the tokens the browser client logs in with (SASL IRCV3BEARER).
    pub token_keys: Vec<String>,
    /// The socket the sidecar checks devices' own keys on ([`crate::signin`]), where they're on.
    pub auth_socket: Option<&'a str>,
    /// Where Ergo's HTTP API listens: on loopback, [`API_LISTENER`] unless a runner with
    /// another on the same host (tests) needs its own.
    pub api_listener: &'a str,
    /// Ergo's log at `debug` rather than `info`.
    pub debug: bool,
}

/// Ergo's overrides for `server`, in order.
#[must_use]
pub fn ergo(server: &Server) -> Vec<(&'static str, Value)> {
    let secret = Value::Secret;
    let mut env: Vec<(&'static str, Value)> = vec![
        ("ERGO__NETWORK__NAME", server.network.into()),
        ("ERGO__SERVER__NAME", server.name.into()),
        ("ERGO__SERVER__LISTENERS", server.listeners.clone().into()),
        (
            "ERGO__SERVER__PROXY_ALLOWED_FROM",
            // Spaced as hosted servers' have always been: their Ergo restarts on any change.
            format!(
                "[{}]",
                server
                    .proxy_allowed_from
                    .iter()
                    .map(|from| json!(from).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into(),
        ),
        // The team's gate. Accounts log in with SASL, so PASS mustn't double as an account
        // login, as it does by default.
        ("ERGO__SERVER__PASSWORD", secret(Secret::GateHash)),
        ("ERGO__ACCOUNTS__LOGIN_VIA_PASS_COMMAND", "false".into()),
        // Only the sidecar registers accounts (docs/accounts.md); once every connection has
        // one, the server can require them.
        ("ERGO__ACCOUNTS__REGISTRATION__ENABLED", "false".into()),
        // An account's login stands in for the gate password, so machines and people move to
        // accounts while the gate still keeps everyone else out.
        ("ERGO__ACCOUNTS__SKIP_SERVER_PASSWORD", "true".into()),
        (
            "ERGO__ACCOUNTS__REQUIRE_SASL__ENABLED",
            server.require_sasl.to_string().into(),
        ),
        // People's accounts are always-on (docs/accounts.md); one nobody has connected to
        // for this long leaves the server, so people who've gone don't stay listed.
        (
            "ERGO__ACCOUNTS__MULTICLIENT__ALWAYS_ON_EXPIRATION",
            ALWAYS_ON_EXPIRATION.into(),
        ),
        // The operator the sidecar suspends accounts as.
        ("ERGO__OPERS__ADMIN__PASSWORD", secret(Secret::OperHash)),
        ("ERGO__LANGUAGES__ENABLED", "false".into()),
        // A line on joining, rather than Ergo's "MOTD File is missing": Ergo reads the file
        // its runner writes beside its config.
        ("ERGO__SERVER__MOTD", crate::config::MOTD.into()),
        ("ERGO__SERVER__MOTD_FORMATTING", "false".into()),
        // Only these pages may open a WebSocket here.
        (
            "ERGO__SERVER__WEBSOCKETS__ALLOWED_ORIGINS",
            json!(server.web_origins).to_string().into(),
        ),
    ];
    // Where they've always been in the list: a hosted server's Ergo restarts when it changes.
    env.extend(HISTORY.iter().map(|&(key, value)| (key, value.into())));
    env.extend([
        // The API, on localhost only, for the sidecar.
        ("ERGO__API__ENABLED", "true".into()),
        ("ERGO__API__LISTENER", server.api_listener.into()),
        ("ERGO__API__BEARER_TOKENS", secret(Secret::ApiBearerTokens)),
    ]);
    // Ergo's own push notifications are off: the sidecar's pusher decides what reaches
    // people's phones, by zirc's rules (crate::pusher), and Ergo refuses WEBPUSH REGISTER.
    env.push(("ERGO__WEBPUSH__ENABLED", "false".into()));
    // The browser client logs in with a token the web app signs for an account that's
    // already there (SASL IRCV3BEARER); Ergo checks it with the organisation's public keys
    // alone, any of them, so a new one can come in before the old one goes.
    if !server.token_keys.is_empty() {
        let tokens: serde_json::Value = server
            .token_keys
            .iter()
            .map(|key| json!({"algorithm": "eddsa", "key": key, "account-claims": ["account"]}))
            .collect();
        env.extend([
            ("ERGO__ACCOUNTS__JWT_AUTH__ENABLED", "true".into()),
            // Never an account made by a token: guests come in through the gateway, without one.
            ("ERGO__ACCOUNTS__JWT_AUTH__AUTOCREATE", "false".into()),
            (
                "ERGO__ACCOUNTS__JWT_AUTH__TOKENS",
                tokens.to_string().into(),
            ),
        ]);
    }
    // Devices sign in with keys of their own, which the sidecar checks (crate::signin): Ergo
    // hands each SASL login to it over a socket on the disk they share, a device's token as
    // OAUTHBEARER. Ergo checks passphrases itself when the sidecar says no or doesn't answer.
    // The auth-script block is one value: Ergo can't override its fields one by one, and its
    // default `command` must go, as Ergo takes a command or a socket, not both.
    if let Some(socket) = server.auth_socket {
        let script = json!({
            "enabled": true,
            "command": "",
            "socket": socket,
            "autocreate": false,
            "timeout": "3s",
            "kill-timeout": "1s",
            "max-concurrency": 64,
        });
        env.extend([
            ("ERGO__ACCOUNTS__AUTH_SCRIPT", script.to_string().into()),
            ("ERGO__ACCOUNTS__OAUTH2__ENABLED", "true".into()),
            ("ERGO__ACCOUNTS__OAUTH2__AUTH_SCRIPT", "true".into()),
            ("ERGO__ACCOUNTS__OAUTH2__AUTOCREATE", "false".into()),
        ]);
    }
    let level = if server.debug { "debug" } else { "info" };
    env.push(("ERGO__LOGGING", logging(level).into()));
    env
}

/// Ergo's `logging`, as default.yaml has it, at `level`, without what people say (that stays
/// in the server's history, for who may read it) or the types that name channels.
fn logging(level: &str) -> String {
    let left_out = ["userinput", "useroutput"]
        .iter()
        .chain(&NAMES_CHANNELS)
        .map(|kind| format!(" -{kind}"))
        .collect::<Vec<_>>()
        .concat();
    json!([{"method": "stderr", "type": format!("*{left_out}"), "level": level}]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server<'static> {
        Server {
            network: "acme",
            name: "irc.acme.test",
            listeners: "{}".into(),
            proxy_allowed_from: vec!["localhost".into()],
            require_sasl: true,
            web_origins: vec![],
            token_keys: vec![],
            auth_socket: None,
            api_listener: API_LISTENER,
            debug: false,
        }
    }

    fn value<'e>(env: &'e [(&str, Value)], name: &str) -> Option<&'e Value> {
        env.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    #[test]
    fn ergo_pushes_nothing_itself() {
        let env = ergo(&server());
        assert_eq!(value(&env, "ERGO__WEBPUSH__ENABLED"), Some(&"false".into()));
        assert_eq!(value(&env, "ERGO__WEBPUSH__SUBSCRIBER"), None);
    }

    #[test]
    fn secrets_are_left_to_the_runner() {
        let env = ergo(&server());
        assert_eq!(
            value(&env, "ERGO__SERVER__PASSWORD"),
            Some(&Value::Secret(Secret::GateHash))
        );
        assert_eq!(
            value(&env, "ERGO__API__BEARER_TOKENS"),
            Some(&Value::Secret(Secret::ApiBearerTokens))
        );
    }

    #[test]
    fn the_log_never_has_what_people_say() {
        let env = ergo(&server());
        let Some(Value::Plain(log)) = value(&env, "ERGO__LOGGING") else {
            panic!("no log");
        };
        assert!(log.contains("-userinput -useroutput -channels"));
    }
}
