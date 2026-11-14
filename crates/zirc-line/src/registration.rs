//! Registering with a server — capability negotiation, the server password, SASL and a free
//! nick — as a state machine over lines, without any I/O: [`Registration::opening`] says what
//! to send first, and [`Registration::step`] what to answer each line the server sends,
//! until it says the connection is [`Registered`]. `zirc-irc` drives it over TCP, and the web
//! client over a WebSocket.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::{Line, multiline};

/// The capabilities asked for when a server offers them. `sasl` is added when there are
/// credentials to log in with. `draft/event-playback` has history replay reactions (and
/// joins and parts) as the lines they were; without it, Ergo replays them as its
/// `HistServ`'s words, "bob sent a `TAGMSG`", and reactions are lost on reloading.
/// `draft/message-redaction` lets a client trim a message from history and hears when
/// others do; a server offers it only where it allows that.
pub const DEFAULT_CAPS: &[&str] = &[
    "account-tag",
    "away-notify",
    "batch",
    "draft/chathistory",
    "draft/event-playback",
    "draft/message-redaction",
    "draft/multiline",
    "draft/read-marker",
    "message-tags",
    "server-time",
];

/// How many times a taken nick gets another `_` before giving up.
pub const NICK_ATTEMPTS: usize = 5;
/// SASL sends its payload in chunks of this many base64 bytes.
const SASL_CHUNK: usize = 400;

/// Who to register as.
#[derive(Clone)]
pub struct Identity {
    /// The nick to ask for. A taken one gets `_` appended, a few times.
    pub nick: String,
    /// The username part of `nick!user@host`.
    pub user: String,
    /// The free-text "real name".
    pub realname: String,
    /// The server password, sent as `PASS`, for servers that require one.
    pub server_password: Option<String>,
    /// How to log in with SASL, if at all.
    pub sasl: Option<Sasl>,
    /// Capabilities to ask for when offered. `sasl` is added when there's a login.
    pub caps: Vec<String>,
}

impl Identity {
    /// `nick`, as its own user and real name too, asking for `caps`, without a password or a
    /// login.
    #[must_use]
    pub fn new(nick: impl Into<String>, caps: Vec<String>) -> Self {
        let nick = nick.into();
        Self {
            user: nick.clone(),
            realname: nick.clone(),
            nick,
            server_password: None,
            sasl: None,
            caps,
        }
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("nick", &self.nick)
            .field("user", &self.user)
            .field("realname", &self.realname)
            .field(
                "server_password",
                &self.server_password.as_ref().map(|_| "…"),
            )
            .field("sasl", &self.sasl)
            .field("caps", &self.caps)
            .finish()
    }
}

/// A SASL login.
#[derive(Clone)]
pub enum Sasl {
    /// `PLAIN`: an account and its password.
    Plain {
        /// The account name.
        account: String,
        /// The account's password.
        password: String,
    },
    /// `OAUTHBEARER` (RFC 7628): a bearer token the server checks, as Ergo does with its
    /// `oauth2` introspection, asking the token's issuer. The account is the token's.
    OAuthBearer {
        /// The token.
        token: String,
    },
    /// `IRCV3BEARER` (IRCv3's draft `bearer`): a JWT, which Ergo's `jwt-auth` checks
    /// against its issuer's public key without asking anyone. The account is the token's.
    Jwt {
        /// The token.
        token: String,
    },
}

impl Sasl {
    /// The mechanism's name, as `AUTHENTICATE` takes it.
    #[must_use]
    pub fn mechanism(&self) -> &'static str {
        match self {
            Self::Plain { .. } => "PLAIN",
            Self::OAuthBearer { .. } => "OAUTHBEARER",
            Self::Jwt { .. } => "IRCV3BEARER",
        }
    }

    /// The mechanism's one message to the server, before base64.
    fn payload(&self) -> String {
        match self {
            Self::Plain { account, password } => format!("{account}\0{account}\0{password}"),
            Self::OAuthBearer { token } => format!("n,,\x01auth=Bearer {token}\x01\x01"),
            // No authorization identity, then the token's type and the token.
            Self::Jwt { token } => format!("\0jwt\0{token}"),
        }
    }
}

impl fmt::Debug for Sasl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain { account, .. } => f
                .debug_struct("Plain")
                .field("account", account)
                .field("password", &"…")
                .finish(),
            Self::OAuthBearer { .. } => f.debug_struct("OAuthBearer").field("token", &"…").finish(),
            Self::Jwt { .. } => f.debug_struct("Jwt").field("token", &"…").finish(),
        }
    }
}

/// Why registration failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The server ended the connection.
    #[error("server closed the connection: {0}")]
    Closed(String),
    /// The server refused the server password.
    #[error("server refused the server password")]
    BadPassword,
    /// SASL login failed.
    #[error("SASL login failed: {0}")]
    Sasl(String),
    /// No acceptable nick was free.
    #[error("nick {0:?} is taken or not allowed")]
    Nick(String),
}

/// What to do after a line from the server.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Send these, possibly nothing, and keep reading.
    Reply(Vec<Line>),
    /// Registration is done.
    Registered(Registered),
}

/// A registered connection: what the chat model needs to know about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    /// The nick the server gave us, which may differ from the one asked for.
    pub nick: String,
    /// The capabilities the server enabled.
    pub caps: BTreeSet<String>,
    /// What the server takes in one multiline message, when it enabled `draft/multiline`.
    pub multiline: Option<multiline::Limits>,
}

/// Registration, from the first line sent to `RPL_WELCOME`.
#[derive(Debug)]
pub struct Registration {
    identity: Identity,
    nick: String,
    nick_attempts: usize,
    offered: BTreeSet<String>,
    /// The values of offered capabilities that have one, such as `draft/multiline`'s limits.
    values: BTreeMap<String, String>,
    enabled: BTreeSet<String>,
}

impl Registration {
    /// Registration as `identity`.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self {
            nick: identity.nick.clone(),
            identity,
            nick_attempts: 0,
            offered: BTreeSet::new(),
            values: BTreeMap::new(),
            enabled: BTreeSet::new(),
        }
    }

    /// What the client sends first. `CAP LS` holds registration open until `CAP END`.
    #[must_use]
    pub fn opening(&self) -> Vec<Line> {
        let mut lines = vec![Line::new("CAP", ["LS", "302"])];
        if let Some(password) = &self.identity.server_password {
            lines.push(Line::new("PASS", [password.as_str()]));
        }
        lines.push(Line::new("NICK", [self.nick.as_str()]));
        // A real name can come from anyone, a web page's visitor among them: one line, and
        // not empty, which USER refuses.
        let realname: String = self
            .identity
            .realname
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let realname = match realname.trim() {
            "" => self.nick.as_str(),
            trimmed => trimmed,
        };
        lines.push(Line::new(
            "USER",
            [self.identity.user.as_str(), "0", "*", realname],
        ));
        lines
    }

    /// What to answer `line` from the server.
    ///
    /// # Errors
    ///
    /// When the server refuses us: the connection is no use then, and should be closed.
    pub fn step(&mut self, line: &Line) -> Result<Step, Error> {
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        let reply = |lines: Vec<Line>| Ok(Step::Reply(lines));
        match line.command.as_str() {
            "PING" => reply(vec![Line::new("PONG", line.params.clone())]),
            "CAP" => match param(1) {
                "LS" => {
                    // `CAP * LS * :caps` continues on the next line; the last one has no `*`.
                    let (more, caps) = match param(2) {
                        "*" => (true, param(3)),
                        caps => (false, caps),
                    };
                    self.offered.extend(cap_names(caps));
                    self.values.extend(cap_values(caps));
                    if more {
                        return reply(vec![]);
                    }
                    let wanted: Vec<&str> = self
                        .wanted()
                        .filter(|cap| self.offered.contains(*cap))
                        .collect();
                    if wanted.is_empty() {
                        reply(vec![cap_end()])
                    } else {
                        reply(vec![Line::new("CAP", ["REQ", &wanted.join(" ")])])
                    }
                }
                "ACK" => {
                    self.enabled.extend(cap_names(param(2)));
                    match &self.identity.sasl {
                        Some(sasl) if self.enabled.contains("sasl") => {
                            reply(vec![Line::new("AUTHENTICATE", [sasl.mechanism()])])
                        }
                        _ => reply(vec![cap_end()]),
                    }
                }
                "NAK" => reply(vec![cap_end()]),
                _ => reply(vec![]),
            },
            "AUTHENTICATE" => match (&self.identity.sasl, param(0)) {
                (Some(sasl), "+") => reply(authenticate(&sasl.payload())),
                // An OAUTHBEARER server refusing the token says why, and waits for a lone
                // 0x01 before it fails the login (RFC 7628, 3.2.3).
                (Some(Sasl::OAuthBearer { .. }), _) => reply(authenticate("\x01")),
                _ => reply(vec![Line::new("AUTHENTICATE", ["*"])]),
            },
            // RPL_SASLSUCCESS
            "903" => reply(vec![cap_end()]),
            // ERR_NICKLOCKED, ERR_SASLFAIL, ERR_SASLTOOLONG, ERR_SASLABORTED, ERR_SASLALREADY
            "902" | "904" | "905" | "906" | "907" => {
                Err(Error::Sasl(line.params.last().cloned().unwrap_or_default()))
            }
            // ERR_NICKNAMEINUSE
            "433" if self.nick_attempts < NICK_ATTEMPTS => {
                self.nick_attempts += 1;
                self.nick.push('_');
                reply(vec![Line::new("NICK", [self.nick.as_str()])])
            }
            // ERR_NICKNAMEINUSE once out of attempts, ERR_ERRONEUSNICKNAME
            "433" | "432" => Err(Error::Nick(self.nick.clone())),
            // ERR_PASSWDMISMATCH
            "464" => Err(Error::BadPassword),
            // RPL_WELCOME: the first parameter is the nick we registered as.
            "001" => Ok(Step::Registered(self.registered(param(0)))),
            "ERROR" => Err(Error::Closed(param(0).to_owned())),
            _ => reply(vec![]),
        }
    }

    fn registered(&self, nick: &str) -> Registered {
        let multiline = self
            .enabled
            .contains(multiline::CAP)
            .then(|| self.values.get(multiline::CAP))
            .flatten()
            .and_then(|value| multiline::Limits::parse(value));
        Registered {
            nick: if nick.is_empty() {
                self.nick.clone()
            } else {
                nick.to_owned()
            },
            caps: self.enabled.clone(),
            multiline,
        }
    }

    fn wanted(&self) -> impl Iterator<Item = &str> {
        let sasl = self.identity.sasl.as_ref().map(|_| "sasl");
        self.identity.caps.iter().map(String::as_str).chain(sasl)
    }
}

/// `line` as it can be logged: the server password and SASL payloads left out, and of a
/// message (`PRIVMSG`, `NOTICE`, `TAGMSG`) only who sent it, its command and its target,
/// never its text or tags, which are what people wrote, direct messages' included.
#[must_use]
pub fn loggable(line: &Line) -> String {
    match (line.command.as_str(), line.params.as_slice()) {
        ("PRIVMSG" | "NOTICE" | "TAGMSG", params) => {
            let source = line
                .source
                .as_deref()
                .map_or_else(String::new, |source| format!(":{source} "));
            let target = params.first().map_or("", String::as_str);
            format!("{source}{} {target} …", line.command)
        }
        ("PASS", _) => "PASS …".to_owned(),
        ("AUTHENTICATE", [mechanism])
            if matches!(
                mechanism.as_str(),
                "PLAIN" | "OAUTHBEARER" | "IRCV3BEARER" | "+" | "*"
            ) =>
        {
            line.to_string()
        }
        ("AUTHENTICATE", _) => "AUTHENTICATE …".to_owned(),
        _ => line.to_string(),
    }
}

/// Capability names from a `CAP` list, without values (`sasl=PLAIN` is `sasl`) or
/// modifiers (`-batch` is `batch`).
fn cap_names(list: &str) -> impl Iterator<Item = String> + '_ {
    list.split(' ')
        .filter(|cap| !cap.is_empty())
        .map(|cap| cap.trim_start_matches(['-', '~', '=']))
        .map(|cap| cap.split_once('=').map_or(cap, |(name, _)| name).to_owned())
}

/// Capability values from a `CAP LS` list: `draft/multiline=max-bytes=4096` is
/// (`draft/multiline`, `max-bytes=4096`).
fn cap_values(list: &str) -> impl Iterator<Item = (String, String)> + '_ {
    list.split(' ')
        .filter_map(|cap| cap.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
}

fn cap_end() -> Line {
    Line::new("CAP", ["END"])
}

/// `AUTHENTICATE` lines carrying `payload`, chunked as SASL asks: 400 bytes a line, and a
/// lone `+` after a payload that's an exact multiple of that.
fn authenticate(payload: &str) -> Vec<Line> {
    let payload = BASE64.encode(payload);
    let mut lines: Vec<Line> = payload
        .as_bytes()
        .chunks(SASL_CHUNK)
        .map(|chunk| Line::new("AUTHENTICATE", [String::from_utf8_lossy(chunk)]))
        .collect();
    if payload.len().is_multiple_of(SASL_CHUNK) {
        lines.push(Line::new("AUTHENTICATE", ["+"]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity::new("me", vec!["batch".into(), "message-tags".into()])
    }

    fn plain(account: &str, password: &str) -> Sasl {
        Sasl::Plain {
            account: account.into(),
            password: password.into(),
        }
    }

    fn line(text: &str) -> Line {
        text.parse().unwrap()
    }

    fn sent(step: Result<Step, Error>) -> Vec<String> {
        match step.unwrap() {
            Step::Reply(lines) => lines.iter().map(ToString::to_string).collect(),
            Step::Registered(registered) => panic!("registered early as {}", registered.nick),
        }
    }

    #[test]
    fn leaves_passwords_out_of_logs() {
        let mut identity = identity();
        identity.server_password = Some("gate".into());
        let mut lines = Registration::new(identity).opening();
        lines.extend(authenticate(&plain("me", "secret").payload()));
        lines.push(Line::new("AUTHENTICATE", ["PLAIN"]));
        lines.push(Line::new("AUTHENTICATE", ["OAUTHBEARER"]));
        let token = Sasl::OAuthBearer {
            token: "tok".into(),
        };
        lines.extend(authenticate(&token.payload()));
        let logged: Vec<String> = lines.iter().map(loggable).collect();
        assert!(logged.contains(&"PASS …".to_owned()), "{logged:?}");
        assert!(
            logged.contains(&"AUTHENTICATE PLAIN".to_owned()),
            "{logged:?}"
        );
        assert!(
            logged.contains(&"AUTHENTICATE OAUTHBEARER".to_owned()),
            "{logged:?}"
        );
        let payloads = [
            BASE64.encode("me\0me\0secret"),
            BASE64.encode(token.payload()),
        ];
        for line in &logged {
            assert!(
                !line.contains("gate") && !payloads.iter().any(|p| line.contains(p.as_str())),
                "{line}"
            );
        }
    }

    #[test]
    fn opens_with_cap_ls_then_identifies() {
        let mut identity = identity();
        identity.server_password = Some("gate".into());
        let opening: Vec<String> = Registration::new(identity)
            .opening()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            opening,
            ["CAP LS 302", "PASS gate", "NICK me", "USER me 0 * me"]
        );
    }

    #[test]
    fn a_real_name_stays_one_line() {
        let mut identity = identity();
        identity.realname = "Ada\r\nQUIT :gone".into();
        let opening = Registration::new(identity.clone()).opening();
        assert_eq!(
            opening.last().unwrap().to_string(),
            "USER me 0 * :Ada  QUIT :gone"
        );
        identity.realname = " \n ".into();
        let opening = Registration::new(identity).opening();
        assert_eq!(opening.last().unwrap().to_string(), "USER me 0 * me");
    }

    #[test]
    fn asks_only_for_offered_caps_across_continued_lists() {
        let mut registration = Registration::new(identity());
        assert!(sent(registration.step(&line(":s CAP * LS * :batch sasl=PLAIN"))).is_empty());
        assert_eq!(
            sent(registration.step(&line(":s CAP * LS :message-tags multi-prefix"))),
            ["CAP REQ :batch message-tags"]
        );
        assert_eq!(
            sent(registration.step(&line(":s CAP * ACK :batch message-tags"))),
            ["CAP END"]
        );
        let Step::Registered(registered) = registration.step(&line(":s 001 me :Welcome")).unwrap()
        else {
            panic!("not registered");
        };
        assert_eq!(registered.nick, "me");
        assert_eq!(
            registered.caps,
            BTreeSet::from(["batch".into(), "message-tags".into()])
        );
        assert_eq!(registered.multiline, None);
    }

    #[test]
    fn reads_multiline_limits_from_the_offer() {
        let mut registration = Registration::new(Identity::new(
            "me",
            vec![multiline::CAP.into(), "batch".into()],
        ));
        sent(registration.step(&line(
            ":s CAP * LS :batch draft/multiline=max-bytes=4096,max-lines=24",
        )));
        sent(registration.step(&line(":s CAP * ACK :draft/multiline batch")));
        let Step::Registered(registered) = registration.step(&line(":s 001 me_ :hi")).unwrap()
        else {
            panic!("not registered");
        };
        assert_eq!(registered.nick, "me_");
        assert_eq!(
            registered.multiline,
            multiline::Limits::parse("max-bytes=4096,max-lines=24")
        );
    }

    #[test]
    fn ends_cap_when_nothing_wanted_is_offered() {
        let mut registration = Registration::new(identity());
        assert_eq!(
            sent(registration.step(&line(":s CAP * LS :multi-prefix"))),
            ["CAP END"]
        );
    }

    #[test]
    fn logs_in_with_sasl_plain() {
        let mut identity = Identity::new("me", vec![]);
        identity.sasl = Some(plain("me", "pw"));
        let mut registration = Registration::new(identity.clone());
        assert_eq!(
            sent(registration.step(&line(":s CAP * LS :sasl=PLAIN,EXTERNAL"))),
            ["CAP REQ sasl"]
        );
        assert_eq!(
            sent(registration.step(&line(":s CAP * ACK sasl"))),
            ["AUTHENTICATE PLAIN"]
        );
        assert_eq!(
            sent(registration.step(&line("AUTHENTICATE +"))),
            [format!("AUTHENTICATE {}", BASE64.encode("me\0me\0pw"))]
        );
        assert_eq!(
            sent(registration.step(&line(":s 903 me :SASL authentication successful"))),
            ["CAP END"]
        );
        assert_eq!(
            Registration::new(identity).step(&line(":s 904 * :SASL authentication failed")),
            Err(Error::Sasl("SASL authentication failed".into()))
        );
    }

    #[test]
    fn logs_in_with_a_bearer_token() {
        let mut identity = Identity::new("me", vec![]);
        identity.sasl = Some(Sasl::OAuthBearer {
            token: "eyJ.token".into(),
        });
        let mut registration = Registration::new(identity);
        assert_eq!(
            sent(registration.step(&line(":s CAP * LS :sasl=PLAIN,OAUTHBEARER"))),
            ["CAP REQ sasl"]
        );
        assert_eq!(
            sent(registration.step(&line(":s CAP * ACK sasl"))),
            ["AUTHENTICATE OAUTHBEARER"]
        );
        assert_eq!(
            sent(registration.step(&line("AUTHENTICATE +"))),
            [format!(
                "AUTHENTICATE {}",
                BASE64.encode("n,,\x01auth=Bearer eyJ.token\x01\x01")
            )]
        );
        assert_eq!(
            sent(registration.step(&line(":s 903 me :SASL authentication successful"))),
            ["CAP END"]
        );
    }

    #[test]
    fn logs_in_with_a_jwt() {
        let mut identity = Identity::new("me", vec![]);
        identity.sasl = Some(Sasl::Jwt {
            token: "eyJ.jwt".into(),
        });
        let mut registration = Registration::new(identity);
        sent(registration.step(&line(":s CAP * LS :sasl=PLAIN,IRCV3BEARER")));
        assert_eq!(
            sent(registration.step(&line(":s CAP * ACK sasl"))),
            ["AUTHENTICATE IRCV3BEARER"]
        );
        let payload = BASE64.encode("\0jwt\0eyJ.jwt");
        assert_eq!(
            sent(registration.step(&line("AUTHENTICATE +"))),
            [format!("AUTHENTICATE {payload}")]
        );
        assert_eq!(
            loggable(&Line::new("AUTHENTICATE", [payload])),
            "AUTHENTICATE …"
        );
    }

    #[test]
    fn logs_a_message_without_its_text_or_tags() {
        let dm = line("@+draft/reply=abc;time=2026-11-09T10:00:00Z :ann!a@h PRIVMSG me :my secret");
        assert_eq!(loggable(&dm), ":ann!a@h PRIVMSG me …");
        assert_eq!(
            loggable(&Line::new("NOTICE", ["#ops", "hello there"])),
            "NOTICE #ops …"
        );
        assert_eq!(
            loggable(&line("@+typing=active TAGMSG #ops")),
            "TAGMSG #ops …"
        );
        assert_eq!(loggable(&line("JOIN #ops")), "JOIN #ops");
    }

    #[test]
    fn acknowledges_a_refused_token_so_the_server_fails_the_login() {
        let mut identity = Identity::new("me", vec![]);
        identity.sasl = Some(Sasl::OAuthBearer {
            token: "stale".into(),
        });
        let mut registration = Registration::new(identity);
        let why = BASE64.encode(r#"{"status":"invalid_token"}"#);
        assert_eq!(
            sent(registration.step(&line(&format!("AUTHENTICATE {why}")))),
            ["AUTHENTICATE AQ=="]
        );
    }

    #[test]
    fn chunks_long_sasl_payloads() {
        // 149 + 1 + 149 + 1 + 398 = 698 bytes encode to exactly 932 bytes...
        let lines = authenticate(&plain(&"a".repeat(149), &"p".repeat(398)).payload());
        let sizes: Vec<usize> = lines.iter().map(|line| line.params[0].len()).collect();
        assert_eq!(
            sizes.iter().sum::<usize>(),
            BASE64.encode("x".repeat(698)).len()
        );
        assert!(sizes.iter().all(|&size| size <= SASL_CHUNK));

        // ...while 300 bytes encode to exactly 400, which a lone `+` must follow.
        let lines = authenticate(&plain(&"a".repeat(99), &"p".repeat(100)).payload());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].to_string(), "AUTHENTICATE +");
    }

    #[test]
    fn tries_another_nick_a_few_times() {
        let mut registration = Registration::new(identity());
        for attempt in 1..=NICK_ATTEMPTS {
            let nick = format!("me{}", "_".repeat(attempt));
            assert_eq!(
                sent(registration.step(&line(":s 433 * me :Nickname is already in use"))),
                [format!("NICK {nick}")]
            );
        }
        assert!(matches!(
            registration.step(&line(":s 433 * me :Nickname is already in use")),
            Err(Error::Nick(_))
        ));
    }

    #[test]
    fn answers_pings_and_fails_on_refusals() {
        let mut registration = Registration::new(identity());
        assert_eq!(sent(registration.step(&line("PING :abc"))), ["PONG abc"]);
        assert_eq!(
            registration.step(&line(":s 464 * :Password incorrect")),
            Err(Error::BadPassword)
        );
        assert_eq!(
            registration.step(&line("ERROR :Closing link")),
            Err(Error::Closed("Closing link".into()))
        );
    }

    #[test]
    fn keeps_secrets_out_of_debug_output() {
        let mut identity = identity();
        identity.server_password = Some("gate-secret".into());
        identity.sasl = Some(plain("me", "sasl-secret"));
        let debug = format!("{identity:?}");
        assert!(!debug.contains("secret"), "{debug}");
        let token = Sasl::OAuthBearer {
            token: "token-secret".into(),
        };
        assert!(!format!("{token:?}").contains("secret"));
    }
}
