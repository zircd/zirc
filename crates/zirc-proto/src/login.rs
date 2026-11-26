//! The `zirc login` handshake between a zirc client and the web app at zirc.dev, as
//! `gh auth login` does it. The client starts a login, the person finishes it in the browser,
//! and the client collects what it needs to connect:
//!
//! 1. `POST /api/cli/login` with a [`LoginRequest`] answers a [`LoginStarted`]: the login's
//!    id, a short code, and the URL where the person confirms it.
//! 2. The person opens that URL, signs in, and confirms the code is the one the client shows.
//! 3. `GET /api/cli/login/<id>`, polled every [`LoginStarted::interval`] seconds, answers a
//!    [`LoginStatus`]: pending, until it's done with the [`Credentials`], or denied, or
//!    expired. An unknown id answers 404, which means expired too.
//!
//! A machine whose login the server refuses, as after its owner's handle changed, asks for
//! its current credentials: `POST /api/cli/machine`, authenticated with HTTP Basic as its
//! account and the passphrase it has, answers [`Credentials`], or 401 when that account
//! isn't a machine's any more (revoked), when only a new `zirc login` will do.
//!
//! An organisation's owner's machine reads its server's log, which has the messages people
//! send while the owners have debug logging on: `GET /api/cli/log`, authenticated as above,
//! answers a [`LogPage`], 403 for a machine whose person isn't an owner. Its query: `since`,
//! how far back, such as `30m`, `12h`, `3d`, `2w` or `1mo` (at most the 30 days the log is
//! kept); `q`, text the lines must have; `before`, a time, for the page before one; and
//! `all`, for every line of the log rather than only the messages.
//!
//! The web app's TypeScript types are generated from these: see [`typescript`].

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a login starts, under the web app's address.
pub const START_PATH: &str = "/api/cli/login";

/// Where a machine asks for its current credentials, under the web app's address.
pub const MACHINE_PATH: &str = "/api/cli/machine";

/// Where a person's app asks for its current credentials when the server refuses its
/// login, as a machine does at [`MACHINE_PATH`]: after a handle change elsewhere, the new
/// handle's, with the same passphrase. Asked with the account and passphrase it has.
pub const PERSON_PATH: &str = "/api/cli/person";

/// Where an owner's machine reads the server's log, under the web app's address.
pub const LOG_PATH: &str = "/api/cli/log";

/// Where an owner's or admin's machine makes one of the organisation's CI tokens (POST, a
/// [`CiTokenRequest`], answered a [`CiTokenMade`]) and lists them (GET, a [`CiTokens`]). With
/// such a token an organisation's CI says how a build went: `POST` [`CI_PATH`].
pub const CI_TOKEN_PATH: &str = "/api/cli/ci-token";

/// Where an owner's or admin's machine revokes a CI token by its name (POST, a
/// [`CiTokenRequest`]).
pub const CI_TOKEN_REVOKE_PATH: &str = "/api/cli/ci-token/revoke";

/// Where CI says how a build went, with `Authorization: Bearer <a CI token>`.
pub const CI_PATH: &str = "/ci";

/// What a CI token is called when nobody names it.
pub const CI_TOKEN_NAME: &str = "ci";

/// The CI token to make or revoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CiTokenRequest {
    /// Its name among the organisation's: lowercase letters, digits and dashes.
    pub name: String,
}

/// A CI token, made: the only time the token itself is given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CiTokenMade {
    /// Its name.
    pub name: String,
    /// The token, for CI's secrets.
    pub token: String,
    /// The organisation whose channels it posts to.
    pub organisation: String,
}

/// The organisation's CI tokens, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CiTokens {
    /// Each token still in use, never the token itself.
    pub tokens: Vec<CiTokenListed>,
}

/// One of an organisation's CI tokens, as it's listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CiTokenListed {
    /// Its name.
    pub name: String,
    /// The handle of whoever made it, while they're a member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub by: Option<String>,
    /// When it was made, as RFC 3339.
    pub made: String,
    /// When CI last posted with it, as RFC 3339, if it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub used: Option<String>,
}

/// Where a machine reads the organisation's history, as its owner may see it, from the
/// server's own store: a page of messages, newest first.
pub const HISTORY_PATH: &str = "/api/cli/history";

/// Where a machine reads which conversations had messages lately, as its owner may see them.
pub const CONVERSATIONS_PATH: &str = "/api/cli/conversations";

/// Where a machine or an app asks, with its account, for a short token to read the
/// organisation's history straight from its server, where the server answers content itself
/// (its content API): a [`ContentAccess`]. 404 where it doesn't, and history is read through
/// [`HISTORY_PATH`] and [`CONVERSATIONS_PATH`] as before.
pub const CONTENT_TOKEN_PATH: &str = "/api/cli/content-token";

/// Where on the server's content API the history is, under [`ContentAccess::base`]: the same
/// query and answer as [`HISTORY_PATH`].
pub const CONTENT_HISTORY_PATH: &str = "/history";

/// Where on the server's content API the recent conversations are, as at
/// [`CONVERSATIONS_PATH`].
pub const CONTENT_CONVERSATIONS_PATH: &str = "/conversations";

/// A short token for the organisation's own server, and where to use it: its content API,
/// which checks the person's role and the organisation's visibility itself, so the content
/// never passes through zirc.dev.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ContentAccess {
    /// The content API's address, such as `https://zirc.irc.zirc.dev/api/v1`.
    pub base: String,
    /// The token, for `Authorization: Bearer`: for this server alone, and short-lived.
    pub token: String,
    /// Until when it's good, in milliseconds since the epoch.
    pub expires: u64,
}

/// Where a login's status is polled, under the web app's address.
#[must_use]
pub fn status_path(id: &str) -> String {
    format!("{START_PATH}/{id}")
}

/// What a client sends to start a login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LoginRequest {
    /// The client and its version, such as `zirc 0.1.0`.
    pub client: String,
    /// The machine's name, shown in the browser so the person knows what they're letting in.
    pub host: String,
    /// Whose login it is: a machine's, as `zirc login`'s (when absent), or a person's, as
    /// their own apps sign in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<LoginKind>,
    /// Whether the client can be signed in to several organisations at once: then the web
    /// app lets the person choose several, and answers with [`Credentials::orgs`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub orgs: Option<bool>,
    /// The organisation to sign in to, such as one to add to those the client is signed in
    /// to already: the web app offers it first, if the person is one of its members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub org: Option<String>,
    /// The machine's own random id, kept on it, for a machine's login: the web app knows
    /// it's the same machine as one already let in, whatever its host name says now, and
    /// lets it in again under that name, or renames it, but never adds it twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub machine_id: Option<String>,
    /// The public half of the key the client made on this device ([`crate::device_key`]):
    /// where the server takes device keys, zirc.dev registers it for the account, and says so
    /// with [`Account::device_key`]. Older clients send none, and are never told of one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_key: Option<String>,
    /// For a person's app that takes several organisations ([`Self::orgs`]): the servers it
    /// is signed in to already, by host, such as `zirc.irc.zirc.dev` (empty for the first
    /// sign-in). Sending the field, even empty, says the app takes several: the web app then
    /// lets the person choose some organisations, shows those on these hosts as connected on
    /// this device and not to choose again, and answers with [`Credentials::orgs`]. Together
    /// with the chosen ones an app holds at most [`MAX_ORGANISATIONS`]. Older clients send none
    /// and are given exactly one organisation, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub connected: Option<Vec<String>>,
}

/// The most organisations one app is signed in to at once. zirc.dev enforces it
/// (web/src/lib/logins.ts `MAX_ORGANISATIONS`, a copy: say so at both places).
pub const MAX_ORGANISATIONS: usize = 8;

/// Whose login it is, and so which account it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LoginKind {
    /// A machine, such as one running zircd: it gets an account of its own,
    /// `<handle>-<machine>`, with a passphrase for it alone.
    Machine,
    /// A person, signing in to their own apps: they get their own account, named by their
    /// handle.
    Person,
}

/// A login, started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LoginStarted {
    /// The login's id, secret between the client and the web app: whoever has it collects
    /// the credentials.
    pub id: String,
    /// What the client shows and the browser asks the person to check, such as `WDJB-MJHT`.
    pub code: String,
    /// The page where the person confirms the login.
    pub url: String,
    /// How many seconds to wait between polls.
    pub interval: u32,
    /// How many seconds the login has before it expires.
    pub expires_in: u32,
}

/// Where a login stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "kebab-case")]
#[expect(
    clippy::large_enum_variant,
    reason = "made once per login and read once: a box would only make it awkward to match on"
)]
pub enum LoginStatus {
    /// The person hasn't confirmed it yet.
    Pending,
    /// Confirmed: what to connect with. The web app answers this once, then forgets it.
    Done(Credentials),
    /// The person said no.
    Denied,
    /// Too late: start again.
    Expired,
}

/// What a confirmed login gives the client: the settings it connects with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Credentials {
    /// The organisation's server.
    pub server: Server,
    /// The account to log in to with SASL, if the server has accounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account: Option<Account>,
    /// The nick to use, if the web app has one for the person: the account's name, when
    /// there's an account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nick: Option<String>,
    /// The handle of the person the machine belongs to, which it gives as its IRC real name
    /// so any client can show `mattias-mbp` as mattias's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<String>,
    /// The IRC real name to connect with and to send in `SETNAME` when it changes: the
    /// person's name as their organisation shows it ([`crate::profile::Profile::name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub realname: Option<String>,
    /// The person's account's opaque id, which never changes and names no one: what the
    /// apps' analytics link this install to, so a person counts once across devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account_id: Option<String>,
    /// The organisation's opaque id on the web app, which names no one: what the apps'
    /// analytics group an organisation's events by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub organisation_id: Option<String>,
    /// For a person's app: the zirc.dev browser session that let it in, by its hash, which the
    /// app asks zirc.dev to end when the person signs out of zirc.dev too (`DELETE
    /// /api/session` with [`WebSessionEnd`]). Not a credential: the hash signs nobody in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub web_session: Option<String>,
    /// Where files are shared, when it isn't the web app: a self-hosted organisation's own
    /// server, such as `https://irc.zirc.com`, which keeps them (`zirc_proto::media`'s API at
    /// that address). People's pictures stay at the web app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub media: Option<String>,
    /// Every organisation the person chose, the one above first, when the client asked for
    /// several ([`LoginRequest::orgs`]). Empty means the one above alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<OrgCredentials>>", optional)]
    pub orgs: Vec<OrgCredentials>,
}

/// What a person's app sends, as its own account, to end the zirc.dev browser session that
/// let it in ([`Credentials::web_session`]), so signing out of the app signs out of zirc.dev
/// on the phone too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct WebSessionEnd {
    /// The session, by the hash [`Credentials::web_session`] gave.
    pub session: String,
}

/// One organisation's settings in [`Credentials::orgs`]: as [`Credentials`] gives the first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct OrgCredentials {
    /// The organisation's name, its slug on the web app, which the client knows it by.
    pub name: String,
    /// Its server.
    pub server: Server,
    /// The account on its server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account: Option<Account>,
    /// The nick to use there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nick: Option<String>,
    /// The person's handle there, which may differ from organisation to organisation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<String>,
    /// The IRC real name there, as [`Credentials::realname`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub realname: Option<String>,
    /// Its opaque id, as [`Credentials::organisation_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub organisation_id: Option<String>,
    /// Where files are shared, when it isn't the web app, as [`Credentials::media`]: a self-hosted organisation's own
    /// server, such as `https://irc.zirc.com`, which keeps them (`zirc_proto::media`'s API at
    /// that address). People's pictures stay at the web app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub media: Option<String>,
}

/// An IRC server and how to connect to it. `config.toml` keeps it as `[server]`. Its
/// `Debug` leaves the password out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Server {
    /// Its host name.
    pub host: String,
    /// Its port, 6697 when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub port: Option<u16>,
    /// Whether to use TLS, true when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tls: Option<bool>,
    /// The server password, for servers that require one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub password: Option<String>,
}

/// An account and its password. `config.toml` keeps it as `[account]`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// The account name.
    pub name: String,
    /// Its password.
    pub password: String,
    /// Whether the server has the device's key ([`LoginRequest::device_key`]) for this
    /// account: the client then signs in with the key, which works while zirc.dev is down,
    /// and keeps the password for a server that refuses it. Sent only to a client that sent a
    /// key, as older ones refuse a field they don't know.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub device_key: bool,
}

/// A page of the server's log, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LogPage {
    /// Its lines.
    pub lines: Vec<LogLine>,
    /// The time it looked back to: `since` before now, or the 30 days the log is kept when
    /// `since` asked for more.
    pub from: String,
    /// Whether `since` asked for more than the log keeps.
    #[serde(default)]
    pub capped: bool,
    /// The `before` that asks for the page before this one, when there's more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub older: Option<String>,
}

/// A line of the server's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LogLine {
    /// When, as RFC 3339.
    pub time: String,
    /// The line for people: `alice → #web: hello` for a message, the server's own words for
    /// the rest.
    pub text: String,
    /// The message, for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<LogMessage>,
    /// Whether it's the first line of a start of the server, so a page of the log can mark
    /// it: two starts in a page read like the same lines twice unless marked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub start: bool,
}

/// A message someone sent, from the server's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LogMessage {
    /// Who sent it: their nick.
    pub nick: String,
    /// Where to: a channel, or a nick for a direct message.
    pub to: String,
    /// What kind it is.
    pub kind: MessageKind,
    /// What it says, as people read it: a zirc event's text, not its envelope.
    pub text: String,
}

/// What kind of message a [`LogMessage`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    /// A message (PRIVMSG).
    Message,
    /// A notice (NOTICE).
    Notice,
    /// A channel's new topic (TOPIC).
    Topic,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("name", &self.name)
            .field("password", &"…")
            .field("device_key", &self.device_key)
            .finish()
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("password", &self.password.as_ref().map(|_| "…"))
            .finish()
    }
}

/// The TypeScript declarations of these types, for the web app:
/// `bazel run //crates/zirc-proto:typescript`.
#[must_use]
pub fn typescript() -> String {
    // Sizes and counts, well inside a double.
    let config = ts_rs::Config::new().with_large_int("number");
    let mut out = String::from(
        "// The `zirc login` handshake's, the log's, uploads', cards', names' and a server's desired\n\
         // state's types and rules,\n\
         // generated from\n\
         // zirc-proto (crates/zirc-proto/src/login.rs, media.rs, profile.rs, card.rs,\n\
         // report.rs, role_report.rs, trigger.rs, role.rs, roles.rs, names.rs, desired.rs, onboarding.rs, push.rs, and lib.rs's, request.rs's and session.rs's message types) by `bazel run //web:zirc_proto_types`, which a test checks.\n\
         // Don't edit: change them there and generate this again.\n",
    );
    for decl in [
        LoginRequest::decl(&config),
        LoginKind::decl(&config),
        LoginStarted::decl(&config),
        LoginStatus::decl(&config),
        Credentials::decl(&config),
        OrgCredentials::decl(&config),
        WebSessionEnd::decl(&config),
        Server::decl(&config),
        Account::decl(&config),
        CiTokenRequest::decl(&config),
        CiTokenMade::decl(&config),
        CiTokens::decl(&config),
        CiTokenListed::decl(&config),
        LogPage::decl(&config),
        LogLine::decl(&config),
        LogMessage::decl(&config),
        MessageKind::decl(&config),
        ContentAccess::decl(&config),
    ]
    .into_iter()
    .chain(crate::media::typescript(&config))
    .chain(crate::join::typescript(&config))
    .chain(crate::files::typescript(&config))
    .chain(crate::uploads::typescript(&config))
    .chain(crate::profile::typescript(&config))
    .chain(crate::card::typescript(&config))
    .chain(crate::report::typescript(&config))
    .chain(crate::role_report::typescript(&config))
    .chain(crate::roles::typescript(&config))
    .chain(crate::inbound::typescript(&config))
    .chain(crate::hints::typescript(&config))
    .chain(crate::gotchas::typescript(&config))
    .chain(crate::push::typescript(&config))
    .chain([
        format!("const CI_TOKEN_PATH = {CI_TOKEN_PATH:?};"),
        format!("const CI_TOKEN_REVOKE_PATH = {CI_TOKEN_REVOKE_PATH:?};"),
        format!("const CI_PATH = {CI_PATH:?};"),
        format!("const CI_TOKEN_NAME = {CI_TOKEN_NAME:?};"),
    ])
    .chain(crate::ci::typescript())
    .chain(crate::names::typescript())
    .chain(crate::frame_typescript(&config))
    .chain(crate::irc::typescript(&config))
    .chain(crate::desired::typescript(&config))
    .chain(crate::onboarding::typescript(&config))
    .chain(crate::working::typescript(&config))
    {
        out.push_str("\nexport ");
        out.push_str(&decl);
        out.push('\n');
    }
    out
}

/// A page of the organisation's history from [`HISTORY_PATH`], newest first: what
/// `zirc search` and the apps' search read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HistoryPage {
    /// Whose conversations the organisation lets its people read: `participant`,
    /// `channels` or `everything`.
    pub policy: String,
    /// The messages, newest first.
    pub messages: Vec<HistoryMessage>,
    /// The cursor for the page before this one, when there's more.
    #[serde(default)]
    pub next: Option<String>,
    /// Whether the server's search index answered; an older server, or one still building its
    /// index, read its history instead, and gives no [`HistoryPage::counts`].
    #[serde(default)]
    pub indexed: bool,
    /// When asked for (`counts=1`), how many messages the text finds in each conversation the
    /// asker may read, most first; never one they may not, not even as nothing. `None` when
    /// the server couldn't count, such as for text shorter than three characters.
    #[serde(default)]
    pub counts: Option<Vec<ConversationCount>>,
}

/// How many messages a search finds in one conversation, beside a [`HistoryPage`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConversationCount {
    /// The conversation, as [`HistoryMessage::conversation`] names it.
    pub conversation: String,
    /// How many messages in it match.
    pub count: u64,
}

/// One message from the organisation's history.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HistoryMessage {
    /// The server's id for it.
    pub msgid: String,
    /// When, as RFC 3339.
    pub time: String,
    /// The channel, or the account a direct message is with.
    pub conversation: String,
    /// Who said it.
    pub sender: HistorySender,
    /// As people read it.
    pub text: String,
    /// The zirc event it carries, if it's one, with its session.
    #[serde(default)]
    pub zirc: Option<serde_json::Value>,
}

/// Who said a message in the organisation's history.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HistorySender {
    /// Their account, when they had one.
    #[serde(default)]
    pub account: Option<String>,
    /// The nick they said it as.
    pub nick: String,
    /// `person` or `agent`.
    pub kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_tagged_as_the_web_app_writes_them() {
        let done: LoginStatus = serde_json::from_str(
            r#"{"status":"done","server":{"host":"zirc.irc.zirc.dev","password":"gate"}}"#,
        )
        .unwrap();
        assert_eq!(
            done,
            LoginStatus::Done(Credentials {
                server: Server {
                    host: "zirc.irc.zirc.dev".into(),
                    port: None,
                    tls: None,
                    password: Some("gate".into()),
                },
                account: None,
                nick: None,
                owner: None,
                realname: None,
                account_id: None,
                organisation_id: None,
                web_session: None,
                orgs: Vec::new(),
                media: None,
            })
        );
        // An older CLI's request is a machine's; a person's says so.
        let older: LoginRequest =
            serde_json::from_str(r#"{"client":"zirc 0.1.3","host":"mbp"}"#).unwrap();
        assert_eq!(older.kind, None);
        let person = LoginRequest {
            client: "zirc-desktop 0.1.0".into(),
            host: "mbp".into(),
            kind: Some(LoginKind::Person),
            orgs: None,
            org: None,
            machine_id: None,
            device_key: None,
            connected: None,
        };
        assert!(
            serde_json::to_string(&person)
                .unwrap()
                .ends_with(r#","kind":"person"}"#)
        );
        let pending: LoginStatus = serde_json::from_str(r#"{"status":"pending"}"#).unwrap();
        assert_eq!(pending, LoginStatus::Pending);
        assert_eq!(
            serde_json::to_string(&LoginStatus::Expired).unwrap(),
            r#"{"status":"expired"}"#
        );
    }

    #[test]
    fn a_login_for_several_organisations_lists_each() {
        let done: LoginStatus = serde_json::from_str(
            r#"{"status":"done","server":{"host":"zirc.irc.zirc.dev"},"nick":"m-mbp",
                "orgs":[{"name":"zirc","server":{"host":"zirc.irc.zirc.dev"},"nick":"m-mbp"},
                        {"name":"globex","server":{"host":"globex.irc.zirc.dev"},
                         "account":{"name":"matt-mbp","password":"pw"},"owner":"matt"}]}"#,
        )
        .unwrap();
        let LoginStatus::Done(credentials) = done else {
            panic!("not done")
        };
        let names: Vec<&str> = credentials
            .orgs
            .iter()
            .map(|org| org.name.as_str())
            .collect();
        assert_eq!(names, ["zirc", "globex"]);
        assert_eq!(credentials.orgs[1].owner.as_deref(), Some("matt"));
        // A client that takes several says so, and one that doesn't sends what it did.
        let several = LoginRequest {
            client: "zirc 0.1.17".into(),
            host: "mbp".into(),
            kind: None,
            orgs: Some(true),
            org: Some("globex".into()),
            machine_id: Some("2f1c…".into()),
            device_key: None,
            connected: None,
        };
        assert_eq!(
            serde_json::to_string(&several).unwrap(),
            r#"{"client":"zirc 0.1.17","host":"mbp","orgs":true,"org":"globex","machine_id":"2f1c…"}"#
        );
    }

    #[test]
    fn generates_typescript_for_every_type() {
        let ts = typescript();
        for name in [
            "LoginRequest",
            "LoginKind",
            "LoginStarted",
            "LoginStatus",
            "Credentials",
            "OrgCredentials",
            "WebSessionEnd",
            "Server",
            "Account",
        ] {
            assert!(
                ts.contains(&format!("export type {name} =")),
                "{name}:\n{ts}"
            );
        }
        assert!(ts.contains(r#""status": "pending""#), "{ts}");
        assert!(ts.contains("port?: number"), "{ts}");
    }

    #[test]
    fn keeps_passwords_out_of_debug_output() {
        let credentials = Credentials {
            server: Server {
                host: "zirc.irc.zirc.dev".into(),
                port: None,
                tls: None,
                password: Some("gate-secret".into()),
            },
            account: Some(Account {
                name: "mattias-mbp".into(),
                password: "account-secret".into(),
                device_key: false,
            }),
            nick: None,
            owner: None,
            realname: None,
            account_id: None,
            organisation_id: None,
            web_session: None,
            orgs: Vec::new(),
            media: None,
        };
        let shown = format!("{credentials:?}");
        assert!(shown.contains("zirc.irc.zirc.dev"), "{shown}");
        assert!(!shown.contains("secret"), "{shown}");
    }
}
