//! Guests on a team server's public channels (`spec.publicChannels`): visitors from a site's
//! "Chat with us", who reach Ergo only through this gateway in the sidecar. Ergo's own rules
//! are the same for every client, so the gateway is what holds a guest to theirs:
//!
//! - a token zirc.dev signs with the organisation's key names the guest and the one public
//!   channel they may be in, or their room of their own beside one
//!   ([`zirc_proto::guest_room`], `#support/guest-21042`), which the gateway makes secret and,
//!   when nobody from the team is in it, tells the public channel they're waiting in;
//!   Ergo makes no account for them;
//! - the gateway connects to Ergo for them on the pod's loopback, as their nick, and joins
//!   that channel;
//! - of what they send, it passes on only talk in that channel, answers to whoever on the
//!   organisation's staff has written to them (ever: who has is in Ergo's history), reading
//!   METADATA (people's names and pictures) but never setting their own, and the protocol's
//!   housekeeping, at a guest's pace; the rest is refused;
//! - of what's sent to them, it passes on nothing from an agent (a machine's account), and
//!   no private message but from a person on staff or the server's services, so agents never
//!   talk to guests, nor guests to each other; and of the team's `+zirc/support` events about
//!   their conversation, only that someone joined it;
//! - a few connections per address, and so many guests at once per organisation.
//!
//! On a demo server (`spec.demo`, zirc.dev's live demo), whose only agents are the made-up
//! team the sidecar plays (`crate::demo`), a guest hears agents in their channel too, may write
//! privately to the cast and hear its answers, and the gateway tells the cast each time a guest
//! is in, in place of telling staff they're waiting.
//!
//! With `spec.accounts.guests`, each guest also has an always-on account of their own, named as
//! they are, which the gateway makes as they first come (at most [`GUEST_ACCOUNTS_MAX`](crate::limits::GUEST_ACCOUNTS_MAX) an organisation)
//! and logs them in to with SASL, with a passphrase only it knows. Ergo then keeps what staff
//! write to them while they're away, and their conversations, for them to come back to; and
//! a guest whose account is suspended (banned) can't log in, so isn't let in.
//!
//! In a public channel that takes terminals (`spec.terminalChannels`), a guest may also connect
//! a terminal of theirs ([`zirc_proto::guest_terminal`]): zircd, with a grant zirc.dev signed
//! that names the guest, their channel and the device's key, and a fresh token from that key
//! each time. It comes in beside the guest's browser as `<guest>-<machine>` ([`terminal_nick`]),
//! a guest's nick too, into the same channel, under the same rules and at the same pace, and
//! answers whoever the guest may. It logs in to no account, theirs or its own.
//!
//! Apps tell a guest by their nick and account (`guest-…`, a handle no member can have, as
//! zirc.dev reserves `guest` and handles have no `-`, and which only the gateway makes) and
//! their real name, which the gateway sets.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, warn};
use zirc_irc::Line;

/// A guest's real name, which apps don't take for an owner as they would a machine's.
pub const REALNAME: &str = "zirc.dev guest";

/// Connections one address may have at once, and open in a minute.
const AT_ONCE: usize = 3;
const PER_MINUTE: usize = 6;

/// A guest's pace for what others hear ([`is_talk`]): a burst of so many lines, then one every
/// so often; beyond a queue of so many, they're disconnected.
const BURST: u32 = 5;
const EVERY: Duration = Duration::from_secs(2);
const QUEUE: usize = 20;

/// The same for the rest, which their app sends by itself, such as a read marker for each new
/// message, people's names and pictures, and history: its own, quicker pace, so a reaction or
/// a message never waits behind it. Ergo's own flood limit holds both.
const QUIET_BURST: u32 = 20;
const QUIET_EVERY: Duration = Duration::from_millis(200);

/// Whether `line`, one a guest may send ([`allowed`]), is talk others hear: a message, a
/// multiline message's lines, or a reaction; not a typing notice, nor housekeeping.
#[must_use]
pub fn is_talk(line: &Line) -> bool {
    match line.command.to_ascii_uppercase().as_str() {
        "PRIVMSG" | "BATCH" => true,
        "TAGMSG" => line.tags.iter().any(|(tag, _)| {
            tag.starts_with('+') && !matches!(tag.as_str(), "+typing" | "+draft/typing")
        }),
        _ => false,
    }
}

/// One pace of a guest's lines: those waiting, and how many may go now.
struct Lane {
    queue: VecDeque<Line>,
    tokens: u32,
    burst: u32,
}

impl Lane {
    const fn new(burst: u32) -> Self {
        Self {
            queue: VecDeque::new(),
            tokens: burst,
            burst,
        }
    }

    fn refill(&mut self) {
        self.tokens = (self.tokens + 1).min(self.burst);
    }

    /// What may go now, in order.
    fn due(&mut self) -> Vec<Line> {
        let mut due = Vec::new();
        while self.tokens > 0
            && let Some(line) = self.queue.pop_front()
        {
            self.tokens -= 1;
            due.push(line);
        }
        due
    }
}

/// How long a guest has to show their token.
const HELLO: Duration = Duration::from_secs(10);

/// The most one WebSocket message from a guest may be: a line or two, as IRC's lines are under
/// 9 KB with their tags (`zirc_irc`), and clients send one a message. Nobody is known yet when
/// the first arrive, so nothing bigger is ever buffered for them.
const MAX_MESSAGE: usize = 16 * 1024;

/// What a guest may send before their token: a few `CAP` lines, and not many bytes of them.
const MAX_EARLY_LINES: usize = 16;
const MAX_EARLY_BYTES: usize = 32 * 1024;

/// The tag of the team's events in a conversation with a guest (assigned, closed, joined),
/// which zirc's apps fold into it.
pub const SUPPORT_TAG: &str = "+zirc/support";

/// The server's services, whose notices a guest may get.
const SERVICES: &[&str] = &["nickserv", "chanserv", "histserv"];

/// Whether `account` is a person on the organisation's staff: not a guest or an
/// integration, whose names have a `-` that no handle has, not the founder the sidecar
/// registers channels to, and not a machine or a workflow's nick, which are told from
/// people by what the server knows of them ([`crate::machines::is_agent`],
/// [`crate::workflows::is_zircs_own`]), never by their names: a machine can be called
/// `laptop` and a workflow `support`, and their agents don't talk to guests.
#[must_use]
pub fn person(account: &str) -> bool {
    !account.contains('-')
        && account != crate::teams::FOUNDER
        && !crate::machines::is_agent(account)
        && !crate::workflows::is_zircs_own(account)
}

/// What a guest is sent of what comes from Ergo for them (`me`, lower case): everything but
/// private messages, notices and tags from anyone but a person on staff or a service, talk
/// from agents anywhere (but on a `demo` server, whose agents are the sidecar's cast, which
/// talks to them in their channel and privately), and the team's events but "joined".
///
/// A multiline message's opening, `BATCH +<ref> draft/multiline <target>`, is judged as its
/// message is, and the lines inside it go as it does ([`Multiline`]): only the opening has the
/// sender's account.
pub fn delivered(line: &Line, me: &str, demo: bool) -> bool {
    let at = match line.command.as_str() {
        "PRIVMSG" | "NOTICE" | "TAGMSG" => 0,
        "BATCH" if multiline_opening(line).is_some() => 2,
        _ => return true,
    };
    // From the server itself, or a server-wide notice: no user behind it.
    let Some(nick) = line
        .source
        .as_deref()
        .filter(|s| s.contains('!'))
        .and_then(|s| s.split('!').next())
    else {
        return true;
    };
    let nick = nick.to_lowercase();
    if nick == me {
        return true;
    }
    if line
        .tag(SUPPORT_TAG)
        .is_some_and(|event| !event.starts_with("joined"))
    {
        return false;
    }
    let account = line.tag("account").map(str::to_lowercase);
    let agent = account.as_deref().is_some_and(|a| !person(a));
    let to_them = line.params.get(at).is_some_and(|t| t.to_lowercase() == me);
    if to_them {
        account.as_deref().is_some_and(person)
            || SERVICES.contains(&nick.as_str())
            || (demo && account.as_deref().is_some_and(crate::demo::is_cast))
    } else {
        !agent || demo
    }
}

/// A multiline message's opening's reference: `BATCH +<ref> draft/multiline <target>`.
fn multiline_opening(line: &Line) -> Option<&str> {
    let reference = line.params.first()?.strip_prefix('+')?;
    (line.command == "BATCH" && line.params.get(1)? == "draft/multiline").then_some(reference)
}

/// The multiline messages being sent to a guest, by reference, and whether each goes to them:
/// as its opening went, since the lines inside have no account to judge them by.
#[derive(Default)]
pub struct Multiline(HashMap<String, bool>);

impl Multiline {
    /// Whether `line` goes to `me`: as [`delivered`] says, or as its multiline message does.
    pub fn delivered(&mut self, line: &Line, me: &str, demo: bool) -> bool {
        if let Some(reference) = multiline_opening(line) {
            let goes = delivered(line, me, demo);
            // A few at once at most; more is a sender that never closes them.
            if self.0.len() > 64 {
                self.0.clear();
            }
            self.0.insert(reference.to_owned(), goes);
            return goes;
        }
        if line.command == "BATCH"
            && let Some(reference) = line.params.first().and_then(|r| r.strip_prefix('-'))
            && let Some(goes) = self.0.remove(reference)
        {
            return goes;
        }
        match line.tag("batch").and_then(|r| self.0.get(r)) {
            Some(goes) => *goes,
            None => delivered(line, me, demo),
        }
    }
}

/// When the owners are told a visitor is here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alert {
    #[default]
    /// Never.
    None,
    /// When a guest comes into a public channel: zirc's own server, for its support channel.
    Arrivals,
    /// When a guest writes in a public channel: the live demo's server.
    Messages,
}

impl Alert {
    /// Whether nobody is told.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// What the controller tells the sidecar, with the teams: the public channels, the key zirc.dev
/// signs guest tokens with, and how many guests may be on at once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Public {
    #[serde(default)]
    /// The public channels' names.
    pub channels: BTreeSet<String>,
    /// The organisation's public keys, PEM (`ZircServerSpec::token_keys`): a guest's token
    /// signed with any of them is good, as with Ergo's own logins, so zirc.dev can move to a
    /// new key without the server hearing of it first. Checking the first alone once turned
    /// every guest away, when zirc.dev began signing with a key listed second.
    #[serde(default)]
    pub keys: Vec<String>,
    /// The first of them, as sidecars from before `keys` read it: the controller sends both,
    /// as the tenants' sidecar image is pinned apart from its own and may be older. Read only
    /// when `keys` is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Of `channels`, those whose guests may connect a terminal.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub terminals: BTreeSet<String>,
    #[serde(default = "Public::default_cap")]
    /// How many guests may be on at once.
    pub cap: usize,
    /// Guests get accounts (`spec.accounts.guests`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accounts: bool,
    /// The live demo's server (`spec.demo`): its guests hear the cast's agents, and are
    /// kept [`crate::demo::KEPT`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub demo: bool,
    /// The server runs its organisation's workflows (`spec.workflows`, `crate::workflows`):
    /// off unless zirc.dev turns them on, so no other organisation sees their host.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workflows: bool,
    /// What the owners are told, by a private message (so a push; `crate::visitors`).
    #[serde(default, skip_serializing_if = "Alert::is_none")]
    pub alert: Alert,
}

impl Public {
    /// Guests at once on any plan, for now.
    pub const CAP: usize = crate::limits::GUESTS_AT_ONCE;

    const fn default_cap() -> usize {
        Self::CAP
    }

    /// Whether there are no public channels.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }
}

impl Default for Public {
    fn default() -> Self {
        Self {
            channels: BTreeSet::new(),
            terminals: BTreeSet::new(),
            keys: Vec::new(),
            key: None,
            cap: Self::CAP,
            accounts: false,
            demo: false,
            workflows: false,
            alert: Alert::None,
        }
    }
}

/// A guest, as their token says.
#[derive(Debug, PartialEq, Eq)]
pub struct Token {
    /// The guest's name.
    pub guest: String,
    /// The public channel the token admits them to.
    pub channel: String,
    /// Whether they're signed in to zirc.dev, if it says, which the gateway tells everyone
    /// (METADATA [`SIGNED_IN`]).
    pub signed_in: Option<bool>,
    /// The name they typed, if it's one zirc takes ([`zirc_proto::profile::name`]): their real
    /// name, in place of [`REALNAME`]. Their nick stays theirs, `guest-…`.
    pub name: Option<String>,
    /// For a guest's terminal: its machine's name, from its grant.
    pub machine: Option<String>,
}

/// The METADATA key on a guest that says whether they're signed in to zirc.dev, "1" or "0".
pub const SIGNED_IN: &str = "zirc.dev/signed-in";

/// The raw Ed25519 key in a PEM `PUBLIC KEY` (`SubjectPublicKeyInfo`).
#[must_use]
pub fn public_key(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .concat();
    let der = STANDARD.decode(body.trim()).ok()?;
    // 12 bytes of algorithm identifier, then the 32-byte key.
    (der.len() == 44).then(|| der[12..].to_vec())
}

/// A guest token's claims, if `key` signed it, it's in date at `now` (seconds since the
/// epoch), and it's a guest's: a `guest-…` name, one channel, and never an `account` claim,
/// which Ergo would take as a login.
///
/// # Errors
///
/// Why it isn't one.
pub fn verify(token: &str, key: &[u8], now: u64) -> Result<Token, &'static str> {
    #[derive(Deserialize)]
    struct Claims {
        guest: String,
        channel: String,
        exp: u64,
        account: Option<serde_json::Value>,
        #[serde(rename = "signedIn", default)]
        signed_in: Option<bool>,
        #[serde(default)]
        name: Option<String>,
    }
    let claims: Claims = serde_json::from_slice(&signed(token, key)?).map_err(|_| "not a token")?;
    if claims.account.is_some() {
        return Err("not a guest's token");
    }
    if claims.exp <= now {
        return Err("expired");
    }
    if !zirc_proto::names::is_guest_account(&claims.guest) {
        return Err("not a guest's name");
    }
    Ok(Token {
        guest: claims.guest,
        channel: claims.channel.to_lowercase(),
        signed_in: claims.signed_in,
        name: claims
            .name
            .as_deref()
            .and_then(zirc_proto::profile::name)
            .map(str::to_owned),
        machine: None,
    })
}

/// A JWT's claims, as bytes, if `key` signed it with `EdDSA`.
fn signed(token: &str, key: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut parts = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("not a token");
    };
    let header: serde_json::Value = URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|h| serde_json::from_slice(&h).ok())
        .ok_or("not a token")?;
    if header["alg"] != "EdDSA" {
        return Err("not a token");
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "not a token")?;
    let signed = &token[..token.len() - token.rsplit('.').next().unwrap_or_default().len() - 1];
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(signed.as_bytes(), &signature)
        .map_err(|_| "not signed for this server")?;
    URL_SAFE_NO_PAD.decode(claims).map_err(|_| "not a token")
}

/// A terminal, as its first line's password ([`zirc_proto::guest_terminal::pass`]) says: its
/// grant, if one of `keys` signed it and it's in date at `now`, and the connection's token,
/// if the key the grant names made it, for this grant, for its guest. Also what keeps that
/// token from being used again: its key and `jti`, and until when.
///
/// # Errors
///
/// Why it isn't one.
pub fn verify_terminal(
    pass: &str,
    keys: &[Vec<u8>],
    now: u64,
) -> Result<(Token, String, u64), &'static str> {
    use zirc_proto::{device_key, guest_terminal};
    let (grant, proof) = guest_terminal::split_pass(pass).ok_or("not a terminal's grant")?;
    let mut why = "this server has no public channels";
    let mut claims = None;
    for (i, key) in keys.iter().enumerate() {
        match signed(grant, key) {
            Ok(bytes) => {
                claims = Some(bytes);
                break;
            }
            Err(reason) if i == 0 => why = reason,
            Err(_) => {}
        }
    }
    let claims = claims.ok_or(why)?;
    let raw: serde_json::Value = serde_json::from_slice(&claims).map_err(|_| "not a grant")?;
    if raw.get("account").is_some() {
        return Err("not a guest's grant");
    }
    let grant: guest_terminal::Grant = serde_json::from_value(raw).map_err(|_| "not a grant")?;
    if grant.exp <= now {
        return Err("expired");
    }
    if !zirc_proto::names::is_guest_account(&grant.sub) {
        return Err("not a guest's name");
    }
    if grant.machine.is_empty() || grant.machine != guest_terminal::machine_name(&grant.machine) {
        return Err("not a machine's name");
    }
    let parsed = device_key::parse(proof)?;
    if parsed.kid != grant.key || parsed.claims.sub != grant.sub {
        return Err("not this terminal's key");
    }
    parsed.claims.check(&grant.gid, now)?;
    let point = device_key::point(&parsed.kid).ok_or("not a key")?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, point)
        .verify(parsed.signing_input.as_bytes(), &parsed.signature)
        .map_err(|_| "not signed by that key")?;
    let once = format!("{} {}", parsed.kid, parsed.claims.jti);
    Ok((
        Token {
            guest: grant.sub,
            channel: grant.room.to_lowercase(),
            signed_in: None,
            name: None,
            machine: Some(grant.machine),
        },
        once,
        parsed.claims.exp,
    ))
}

/// [`verify`] with whichever of `keys` signed it.
///
/// # Errors
///
/// Why it isn't a good token for any of them: the first key's reason, or that there's none.
pub fn verify_any(token: &str, keys: &[Vec<u8>], now: u64) -> Result<Token, &'static str> {
    let mut why = "this server has no public channels";
    for (i, key) in keys.iter().enumerate() {
        match verify(token, key, now) {
            Ok(token) => return Ok(token),
            Err(reason) if i == 0 => why = reason,
            Err(_) => {}
        }
    }
    Err(why)
}

/// Whether a token for `guest` may name `channel`: one of the `public` channels, or the guest's
/// own room beside one ([`zirc_proto::guest_room_of`]), never another guest's.
#[must_use]
pub fn admits(public: &BTreeSet<String>, guest: &str, channel: &str) -> bool {
    public.contains(channel)
        || zirc_proto::guest_room_of(channel).is_some_and(|(parent, owner)| {
            public.contains(parent) && owner.eq_ignore_ascii_case(guest)
        })
}

/// What the team is told in the public channel when a guest comes to their room and nobody
/// from the team is in it: `Sam (guest-21042) is waiting in #support/guest-21042`.
#[must_use]
pub fn waiting(guest: &str, name: Option<&str>, room: &str) -> String {
    match name {
        Some(name) => format!("{name} ({guest}) is waiting in {room}"),
        None => format!("{guest} is waiting in {room}"),
    }
}

/// Whether a guest in `channel` may send `line` once they're in: talk there, answers to
/// those who wrote to them (`repliers`, lower case), its history, reading people's names and
/// pictures, and housekeeping.
pub fn allowed<S: std::hash::BuildHasher>(
    line: &Line,
    channel: &str,
    repliers: &HashSet<String, S>,
) -> bool {
    let target = |i: usize| line.params.get(i).map(|t| t.to_lowercase());
    let theirs = |t: Option<String>| t.is_some_and(|t| t == channel || repliers.contains(&t));
    match line.command.to_ascii_uppercase().as_str() {
        "PING" | "PONG" | "QUIT" | "AWAY" | "CAP" => true,
        // Reading names and pictures (`crate::profiles`), never setting their own, which could
        // make them look like staff.
        "METADATA" => line.params.get(1).is_some_and(|sub| {
            matches!(
                sub.to_ascii_lowercase().as_str(),
                "get" | "list" | "sub" | "unsub" | "subs" | "sync"
            )
        }),
        "JOIN" | "PART" | "MARKREAD" | "WHO" => target(0).as_deref() == Some(channel),
        "PRIVMSG" | "TAGMSG" => theirs(target(0)),
        // A multiline message's opening names its target; its close is `-<ref>`.
        "BATCH" => line.params.first().is_some_and(|r| r.starts_with('-')) || theirs(target(2)),
        "CHATHISTORY" => {
            line.params
                .first()
                .is_some_and(|s| s.eq_ignore_ascii_case("TARGETS"))
                || theirs(target(1))
        }
        _ => false,
    }
}

/// The guests on now, by name and by address.
#[derive(Default)]
struct Guests {
    /// Each guest's connection, to end when the same guest connects again: by their name, or
    /// for a terminal of theirs by [`connection`].
    by_name: HashMap<String, (u64, tokio::sync::watch::Sender<bool>)>,
    by_address: HashMap<IpAddr, Address>,
    next: u64,
}

#[derive(Default)]
struct Address {
    connected: usize,
    opened: VecDeque<Instant>,
}

/// The gateway: what the controller said, and who's on.
pub struct Gateway {
    public: Mutex<Public>,
    /// Whether the server runs workflows, as `public` says, for whoever waits on a change.
    workflows: tokio::sync::watch::Sender<bool>,
    guests: Mutex<Guests>,
    /// Ergo's loopback listener, and its gate password if it has one.
    ergo: SocketAddr,
    gate: Option<String>,
    /// Ergo's history, where who has written to each guest is.
    history: Option<PathBuf>,
    /// Who on staff each guest may answer, by guest, as learned since the sidecar started.
    repliers: Mutex<HashMap<String, HashSet<String>>>,
    /// When each guest was last connected, and to which channel, since the sidecar started.
    seen: Mutex<HashMap<String, (jiff::Timestamp, String)>>,
    /// Ergo's API, to make guests' accounts with, and the secret their passphrases come from.
    accounts: Option<(crate::ergo::Api, String)>,
    /// On a demo server, where to say a guest's room is ready (`crate::demo`).
    arrivals: Mutex<Option<tokio::sync::mpsc::UnboundedSender<crate::demo::Arrival>>>,
    /// The owners' handles (the accounts to tell a visitor is here), and what's waiting to be
    /// told them.
    owners: Mutex<Vec<String>>,
    visitors: Mutex<crate::visitors::Visitors>,
    /// Held while pushes are sent, which all come from one nick.
    pushing: tokio::sync::Mutex<()>,
    /// Terminals' tokens used, by key and `jti`, until they expire: each is good once.
    proofs: Mutex<HashMap<String, u64>>,
}

/// What a guest's connection is known by: their name, or for a terminal, their name and its
/// machine's, so a terminal comes in beside the browser and each ends only its own older one.
fn connection(guest: &str, machine: Option<&str>) -> String {
    machine.map_or_else(|| guest.to_owned(), |m| format!("{guest} {m}"))
}

/// The guest a [`connection`] is.
fn guest_of(connection: &str) -> &str {
    connection.split(' ').next().unwrap_or(connection)
}

/// A guest's terminal's nick: theirs and its machine's, `guest-21042-mbp`, within a nick's
/// length ([`zirc_proto::names::NICK_MAX`]).
#[must_use]
pub fn terminal_nick(guest: &str, machine: &str) -> String {
    let room = zirc_proto::names::NICK_MAX.saturating_sub(guest.len() + 1);
    let machine: String = machine.chars().take(room).collect();
    format!("{guest}-{}", machine.trim_end_matches('-'))
}

/// A guest's passphrase: derived from the sidecar's secret, so it's never stored, and the same
/// each time.
fn passphrase(secret: &str, guest: &str) -> String {
    crate::hmac_hex(secret, format!("guest:{guest}").as_bytes())
}

/// A guest let in, until they leave.
struct Admitted {
    gateway: Arc<Gateway>,
    /// Their [`connection`].
    guest: String,
    address: IpAddr,
    id: u64,
    ended: tokio::sync::watch::Receiver<bool>,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        let mut guests = lock(&self.gateway.guests);
        if guests
            .by_name
            .get(&self.guest)
            .is_some_and(|(id, _)| *id == self.id)
        {
            guests.by_name.remove(&self.guest);
        }
        if let Some(address) = guests.by_address.get_mut(&self.address) {
            address.connected = address.connected.saturating_sub(1);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The accounts with a private conversation with `nick` in Ergo's history (`conversations`:
/// a row under each account, with the other as its correspondent).
fn correspondents(history: &std::path::Path, nick: &str) -> rusqlite::Result<HashSet<String>> {
    let db =
        rusqlite::Connection::open_with_flags(history, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut rows =
        db.prepare("SELECT DISTINCT target FROM conversations WHERE correspondent = ?1")?;
    rows.query_map([nick], |row| row.get::<_, String>(0))?
        .collect()
}

impl Gateway {
    /// A gateway to the Ergo at `ergo`, with its gate password, the history database to find guests' conversations in, and the account API for guests' accounts, if guests have any.
    #[must_use]
    pub fn new(
        ergo: SocketAddr,
        gate: Option<String>,
        history: Option<PathBuf>,
        accounts: Option<(crate::ergo::Api, String)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            public: Mutex::new(Public::default()),
            workflows: tokio::sync::watch::Sender::new(false),
            guests: Mutex::new(Guests::default()),
            ergo,
            gate,
            history,
            repliers: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            accounts,
            arrivals: Mutex::new(None),
            owners: Mutex::new(Vec::new()),
            visitors: Mutex::new(crate::visitors::Visitors::new()),
            pushing: tokio::sync::Mutex::new(()),
            proofs: Mutex::new(HashMap::new()),
        })
    }

    /// Where to say, on a demo server, that a guest's room is ready.
    pub fn on_arrival(&self, to: tokio::sync::mpsc::UnboundedSender<crate::demo::Arrival>) {
        *lock(&self.arrivals) = Some(to);
    }

    /// Whether this is the live demo's server.
    pub fn is_demo(&self) -> bool {
        lock(&self.public).demo
    }

    /// Whether the server runs its organisation's workflows, as the controller says, now and
    /// each time it changes.
    pub fn runs_workflows(&self) -> tokio::sync::watch::Receiver<bool> {
        self.workflows.subscribe()
    }

    /// `guest`'s passphrase, once their always-on account is there: made now if it isn't, up
    /// to [`crate::limits::GUEST_ACCOUNTS_MAX`]. `None` while guests have no accounts.
    async fn account(&self, guest: &str) -> Result<Option<String>, &'static str> {
        #[derive(Deserialize)]
        struct Listed {
            accounts: Vec<Named>,
        }
        #[derive(Deserialize)]
        struct Named {
            #[serde(rename = "accountName")]
            name: String,
        }
        #[derive(Deserialize)]
        struct Outcome {
            success: bool,
        }
        let Some((api, secret)) = self
            .accounts
            .as_ref()
            .filter(|_| lock(&self.public).accounts)
        else {
            return Ok(None);
        };
        let unavailable = "we can't let you in right now; try again shortly";
        let failed = |why: &dyn std::fmt::Display| {
            warn!(guest, %why, "couldn't make a guest's account");
            unavailable
        };
        let listed: Listed = api
            .call("ns/list", serde_json::json!({}))
            .await
            .map_err(|e| failed(&e))?;
        let names: Vec<String> = listed
            .accounts
            .into_iter()
            .map(|a| a.name.to_lowercase())
            .collect();
        let pass = passphrase(secret, guest);
        if !names.iter().any(|n| n == guest) {
            if names
                .iter()
                .filter(|n| zirc_proto::named_as_guest(n))
                .count()
                >= crate::limits::GUEST_ACCOUNTS_MAX
            {
                return Err("this organisation has too many guests; try again later");
            }
            let made: Outcome = api
                .call(
                    "ns/saregister",
                    serde_json::json!({"accountName": guest, "passphrase": pass}),
                )
                .await
                .map_err(|e| failed(&e))?;
            if !made.success {
                return Err(failed(&"Ergo didn't register it"));
            }
            // Kept on while they're away, so what staff write reaches them.
            match crate::accounts::set_always_on(api, guest).await {
                Ok(None) => {}
                Ok(Some(why)) => return Err(failed(&why)),
                Err(e) => return Err(failed(&e)),
            }
            info!(guest, "a guest's account made");
        }
        Ok(Some(pass))
    }

    /// The guests connected now, and when each guest was last there, with their channel, as
    /// far as this sidecar has seen.
    pub fn presence(&self) -> (BTreeSet<String>, HashMap<String, (jiff::Timestamp, String)>) {
        let connected = lock(&self.guests)
            .by_name
            .keys()
            .map(|c| guest_of(c).to_owned())
            .collect();
        (connected, lock(&self.seen).clone())
    }

    /// Forgets `guest`: ends their connection, if they're on, and drops what the gateway
    /// remembers of them.
    pub fn forget(&self, guest: &str) {
        for (connection, (_, end)) in &lock(&self.guests).by_name {
            if guest_of(connection) == guest {
                let _ = end.send(true);
            }
        }
        lock(&self.seen).remove(guest);
        lock(&self.repliers).remove(guest);
    }

    /// `guest` is here, in `channel`.
    fn note(&self, guest: &str, channel: &str) {
        lock(&self.seen).insert(
            guest.to_owned(),
            (jiff::Timestamp::now(), channel.to_owned()),
        );
    }

    /// Who on staff `guest` may answer: everyone who has written to them, which Ergo keeps
    /// under each staff member's account with the guest as its correspondent, and anyone since.
    async fn repliers(&self, guest: &str) -> HashSet<String> {
        let mut found = lock(&self.repliers).get(guest).cloned().unwrap_or_default();
        if let Some(history) = self.history.clone() {
            let who = guest.to_lowercase();
            let read = tokio::task::spawn_blocking(move || correspondents(&history, &who)).await;
            match read {
                Ok(Ok(accounts)) => found.extend(accounts.into_iter().filter(|a| person(a))),
                Ok(Err(error)) => warn!(%error, "couldn't read who wrote to a guest"),
                Err(error) => warn!(%error, "couldn't read who wrote to a guest"),
            }
        }
        found
    }

    /// `replier` may now be answered by `guest`, from any of their connections.
    fn may_answer(&self, guest: &str, replier: &str) {
        lock(&self.repliers)
            .entry(guest.to_owned())
            .or_default()
            .insert(replier.to_owned());
    }

    /// What the controller last said.
    pub fn set(&self, public: Public) {
        self.workflows.send_if_modified(|on| {
            let changed = *on != public.workflows;
            *on = public.workflows;
            changed
        });
        *lock(&self.public) = public;
    }

    /// The owners to tell a visitor is here.
    pub fn set_owners(&self, owners: &[String]) {
        *lock(&self.owners) = owners.iter().map(|o| o.to_lowercase()).collect();
    }

    /// `guest` came into `place`, a public channel: tells the owners, if this server does and
    /// it's time to.
    fn visitor_arrived(self: &Arc<Self>, guest: &str, name: Option<&str>, place: &str, room: &str) {
        if lock(&self.public).alert != Alert::Arrivals {
            return;
        }
        let who = name.map_or_else(|| guest.to_owned(), |n| format!("{guest} ({n})"));
        let text = lock(&self.visitors).arrived(&who, place, room, Instant::now());
        if let Some(text) = text {
            self.push(text);
        }
    }

    /// `guest` wrote `text` in `place`, a public channel.
    fn visitor_said(self: &Arc<Self>, guest: &str, text: &str, place: &str, room: &str) {
        if lock(&self.public).alert != Alert::Messages {
            return;
        }
        let text = lock(&self.visitors).said(guest, text, place, room, Instant::now());
        if let Some(text) = text {
            self.push(text);
        }
    }

    /// Tells the owners what has waited long enough; called every so often.
    pub fn push_due(self: &Arc<Self>) {
        let place = self
            .public_channels()
            .into_iter()
            .next()
            .unwrap_or_default();
        let texts = lock(&self.visitors).due(&place, Instant::now());
        for text in texts {
            self.push(text);
        }
    }

    /// Ergo's operator, as the gateway logs in to act for the server: when it has the password.
    fn operator(&self) -> Option<crate::ergo::Operator> {
        let (_, password) = self.accounts.as_ref()?;
        Some(crate::ergo::Operator {
            host: self.ergo.ip().to_string(),
            port: self.ergo.port(),
            server_password: self.gate.clone(),
            password: password.clone(),
        })
    }

    /// Says on `nick` whether the guest is signed in to zirc.dev (METADATA [`SIGNED_IN`]), as
    /// the operator: only the operator sets anyone's metadata (`crate::environment`), so no
    /// client can say what zirc.dev didn't. Done before the guest says anything, so staff who
    /// look when they first hear from them see it.
    async fn tell_signed_in(&self, nick: &str, signed_in: bool) {
        let Some(operator) = self.operator() else {
            return;
        };
        let value = if signed_in { "1" } else { "0" };
        let keys = [(nick.to_owned(), vec![(SIGNED_IN, Some(value))])];
        if let Err(failure) = operator.set_metadata(&keys).await {
            warn!(guest = nick, %failure, "couldn't say whether a guest is signed in");
        }
    }

    /// Sends `text` to each owner as a private message, in the background. Never logged.
    fn push(self: &Arc<Self>, text: String) {
        let owners = lock(&self.owners).clone();
        let Some(operator) = self.operator() else {
            return;
        };
        if owners.is_empty() {
            return;
        }
        let gateway = self.clone();
        tokio::spawn(async move {
            let _one_at_a_time = gateway.pushing.lock().await;
            if let Err(failure) = operator.direct(crate::visitors::FROM, &owners, &text).await {
                warn!(%failure, "couldn't tell the owners a visitor is here");
            }
        });
    }

    /// The public channels.
    pub fn public_channels(&self) -> Vec<String> {
        lock(&self.public).channels.iter().cloned().collect()
    }

    /// On a demo server, tells the cast `guest` is in `channel` (`crate::demo`).
    fn arrived(&self, channel: &str, guest: &str) {
        if self.is_demo()
            && let Some(to) = lock(&self.arrivals).as_ref()
        {
            let _ = to.send(crate::demo::Arrival {
                room: channel.to_owned(),
                guest: guest.to_owned(),
            });
        }
    }

    /// Makes `room` secret, so it's in no channel list, and, when nobody else is in it,
    /// tells `parent` that `guest` is waiting there, from zirc's own [`crate::visitors::FROM`]
    /// (apps show it as System), one at a time as its pushes are.
    fn set_up_room(
        self: &Arc<Self>,
        room: &str,
        parent: &str,
        guest: &str,
        name: Option<&str>,
        others_here: bool,
    ) {
        let Some(operator) = self.operator() else {
            return;
        };
        let (room, parent) = (room.to_owned(), parent.to_owned());
        // On a demo server too, where its owners look in on visitors (the user's decision,
        // 8 Nov 2026): the cast comes in after this, so the room is empty now.
        let text = (!others_here).then(|| waiting(guest, name, &room));
        // Logged by guest, never by room: a room is a channel, and logs go to Google.
        let guest = guest.to_owned();
        let gateway = self.clone();
        tokio::spawn(async move {
            if let Err(failure) = operator.modes(&room, "+s").await {
                warn!(guest, %failure, "couldn't make a guest's room secret");
            }
            let _one_at_a_time = gateway.pushing.lock().await;
            if let Some(text) = text
                && let Err(failure) = operator
                    .notice_as(crate::visitors::FROM, &parent, &text)
                    .await
            {
                warn!(guest, %failure, "couldn't say a guest is waiting");
            }
        });
    }

    /// Whether a terminal's token, `once` (its key and `jti`), good until `exp`, is new at
    /// `now`: it's then used.
    fn first_use(&self, once: String, exp: u64, now: u64) -> bool {
        let mut used = lock(&self.proofs);
        used.retain(|_, until| *until > now);
        used.insert(once, exp).is_none()
    }

    /// Whether the organisation has guests: public channels they may come into.
    pub fn has_guests(&self) -> bool {
        !lock(&self.public).channels.is_empty()
    }

    /// Lets `guest` in from `address` at `now`, ending their connection elsewhere (from
    /// `machine`, for a terminal's: its own older one); or why not.
    fn admit(
        self: &Arc<Self>,
        guest: &str,
        machine: Option<&str>,
        address: IpAddr,
        now: Instant,
    ) -> Result<Admitted, &'static str> {
        let cap = lock(&self.public).cap;
        let mut guests = lock(&self.guests);
        let on: HashSet<&str> = guests.by_name.keys().map(|c| guest_of(c)).collect();
        if !on.contains(guest) && on.len() >= cap {
            return Err("this room is full; try again shortly");
        }
        let at = guests.by_address.entry(address).or_default();
        while at
            .opened
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_mins(1))
        {
            at.opened.pop_front();
        }
        if at.connected >= AT_ONCE || at.opened.len() >= PER_MINUTE {
            return Err("too many connections from your address; try again shortly");
        }
        at.connected += 1;
        at.opened.push_back(now);
        guests.next += 1;
        let id = guests.next;
        let (end, ended) = tokio::sync::watch::channel(false);
        let known_as = connection(guest, machine);
        if let Some((_, older)) = guests.by_name.insert(known_as.clone(), (id, end)) {
            let _ = older.send(true);
        }
        Ok(Admitted {
            gateway: self.clone(),
            guest: known_as,
            address,
            id,
            ended,
        })
    }
}

/// `GET /guest`, the guests' WebSocket.
pub fn router(gateway: Arc<Gateway>) -> Router {
    Router::new()
        .route("/guest", get(upgrade))
        .with_state(gateway)
}

async fn upgrade(
    State(gateway): State<Arc<Gateway>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // The visitor's own address: the first in X-Forwarded-For, as the edge's Envoy writes it.
    let address = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|a| a.trim().parse().ok())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    ws.protocols(["text.ircv3.net", "binary.ircv3.net"])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            if let Err(why) = session(socket, gateway, address).await {
                info!(%why, "a guest left");
            }
        })
}

/// A line from the gateway itself, to the guest.
fn say(command: &str, params: &[&str]) -> Message {
    let mut line = Line::new(command, params.iter().copied());
    line.source = Some("gateway".into());
    Message::text(line.to_string())
}

async fn send(socket: &mut WebSocket, message: Message) -> Result<(), String> {
    socket.send(message).await.map_err(|e| e.to_string())
}

/// What a guest sent, line by line.
fn lines(message: Message) -> Vec<String> {
    let text = match message {
        Message::Text(text) => text.to_string(),
        Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        _ => return vec![],
    };
    text.split(['\r', '\n'])
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// What a guest's client says first: their token (`PASS`), and any `CAP` lines before it,
/// which wait for Ergo: no more than [`MAX_EARLY_LINES`] of them, in [`MAX_EARLY_BYTES`].
async fn hello(socket: &mut WebSocket) -> Result<(String, Vec<Line>), String> {
    let mut early = Early::default();
    let token = tokio::time::timeout(HELLO, async {
        while let Some(Ok(message)) = socket.recv().await {
            for text in lines(message) {
                match early.take(&text) {
                    Said::Token(token) => return Ok(token),
                    Said::TooMuch => return Err("too much before the token"),
                    Said::More => {}
                }
            }
        }
        Err("no token")
    })
    .await
    .unwrap_or(Err("no token"))?;
    Ok((token, early.lines))
}

/// The `CAP` lines a guest sent before their token, counted.
#[derive(Default)]
struct Early {
    lines: Vec<Line>,
    bytes: usize,
}

/// What one line before the token was.
#[derive(Debug, PartialEq)]
enum Said {
    Token(String),
    More,
    TooMuch,
}

impl Early {
    /// Takes one line a guest sent before their token: the token, if it's `PASS`.
    fn take(&mut self, text: &str) -> Said {
        self.bytes += text.len();
        if self.bytes > MAX_EARLY_BYTES {
            return Said::TooMuch;
        }
        let Ok(line) = text.parse::<Line>() else {
            return Said::More;
        };
        match line.command.to_ascii_uppercase().as_str() {
            "PASS" => line.params.first().cloned().map_or(Said::More, Said::Token),
            "CAP" if self.lines.len() >= MAX_EARLY_LINES => Said::TooMuch,
            "CAP" => {
                self.lines.push(line);
                Said::More
            }
            _ => Said::More,
        }
    }
}

/// Who `token` says they are, where `public` lets them in at `now`: a terminal's grant and
/// token, used once, or a browser's token. Why not, otherwise.
fn token_guest(
    gateway: &Gateway,
    public: &Public,
    token: &str,
    now: u64,
) -> Result<Token, &'static str> {
    let listed = if public.keys.is_empty() {
        public.key.iter().cloned().collect()
    } else {
        public.keys.clone()
    };
    let keys: Vec<Vec<u8>> = listed.iter().filter_map(|k| public_key(k)).collect();
    if zirc_proto::guest_terminal::split_pass(token).is_some() {
        verify_terminal(token, &keys, now).and_then(|(t, once, exp)| {
            let public_channel = zirc_proto::guest_room_of(&t.channel)
                .map_or(t.channel.as_str(), |(parent, _)| parent);
            if !public.terminals.contains(public_channel) {
                Err("this channel doesn't take terminals")
            } else if !gateway.first_use(once, exp, now) {
                Err("used before")
            } else {
                Ok(t)
            }
        })
    } else {
        verify_any(token, &keys, now)
    }
    .and_then(|t| {
        if admits(&public.channels, &t.guest, &t.channel) {
            Ok(t)
        } else {
            Err("that channel isn't public")
        }
    })
}

/// `checked`, or else tells the guest why not and ends their session with it.
async fn or_refuse<T>(
    socket: &mut WebSocket,
    checked: Result<T, &'static str>,
) -> Result<T, String> {
    match checked {
        Ok(checked) => Ok(checked),
        Err(why) => {
            send(socket, say("ERROR", &[why])).await?;
            Err(why.into())
        }
    }
}

/// What the gateway says to Ergo for a guest coming in as `nick`, before anything of theirs:
/// the gate's password, the capabilities it needs itself, their account's `login`, the
/// `CAP` lines their client sent `early`, and who they are.
fn opening(
    gateway: &Gateway,
    guest: &Token,
    login: Option<&str>,
    early: Vec<Line>,
    nick: &str,
) -> Vec<Line> {
    let mut opening = Vec::new();
    if let Some(gate) = &gateway.gate {
        opening.push(Line::new("PASS", [gate.as_str()]));
    }
    opening.push(Line::new("CAP", ["REQ", own_caps(login.is_some())]));
    if let Some(pass) = login {
        let name = guest.guest.to_lowercase();
        let plain = STANDARD.encode(format!("{name}\0{name}\0{pass}"));
        opening.push(Line::new("AUTHENTICATE", ["PLAIN"]));
        opening.push(Line::new("AUTHENTICATE", [plain.as_str()]));
    }
    if early.is_empty() {
        opening.push(Line::new("CAP", ["END"]));
    }
    opening.extend(early);
    opening.push(Line::new("NICK", [nick]));
    let realname = guest.name.as_deref().unwrap_or(REALNAME);
    opening.push(Line::new("USER", ["guest", "0", "*", realname]));
    opening
}

/// The capabilities the gateway asks Ergo for itself: who sent what (`delivered`), and, when
/// it `logs_in` to the guest's account, SASL. Its own, so their answers aren't passed on; and
/// ended by the gateway when the guest's client doesn't negotiate, which Ergo would otherwise
/// wait for.
const fn own_caps(logs_in: bool) -> &'static str {
    if logs_in {
        "account-tag sasl"
    } else {
        "account-tag"
    }
}

/// What a line from Ergo did.
enum FromErgo {
    /// It went on to the guest.
    Passed,
    /// It was the gateway's own, or not for the guest: nothing went on.
    Kept,
    /// It ended the session: the guest may not come in, and was told so.
    Ended,
}

/// A guest's session once Ergo has been greeted: who they are, and what the gateway keeps
/// while their lines and Ergo's pass through it.
struct Passing {
    gateway: Arc<Gateway>,
    guest: Token,
    /// The channel they're in.
    channel: String,
    /// Their nick here, lowercased.
    me: String,
    /// The guest they are, lowercased: the same but for a terminal.
    who: String,
    demo: bool,
    /// Whether the gateway logged in to their account.
    logs_in: bool,
    /// Whom they may answer.
    repliers: HashSet<String>,
    /// The capabilities the gateway asked for itself, until Ergo has answered that.
    own_caps: Option<&'static str>,
    talk: Lane,
    quiet: Lane,
    registered: bool,
    /// For a room of their own, its public channel.
    room: Option<String>,
    /// Until their room is set up, whether anyone else is there (by its NAMES); on a demo
    /// server, until the cast has been told they're in. None from the first for a terminal,
    /// which comes in beside its guest, who has been: neither the cast nor the team is told
    /// again.
    arriving: Option<bool>,
    multiline: Multiline,
}

impl Passing {
    /// The session of `guest`, come in as `nick`, whom the gateway has greeted Ergo for.
    fn new(
        gateway: &Arc<Gateway>,
        guest: Token,
        nick: &str,
        demo: bool,
        logs_in: bool,
        repliers: HashSet<String>,
    ) -> Self {
        let room = zirc_proto::guest_room_of(&guest.channel).map(|(parent, _)| parent.to_owned());
        let set_up = (room.is_none() && !demo) || guest.machine.is_some();
        Self {
            gateway: Arc::clone(gateway),
            channel: guest.channel.clone(),
            me: nick.to_lowercase(),
            who: guest.guest.to_lowercase(),
            demo,
            logs_in,
            repliers,
            own_caps: Some(own_caps(logs_in)),
            talk: Lane::new(BURST),
            quiet: Lane::new(QUIET_BURST),
            registered: false,
            room,
            arriving: (!set_up).then_some(false),
            multiline: Multiline::default(),
            guest,
        }
    }

    /// What the guest sent: each line that's theirs to send queued at its pace, the rest
    /// dropped or refused, saying why.
    ///
    /// # Errors
    ///
    /// When they send faster than their pace's queue holds, having told them so.
    async fn their_lines(
        &mut self,
        socket: &mut WebSocket,
        message: Message,
    ) -> Result<(), String> {
        let channel = &self.channel;
        for text in lines(message) {
            let Ok(line) = text.parse::<Line>() else {
                continue;
            };
            let command = line.command.to_ascii_uppercase();
            // Who they are is the token's: their own NICK and USER aren't passed on.
            if matches!(command.as_str(), "NICK" | "USER" | "PASS") {
                continue;
            }
            if !(allowed(&line, channel, &self.repliers) || (!self.registered && command == "CAP"))
            {
                info!(command, "a guest's line was refused"); // never what it said
                let why =
                    format!("Guests can talk only in {channel}, and answer whoever wrote to them");
                let refused = say("FAIL", &[command.as_str(), "GUEST_ONLY", why.as_str()]);
                send(socket, refused).await?;
                continue;
            }
            if command == "PRIVMSG"
                && line
                    .params
                    .first()
                    .is_some_and(|t| t.eq_ignore_ascii_case(channel))
                && let Some(text) = line.params.get(1)
            {
                let room = self.room.as_deref().unwrap_or(channel);
                self.gateway.visitor_said(&self.who, text, room, channel);
            }
            if command == "TAGMSG" && line.tag("+draft/react").is_some() {
                info!("a guest reacted"); // never to what, nor with which
            }
            let pace = if is_talk(&line) {
                &mut self.talk
            } else {
                &mut self.quiet
            };
            pace.queue.push_back(line);
            if pace.queue.len() > QUEUE {
                send(socket, say("ERROR", &["too fast"])).await?;
                return Err("too fast".into());
            }
        }
        Ok(())
    }

    /// A line from Ergo, `text`: passed on to the guest unless it's the gateway's own or
    /// isn't for them, after the gateway has done what it calls for.
    async fn ergo_line(
        &mut self,
        socket: &mut WebSocket,
        to_ergo: &mut tokio::net::tcp::OwnedWriteHalf,
        text: String,
    ) -> Result<FromErgo, String> {
        if let Ok(line) = text.parse::<Line>() {
            // The answer to the gateway's own CAP REQ, the first such, and its login.
            if let Some(caps) = self.own_caps
                && line.command == "CAP"
                && line.params.get(1).is_some_and(|s| s == "ACK" || s == "NAK")
                && line.params.last().is_some_and(|c| c.trim() == caps)
            {
                self.own_caps = None;
                return Ok(FromErgo::Kept);
            }
            if self.logs_in {
                match line.command.as_str() {
                    "AUTHENTICATE" | "900" | "903" => return Ok(FromErgo::Kept),
                    // Suspended, as for a guest the organisation banned.
                    "902" | "904" | "905" | "906" => {
                        send(socket, say("ERROR", &["you can't come in here"])).await?;
                        return Ok(FromErgo::Ended);
                    }
                    _ => {}
                }
            }
            if !self.multiline.delivered(&line, &self.me, self.demo) {
                return Ok(FromErgo::Kept);
            }
            if !self.heard(to_ergo, &line).await? {
                return Ok(FromErgo::Kept);
            }
        }
        send(socket, Message::text(text)).await?;
        Ok(FromErgo::Passed)
    }

    /// What the gateway does about a `line` from Ergo that's for the guest: joins them once
    /// they're in, sets their room up once it knows who's there, and lets them answer staff
    /// who write to them. Whether the line goes on to them.
    async fn heard(
        &mut self,
        to_ergo: &mut tokio::net::tcp::OwnedWriteHalf,
        line: &Line,
    ) -> Result<bool, String> {
        let channel = &self.channel;
        match line.command.as_str() {
            // In: into their channel, saying whether they're signed in to zirc.dev.
            "001" => {
                self.registered = true;
                to_ergo
                    .write_all(format!("JOIN {channel}\r\n").as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                if let Some(signed_in) = self.guest.signed_in {
                    // Under the nick Ergo just welcomed them by, which `*` was on their own
                    // connection, when a client could still set its own.
                    let nick = line.params.first().map_or(self.me.as_str(), String::as_str);
                    self.gateway.tell_signed_in(nick, signed_in).await;
                }
            }
            // Their own value, as Ergo tells a client it's subscribed to: the gateway's.
            "761" if line.params.get(2).is_some_and(|k| k == SIGNED_IN) => return Ok(false),
            // Who's in their room as they come in, then: it's made secret, and the
            // team told they're waiting if none of them is there.
            "353"
                if self.arriving.is_some()
                    && line
                        .params
                        .get(2)
                        .is_some_and(|c| c.eq_ignore_ascii_case(channel)) =>
            {
                let names = line.params.last().map(String::as_str).unwrap_or_default();
                let others = names
                    .split(' ')
                    .map(|n| {
                        n.trim_start_matches(['@', '+', '%', '~', '&'])
                            .to_lowercase()
                    })
                    .any(|n| !n.is_empty() && n != self.me);
                self.arriving = self.arriving.map(|others_here| others_here || others);
            }
            "366"
                if self.arriving.is_some()
                    && line
                        .params
                        .get(1)
                        .is_some_and(|c| c.eq_ignore_ascii_case(channel)) =>
            {
                let others_here = self.arriving.take().unwrap_or_default();
                let name = self.guest.name.as_deref();
                if let Some(parent) = &self.room {
                    self.gateway
                        .set_up_room(channel, parent, &self.who, name, others_here);
                }
                self.gateway.arrived(channel, &self.who);
                let room = self.room.as_deref().unwrap_or(channel);
                self.gateway.visitor_arrived(&self.who, name, room, channel);
            }
            // A person on staff who writes to them may be answered, from now on.
            "PRIVMSG" | "NOTICE" | "TAGMSG"
                if line
                    .params
                    .first()
                    .is_some_and(|t| t.to_lowercase() == self.me) =>
            {
                if let Some(account) = line.tag("account").map(str::to_lowercase)
                    && person(&account)
                {
                    self.gateway.may_answer(&self.who, &account);
                    self.repliers.insert(account);
                }
            }
            _ => {}
        }
        Ok(true)
    }

    /// Sends Ergo as many of the guest's lines as their paces allow.
    async fn send_due(
        &mut self,
        to_ergo: &mut tokio::net::tcp::OwnedWriteHalf,
    ) -> Result<(), String> {
        for line in [self.quiet.due(), self.talk.due()].into_iter().flatten() {
            to_ergo
                .write_all(format!("{line}\r\n").as_bytes())
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// One guest, from their token to their leaving.
async fn session(
    mut socket: WebSocket,
    gateway: Arc<Gateway>,
    address: IpAddr,
) -> Result<(), String> {
    let (token, early) = hello(&mut socket).await?;
    let public = lock(&gateway.public).clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let checked = token_guest(&gateway, &public, &token, now);
    let guest = or_refuse(&mut socket, checked).await?;
    let admitted = gateway.admit(
        &guest.guest,
        guest.machine.as_deref(),
        address,
        Instant::now(),
    );
    let mut admitted = or_refuse(&mut socket, admitted).await?;
    // A terminal logs in to no account: its guest's would make it them, not beside them.
    let account = if guest.machine.is_some() {
        Ok(None)
    } else {
        gateway.account(&guest.guest.to_lowercase()).await
    };
    let login = or_refuse(&mut socket, account).await?;

    let ergo = tokio::net::TcpStream::connect(gateway.ergo)
        .await
        .map_err(|e| e.to_string())?;
    let (from_ergo, mut to_ergo) = ergo.into_split();
    let mut from_ergo = BufReader::new(from_ergo).lines();
    let nick = guest
        .machine
        .as_deref()
        .map_or_else(|| guest.guest.clone(), |m| terminal_nick(&guest.guest, m));
    for line in &opening(&gateway, &guest, login.as_deref(), early, &nick) {
        to_ergo
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }
    // Never which channel: logs go to Google.
    if let Some(machine) = &guest.machine {
        info!(guest = guest.guest, machine, "a guest's terminal came in");
    } else {
        info!(guest = guest.guest, "a guest came in");
    }

    let (channel, who) = (guest.channel.clone(), guest.guest.to_lowercase());
    let demo = public.demo;
    gateway.note(&who, &channel);
    let mut repliers = gateway.repliers(&who).await;
    // On a demo server, the cast may be written to from the first: that's the demo.
    if demo {
        repliers.extend(crate::demo::cast().iter().cloned());
    }
    let mut refill = tokio::time::interval(EVERY);
    refill.tick().await;
    let mut refill_quiet = tokio::time::interval(QUIET_EVERY);
    refill_quiet.tick().await;
    let mut passing = Passing::new(&gateway, guest, &nick, demo, login.is_some(), repliers);
    // Whether it was ended by the gateway (connected elsewhere, forgotten, or not let in)
    // rather than by them.
    let ended = loop {
        tokio::select! {
            _ = admitted.ended.changed() => {
                send(&mut socket, say("ERROR", &["connected elsewhere"])).await?;
                break true;
            }
            from_them = socket.recv() => {
                let Some(Ok(message)) = from_them else { break false };
                passing.their_lines(&mut socket, message).await?;
            }
            _ = refill.tick() => passing.talk.refill(),
            _ = refill_quiet.tick() => passing.quiet.refill(),
            from_server = from_ergo.next_line() => {
                let Some(text) = from_server.map_err(|e| e.to_string())? else { break false };
                match passing.ergo_line(&mut socket, &mut to_ergo, text).await? {
                    FromErgo::Passed => {}
                    FromErgo::Kept => continue,
                    FromErgo::Ended => break true,
                }
            }
        }
        passing.send_due(&mut to_ergo).await?;
    };
    if !ended {
        gateway.note(&who, &channel);
    }
    let _ = to_ergo.write_all(b"QUIT :left\r\n").await;
    if let Err(error) = socket.send(Message::Close(None)).await {
        warn!(%error, "couldn't close a guest's WebSocket");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn pair() -> Ed25519KeyPair {
        let rng = ring::rand::SystemRandom::new();
        Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref()).unwrap()
    }

    /// A token as zirc.dev signs one.
    pub(crate) fn token(key: &Ed25519KeyPair, claims: &serde_json::Value) -> String {
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"EdDSA","typ":"JWT"}"#),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        format!(
            "{signed}.{}",
            URL_SAFE_NO_PAD.encode(key.sign(signed.as_bytes()).as_ref())
        )
    }

    #[test]
    fn the_controllers_word_on_keys_reads_on_old_sidecars_and_new() {
        // On 7 Nov the controller sent `keys` alone to sidecars that read only `key`, and
        // every guest heard "this server has no public channels".
        let sent = Public {
            keys: vec!["A".into(), "B".into()],
            key: Some("A".into()),
            ..Public::default()
        };
        let json = serde_json::to_value(&sent).unwrap();
        assert_eq!(json["key"], "A");
        assert_eq!(json["keys"], serde_json::json!(["A", "B"]));
        // What a controller from before `keys` sends.
        let old: Public =
            serde_json::from_str(r##"{"channels": ["#support"], "key": "A"}"##).unwrap();
        assert_eq!((old.keys.len(), old.key.as_deref()), (0, Some("A")));
    }

    #[test]
    fn a_guest_token_signed_with_any_of_the_servers_keys_is_good() {
        // As on 7 Nov: the server was sent [old, new], and zirc.dev then signed with the new
        // one, listed second; checking the first alone turned every guest away.
        let (old, new, stranger) = (pair(), pair(), pair());
        let keys = [
            old.public_key().as_ref().to_vec(),
            new.public_key().as_ref().to_vec(),
        ];
        let good = serde_json::json!({"guest": "guest-7k2q", "channel": "#support", "exp": 2000});
        assert!(verify_any(&token(&new, &good), &keys, 1000).is_ok());
        assert!(verify_any(&token(&old, &good), &keys, 1000).is_ok());
        assert_eq!(
            verify_any(&token(&stranger, &good), &keys, 1000),
            Err("not signed for this server")
        );
        assert_eq!(
            verify_any(&token(&new, &good), &[], 1000),
            Err("this server has no public channels")
        );
    }

    #[test]
    fn a_guest_token_names_a_guest_and_one_channel() {
        let (web, stranger) = (pair(), pair());
        let key = web.public_key().as_ref().to_vec();
        let claims = |c: serde_json::Value| token(&web, &c);
        let good = serde_json::json!({"guest": "guest-7k2q", "channel": "#Support", "exp": 2000});
        assert_eq!(
            verify(&claims(good.clone()), &key, 1000),
            Ok(Token {
                guest: "guest-7k2q".into(),
                channel: "#support".into(),
                signed_in: None,
                name: None,
                machine: None,
            })
        );
        let signed_in = serde_json::json!({"guest": "guest-7k2q", "channel": "#support", "exp": 2000, "signedIn": true});
        assert_eq!(
            verify(&claims(signed_in), &key, 1000).map(|t| t.signed_in),
            Ok(Some(true))
        );
        // The name they typed, trimmed, is their real name; one zirc wouldn't take is dropped.
        let named = |name: &str| {
            let c = serde_json::json!({"guest": "guest-21042", "channel": "#support", "exp": 2000, "name": name});
            verify(&claims(c), &key, 1000).map(|t| t.name)
        };
        assert_eq!(named(" Sam "), Ok(Some("Sam".into())));
        assert_eq!(named("Sam\r\nQUIT"), Ok(None));
        assert_eq!(named(&"a".repeat(65)), Ok(None));
        assert_eq!(verify(&claims(good.clone()), &key, 2000), Err("expired"));
        assert_eq!(
            verify(&token(&stranger, &good), &key, 1000),
            Err("not signed for this server")
        );
        let login = serde_json::json!({"guest": "guest-7k2q", "channel": "#support", "exp": 2000, "account": "mattias"});
        assert_eq!(
            verify(&claims(login), &key, 1000),
            Err("not a guest's token")
        );
        let member = serde_json::json!({"guest": "mattias", "channel": "#support", "exp": 2000});
        assert_eq!(
            verify(&claims(member), &key, 1000),
            Err("not a guest's name")
        );
        assert_eq!(verify("a.b", &key, 1000), Err("not a token"));
    }

    /// A device's key, as zircd's, and a token from it.
    struct Device(ring::signature::EcdsaKeyPair);

    impl Device {
        fn new() -> Self {
            use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            Self(
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap(),
            )
        }

        fn key(&self) -> String {
            zirc_proto::device_key::key_text(self.0.public_key().as_ref())
        }

        fn proof(&self, guest: &str, gid: &str, now: u64, jti: &str) -> String {
            use zirc_proto::device_key;
            let claims = device_key::Claims::new(guest, gid, now, jti.into());
            let input = device_key::signing_input(&self.key(), &claims);
            let rng = ring::rand::SystemRandom::new();
            let signature = self.0.sign(&rng, input.as_bytes()).unwrap();
            device_key::token(&input, signature.as_ref())
        }
    }

    fn grant(web: &Ed25519KeyPair, device: &Device, exp: u64) -> String {
        token(
            web,
            &serde_json::json!({"sub": "guest-21042", "room": "#Web/guest-21042", "machine": "mbp",
                "key": device.key(), "gid": "g1", "exp": exp}),
        )
    }

    #[test]
    fn a_terminal_brings_its_grant_and_a_token_from_the_key_it_names() {
        use zirc_proto::guest_terminal::pass;
        let (web, stranger) = (pair(), pair());
        let keys = [web.public_key().as_ref().to_vec()];
        let (device, thief) = (Device::new(), Device::new());
        let good = grant(&web, &device, 5000);
        let (token, once, exp) = verify_terminal(
            &pass(&good, &device.proof("guest-21042", "g1", 1000, "j1")),
            &keys,
            1000,
        )
        .unwrap();
        assert_eq!(
            token,
            Token {
                guest: "guest-21042".into(),
                channel: "#web/guest-21042".into(),
                signed_in: None,
                name: None,
                machine: Some("mbp".into()),
            }
        );
        assert_eq!((once, exp), (format!("{} j1", device.key()), 1120));
        let refused = |pass: String, now: u64| verify_terminal(&pass, &keys, now).map(|_| ());
        // Another key's token, for another grant, for another guest, out of date.
        assert_eq!(
            refused(
                pass(&good, &thief.proof("guest-21042", "g1", 1000, "j")),
                1000
            ),
            Err("not this terminal's key")
        );
        assert_eq!(
            refused(
                pass(&good, &device.proof("guest-21042", "g2", 1000, "j")),
                1000
            ),
            Err("for another server")
        );
        assert_eq!(
            refused(
                pass(&good, &device.proof("guest-99999", "g1", 1000, "j")),
                1000
            ),
            Err("not this terminal's key")
        );
        assert_eq!(
            refused(
                pass(&good, &device.proof("guest-21042", "g1", 1000, "j")),
                1200
            ),
            Err("expired")
        );
        // A grant that's expired, or not zirc.dev's.
        let old = grant(&web, &device, 900);
        assert_eq!(
            refused(
                pass(&old, &device.proof("guest-21042", "g1", 1000, "j")),
                1000
            ),
            Err("expired")
        );
        let forged = grant(&stranger, &device, 5000);
        assert_eq!(
            refused(
                pass(&forged, &device.proof("guest-21042", "g1", 1000, "j")),
                1000
            ),
            Err("not signed for this server")
        );
        // A grant alone is no guest's token, on this gateway or one from before terminals.
        assert_eq!(verify_any(&good, &keys, 1000), Err("not a token"));
        // Each token once.
        let gateway = Gateway::new(([127, 0, 0, 1], 1).into(), None, None, None);
        assert!(gateway.first_use("k j1".into(), 1120, 1000));
        assert!(!gateway.first_use("k j1".into(), 1120, 1001));
        assert!(gateway.first_use("k j1".into(), 1300, 1200));
    }

    #[test]
    fn a_terminals_nick_is_its_guests_and_its_machines() {
        assert_eq!(terminal_nick("guest-21042", "mbp"), "guest-21042-mbp");
        let long = terminal_nick("guest-21042", &"m".repeat(40));
        assert_eq!(long.len(), zirc_proto::names::NICK_MAX);
        assert!(zirc_proto::named_as_guest(&long));
        assert_eq!(
            terminal_nick("guest-21042", &format!("{}-x", "m".repeat(19))),
            format!("guest-21042-{}", "m".repeat(19))
        );
    }

    #[test]
    fn a_terminal_comes_in_beside_its_guest_and_ends_only_its_own_older_connection() {
        let gateway = Gateway::new(([127, 0, 0, 1], 1).into(), None, None, None);
        let at: IpAddr = [10, 0, 0, 1].into();
        let now = Instant::now();
        let browser = gateway.admit("guest-21042", None, at, now).unwrap();
        let terminal = gateway.admit("guest-21042", Some("mbp"), at, now).unwrap();
        assert!(!*browser.ended.borrow());
        let again = gateway.admit("guest-21042", Some("mbp"), at, now).unwrap();
        assert!(*terminal.ended.borrow());
        assert!(!*browser.ended.borrow());
        assert_eq!(gateway.presence().0, ["guest-21042".to_owned()].into());
        gateway.forget("guest-21042");
        assert!(*browser.ended.borrow());
        assert!(*again.ended.borrow());
    }

    #[test]
    fn a_token_names_a_public_channel_or_the_guests_own_room_beside_one() {
        let public: BTreeSet<String> = ["#support".to_owned()].into();
        assert!(admits(&public, "guest-21042", "#support"));
        assert!(admits(&public, "guest-21042", "#support/guest-21042"));
        assert!(admits(&public, "Guest-21042", "#support/guest-21042"));
        assert!(!admits(&public, "guest-21042", "#support/guest-99999"));
        assert!(!admits(&public, "guest-21042", "#internal/guest-21042"));
        assert!(!admits(&public, "guest-21042", "#internal"));
        assert_eq!(
            waiting("guest-21042", Some("Sam"), "#support/guest-21042"),
            "Sam (guest-21042) is waiting in #support/guest-21042"
        );
        assert_eq!(
            waiting("guest-21042", None, "#support/guest-21042"),
            "guest-21042 is waiting in #support/guest-21042"
        );
    }

    #[test]
    fn the_key_comes_from_its_pem() {
        let web = pair();
        let spki = [
            &[
                0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
            ][..],
            web.public_key().as_ref(),
        ]
        .concat();
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(spki)
        );
        assert_eq!(public_key(&pem).unwrap(), web.public_key().as_ref());
        assert_eq!(
            public_key("-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----"),
            None
        );
    }

    #[test]
    fn a_guest_talks_in_their_channel_and_answers_who_wrote_to_them() {
        let line = |text: &str| text.parse::<Line>().unwrap();
        let mut repliers = HashSet::new();
        let may =
            |text: &str, repliers: &HashSet<String>| allowed(&line(text), "#support", repliers);
        assert!(may("PRIVMSG #support :hello", &repliers));
        assert!(may("PRIVMSG #SUPPORT :hello", &repliers));
        assert!(may("@+typing=active TAGMSG #support", &repliers));
        assert!(may("CHATHISTORY LATEST #support * 50", &repliers));
        assert!(may("BATCH +a draft/multiline #support", &repliers));
        assert!(may("BATCH -a", &repliers));
        assert!(may("PING :x", &repliers));
        assert!(may("WHO #support %tnfr,42", &repliers));
        assert!(may(
            "MARKREAD #support timestamp=2026-11-02T18:00:00.000Z",
            &repliers
        ));
        assert!(may(
            "CHATHISTORY TARGETS timestamp=2026-11-01T00:00:00.000Z timestamp=2026-11-03T00:00:00.000Z 50",
            &repliers
        ));
        assert!(!may("WHO #general", &repliers));
        assert!(!may("PRIVMSG mattias :can we talk", &repliers));
        assert!(!may("PRIVMSG #general :hi", &repliers));
        assert!(!may("JOIN #general", &repliers));
        assert!(!may("LIST", &repliers));
        assert!(may("METADATA * SUB display-name avatar", &repliers));
        assert!(may("METADATA mattias GET display-name", &repliers));
        assert!(may("METADATA #support SYNC", &repliers));
        assert!(!may(
            "METADATA * SET display-name :Mattias (staff)",
            &repliers
        ));
        assert!(!may("METADATA * CLEAR", &repliers));
        assert!(!may("NAMES #general", &repliers));
        assert!(!may("WHOIS mattias", &repliers));
        assert!(!may("CHATHISTORY LATEST #general * 50", &repliers));
        assert!(!may("MODE #support +o guest-7k2q", &repliers));
        repliers.insert("mattias".into());
        assert!(may("PRIVMSG Mattias :thanks", &repliers));
        assert!(may("CHATHISTORY LATEST mattias * 50", &repliers));
    }

    #[test]
    fn a_guest_hears_from_people_on_staff_and_never_from_agents() {
        let heard = |text: &str| delivered(&text.parse::<Line>().unwrap(), "guest-7k2q", false);
        // People on staff, privately and in the channel.
        assert!(heard(
            "@account=mattias :mattias!m@h PRIVMSG guest-7k2q :happy to help"
        ));
        assert!(heard(
            "@account=mattias :mattias!m@h PRIVMSG #support :welcome"
        ));
        assert!(heard("@account=bob :bob!b@h TAGMSG guest-7k2q"));
        // Agents (machines' accounts), nowhere; their typing neither.
        assert!(!heard(
            "@account=mattias-mbp :mattias-mbp!m@h PRIVMSG guest-7k2q :hi"
        ));
        assert!(!heard(
            "@account=mattias-mbp :mattias-mbp!m@h PRIVMSG #support :build passed"
        ));
        assert!(!heard(
            "@account=mattias-mbp;+typing=active :mattias-mbp!m@h TAGMSG #support"
        ));
        // No account, or another guest's, or the founder: never privately.
        assert!(!heard(":stranger!s@h PRIVMSG guest-7k2q :hello"));
        assert!(!heard(
            "@account=guest-9x8w :guest-9x8w!g@h PRIVMSG guest-7k2q :hi"
        ));
        assert!(!heard("@account=zirc :zirc!z@h PRIVMSG guest-7k2q :hi"));
        // But the server's services, and the server itself, as ever.
        assert!(heard(
            ":NickServ!NickServ@localhost NOTICE guest-7k2q :hello"
        ));
        assert!(heard(
            ":irc.zirc.test NOTICE guest-7k2q :*** Looking up your hostname"
        ));
        assert!(heard(":irc.zirc.test 001 guest-7k2q :Welcome"));
        // Their own lines, echoed.
        assert!(heard(":guest-7k2q!g@h PRIVMSG #support :a question"));
        // Of the team's events, only that someone joined.
        assert!(heard(
            "@account=bob;+zirc/support=joined :bob!b@h TAGMSG guest-7k2q"
        ));
        assert!(!heard(
            "@account=mattias;+zirc/support=assigned:bob :mattias!m@h TAGMSG guest-7k2q"
        ));
        assert!(!heard(
            "@account=bob;+zirc/support=closed :bob!b@h TAGMSG guest-7k2q"
        ));
    }

    #[test]
    fn on_a_demo_server_a_guest_hears_the_casts_agents_in_their_room_only() {
        let heard = |text: &str| delivered(&text.parse::<Line>().unwrap(), "guest-7k2q", true);
        assert!(heard(
            "@account=maya-mbp :maya-mbp!m@h PRIVMSG #web/guest-7k2q :about to change Coupon.tsx"
        ));
        assert!(heard(
            "@account=zirc-ci :zirc-ci!c@h PRIVMSG #web/guest-7k2q :landed checkout"
        ));
        // Privately too, from the cast, answering them.
        assert!(heard(
            "@account=maya-mbp :maya-mbp!m@h PRIVMSG guest-7k2q :hi"
        ));
        assert!(heard("@account=maya :maya!m@h PRIVMSG guest-7k2q :hi"));
        // But never from another guest, nor from a nick the cast doesn't have.
        assert!(!heard(
            "@account=guest-9x8w :guest-9x8w!g@h PRIVMSG guest-7k2q :hi"
        ));
        assert!(!heard(
            "@account=stranger-mbp :stranger-mbp!s@h PRIVMSG guest-7k2q :hi"
        ));
        // Nor anywhere else.
        let elsewhere = |text: &str| delivered(&text.parse::<Line>().unwrap(), "guest-7k2q", false);
        assert!(!elsewhere(
            "@account=maya-mbp :maya-mbp!m@h PRIVMSG guest-7k2q :hi"
        ));
    }

    #[test]
    fn a_guests_talk_has_its_own_pace_and_what_their_app_sends_by_itself_another() {
        let talk = |text: &str| is_talk(&text.parse::<Line>().unwrap());
        assert!(talk("PRIVMSG #web/guest-7k2q :hi"));
        assert!(talk("BATCH +m draft/multiline #web/guest-7k2q"));
        assert!(talk(
            "@+draft/react=👍;+draft/reply=m1 TAGMSG #web/guest-7k2q"
        ));
        // A typing notice, and housekeeping, which the app sends with each new message.
        assert!(!talk("@+typing=active TAGMSG #web/guest-7k2q"));
        assert!(!talk(
            "MARKREAD #web/guest-7k2q timestamp=2026-11-08T07:00:00.000Z"
        ));
        assert!(!talk("METADATA maya GET name"));
        assert!(!talk("CHATHISTORY LATEST #web/guest-7k2q * 50"));

        // Read markers that used up the old single pace leave a reaction to go at once.
        let mut lanes = (Lane::new(BURST), Lane::new(QUIET_BURST));
        for _ in 0..10 {
            lanes.1.queue.push_back("MARKREAD #web".parse().unwrap());
        }
        lanes
            .0
            .queue
            .push_back("@+draft/react=👍 TAGMSG #web".parse().unwrap());
        assert_eq!(lanes.1.due().len(), 10);
        assert_eq!(lanes.0.due().len(), 1);
        // Each still at its pace: the talk lane's burst spent, then one a refill.
        for _ in 0..BURST + 2 {
            lanes.0.queue.push_back("PRIVMSG #web :hi".parse().unwrap());
        }
        assert_eq!(lanes.0.due().len(), usize::try_from(BURST - 1).unwrap());
        lanes.0.refill();
        assert_eq!(lanes.0.due().len(), 1);
    }

    #[test]
    fn a_multiline_message_goes_as_its_opening_does() {
        fn lines(texts: &[String], demo: bool) -> Vec<bool> {
            let mut multiline = Multiline::default();
            texts
                .iter()
                .map(|t| multiline.delivered(&t.parse::<Line>().unwrap(), "guest-7k2q", demo))
                .collect()
        }
        fn from(account: &str, target: &str) -> Vec<String> {
            vec![
                format!("@account={account} :{account}!u@h BATCH +1 draft/multiline {target}"),
                format!("@batch=1 :{account}!u@h PRIVMSG {target} :a long"),
                format!("@batch=1 :{account}!u@h PRIVMSG {target} :message"),
                format!(":{account}!u@h BATCH -1"),
            ]
        }
        // From staff, privately: all of it.
        assert_eq!(lines(&from("mattias", "guest-7k2q"), false), [true; 4]);
        // From an agent: none of it, privately or in their channel, but on the demo's server.
        assert_eq!(lines(&from("maya-mbp", "guest-7k2q"), false), [false; 4]);
        assert_eq!(lines(&from("maya-mbp", "#support"), false), [false; 4]);
        assert_eq!(lines(&from("maya-mbp", "guest-7k2q"), true), [true; 4]);
        // History's batches aren't multiline messages: their lines are judged one by one.
        let history = [
            "BATCH +h chathistory #support",
            "@batch=h;account=maya-mbp :maya-mbp!u@h PRIVMSG #support :hi",
            "@batch=h;account=mattias :mattias!u@h PRIVMSG #support :hi",
            "BATCH -h",
        ]
        .map(String::from);
        assert_eq!(lines(&history, false), [true, false, true, true]);
    }

    #[test]
    fn on_a_demo_server_a_guest_may_write_to_the_cast_but_not_to_another_guest() {
        let mut repliers: HashSet<String> = HashSet::new();
        repliers.extend(crate::demo::cast().iter().cloned());
        let may = |text: &str| allowed(&text.parse::<Line>().unwrap(), "#web", &repliers);
        assert!(may("PRIVMSG maya :hi"));
        assert!(may("PRIVMSG maya-mbp :what are you doing?"));
        assert!(!may("PRIVMSG guest-9x8w :hi"));
        assert!(!may("PRIVMSG mattias :hi"));
    }

    #[test]
    fn people_on_staff_are_people_and_nothing_else_is() {
        assert!(person("mattias"));
        assert!(!person("mattias-mbp"));
        assert!(!person("guest-7k2q"));
        assert!(!person(crate::teams::FOUNDER));
        // A machine whose name could be a handle is still a machine, once the server knows.
        assert!(person("staffbox"));
        crate::machines::Machines::default().add("staffbox", "mattias");
        assert!(!person("staffbox"));
        assert!(!person(zirc_proto::workflow::HOST));
    }

    #[test]
    fn the_alert_flags_are_off_by_default_and_leave_the_wire_form_alone() {
        let json = serde_json::to_string(&Public::default()).unwrap();
        assert!(!json.contains("alert"));
        let on: Public = serde_json::from_str(r#"{"alert":"arrivals"}"#).unwrap();
        assert_eq!(on.alert, Alert::Arrivals);
    }

    #[test]
    fn a_visitor_is_told_only_where_the_server_asks_for_it() {
        let gateway = Gateway::new("127.0.0.1:6668".parse().unwrap(), None, None, None);
        // Nothing asked: nothing waits, however many come.
        for i in 0..3 {
            gateway.visitor_arrived(&format!("guest-{i}"), None, "#support", "#support");
            gateway.visitor_said("guest-1", "hi", "#support", "#support");
        }
        assert!(
            lock(&gateway.visitors)
                .due("#support", Instant::now() + Duration::from_hours(1))
                .is_empty()
        );
    }

    #[test]
    fn a_few_guests_per_address_and_so_many_at_once() {
        let gateway = Gateway::new("127.0.0.1:6668".parse().unwrap(), None, None, None);
        gateway.set(Public {
            cap: 4,
            ..Public::default()
        });
        let now = Instant::now();
        let (one, two) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
        let a = gateway.admit("guest-aaaa", None, one, now).unwrap();
        let _b = gateway.admit("guest-bbbb", None, one, now).unwrap();
        let _c = gateway.admit("guest-cccc", None, one, now).unwrap();
        // A fourth from the same address waits.
        assert!(gateway.admit("guest-dddd", None, one, now).is_err());
        // Another address may, up to the cap.
        let _d = gateway.admit("guest-dddd", None, two, now).unwrap();
        assert_eq!(
            gateway.admit("guest-eeee", None, two, now).err(),
            Some("this room is full; try again shortly")
        );
        // The same guest again ends their older connection.
        drop(a);
        let _again = gateway.admit("guest-bbbb", None, two, now).unwrap();
        // Six opened from one address in a minute, and no more until it's passed.
        let three: IpAddr = "10.0.0.3".parse().unwrap();
        let busy = Gateway::new("127.0.0.1:6668".parse().unwrap(), None, None, None);
        for i in 0..6 {
            drop(
                busy.admit(&format!("guest-x{i:03}"), None, three, now)
                    .unwrap(),
            );
        }
        assert!(busy.admit("guest-y000", None, three, now).is_err());
        assert!(
            busy.admit("guest-y000", None, three, now + Duration::from_secs(61))
                .is_ok()
        );
    }

    #[test]
    fn before_their_token_a_guest_sends_a_few_cap_lines_and_no_more() {
        let mut early = Early::default();
        assert_eq!(early.take("CAP LS 302"), Said::More);
        assert_eq!(early.take("NICK guest-1"), Said::More);
        assert_eq!(early.take("PASS :t0ken"), Said::Token("t0ken".into()));
        assert_eq!(early.lines.len(), 1);

        let mut many = Early::default();
        for _ in 0..MAX_EARLY_LINES {
            assert_eq!(many.take("CAP REQ :message-tags"), Said::More);
        }
        assert_eq!(many.take("CAP REQ :message-tags"), Said::TooMuch);

        // Nor many bytes, in lines that aren't CAP's or aren't lines at all.
        let mut long = Early::default();
        let junk = "x".repeat(MAX_EARLY_BYTES / 2);
        assert_eq!(long.take(&junk), Said::More);
        assert_eq!(long.take(&junk), Said::More);
        assert_eq!(long.take("x"), Said::TooMuch);
    }
}
