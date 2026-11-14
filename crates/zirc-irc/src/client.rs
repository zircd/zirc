//! Connecting to a server. [`connect`] registers — capability negotiation, the server
//! password, SASL and a free nick, as `zirc-line`'s [`Registration`] says — and then hands
//! over to two tasks: a reader that answers PINGs and passes every other line on, and a
//! writer that sends what [`Client`] queues. The reader keeps reading while what it passes on
//! waits to be read, so a busy app still answers the server's PINGs and notices it going
//! quiet; it gives up once [`MAX_BACKLOG_BYTES`] are waiting.

use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::StreamExt as _;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore, pki_types::ServerName};
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};

use zirc_line::registration::{self, DEFAULT_CAPS, Identity, Registration, Sasl, Step, loggable};

use crate::{Line, multiline};

/// IRCv3 allows 8191 bytes of tags on top of the classic 512-byte line.
pub(crate) const MAX_LINE_BYTES: usize = 8191 + 512;
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(30);
/// Lines received but not yet read, before the reader holds them back itself.
const INCOMING_BUFFER: usize = 1024;
/// How much the reader holds back, on top of [`INCOMING_BUFFER`], while [`Incoming`] isn't
/// read, before it ends the connection: the app reconnects and catches up from history,
/// rather than hold ever more or leave the server's PINGs unanswered until it times out.
pub const MAX_BACKLOG_BYTES: usize = 16 * 1024 * 1024;
/// [`Config::keepalive`] unless set.
const KEEPALIVE: Duration = Duration::from_secs(60);
/// [`Config::answer_within`] unless set.
const ANSWER_WITHIN: Duration = Duration::from_secs(30);

/// Where to connect and who to be.
#[derive(Clone)]
pub struct Config {
    /// The server's host name, also checked against its TLS certificate.
    pub host: String,
    /// The server's port, usually 6697 with TLS.
    pub port: u16,
    /// Whether to use TLS, verified against the web's usual root certificates.
    pub tls: bool,
    /// The nick to ask for. A taken one gets `_` appended, a few times.
    pub nick: String,
    /// The username part of `nick!user@host`.
    pub user: String,
    /// The free-text "real name".
    pub realname: String,
    /// The server password, sent as `PASS`, for servers that require one.
    pub server_password: Option<String>,
    /// Makes the server password instead, asked afresh at each connection, as a guest's
    /// terminal's is: its grant and a token used once (`zirc_proto::guest_terminal::pass`).
    /// It goes before [`Config::server_password`]; when it makes none, that is used.
    pub password_maker: Option<Bearer>,
    /// A WebSocket to connect to instead of [`Config::host`] and [`Config::port`], such as a
    /// guest gateway's `wss://acme.irc.zirc.dev/guest`: IRC over it as IRCv3's
    /// `text.ircv3.net` subprotocol has it, a line per text message. `wss://` is verified as
    /// [`Config::tls`] is.
    pub websocket: Option<String>,
    /// An account to log in to with SASL PLAIN.
    pub sasl: Option<Credentials>,
    /// Makes a token to log in with over SASL `OAUTHBEARER` instead, asked afresh at each
    /// connection: a device's short-lived one (`zirc_proto::device_key`). It goes before
    /// [`Config::sasl`]; when it makes none, `sasl` is used.
    pub bearer: Option<Bearer>,
    /// Capabilities to ask for when offered.
    pub caps: Vec<String>,
    /// How long the server may be silent before it's sent a `PING`.
    pub keepalive: Duration,
    /// How long it then has to answer, with anything, before the connection counts as dead
    /// and [`Incoming`] ends. A connection to a server that went away without closing it,
    /// or a network that dropped it silently, ends this way.
    pub answer_within: Duration,
}

impl Config {
    /// A TLS connection as `nick`, asking for [`DEFAULT_CAPS`].
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16, nick: impl Into<String>) -> Self {
        let nick = nick.into();
        Self {
            host: host.into(),
            port,
            tls: true,
            user: nick.clone(),
            realname: nick.clone(),
            nick,
            server_password: None,
            password_maker: None,
            websocket: None,
            sasl: None,
            bearer: None,
            caps: DEFAULT_CAPS.iter().map(|&cap| cap.to_owned()).collect(),
            keepalive: KEEPALIVE,
            answer_within: ANSWER_WITHIN,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("nick", &self.nick)
            .field("user", &self.user)
            .field("realname", &self.realname)
            .field(
                "server_password",
                &self.server_password.as_ref().map(|_| "…"),
            )
            .field("password_maker", &self.password_maker.as_ref().map(|_| "…"))
            .field("websocket", &self.websocket)
            .field("sasl", &self.sasl)
            .field("bearer", &self.bearer.as_ref().map(|_| "…"))
            .field("caps", &self.caps)
            .field("keepalive", &self.keepalive)
            .field("answer_within", &self.answer_within)
            .finish()
    }
}

/// What makes a [`Config::bearer`] token or a [`Config::password_maker`] password, each time
/// one's needed.
pub type Bearer = std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// An account and its password.
#[derive(Clone)]
pub struct Credentials {
    /// The account name.
    pub account: String,
    /// The account's password.
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("account", &self.account)
            .field("password", &"…")
            .finish()
    }
}

impl Config {
    /// Who to register as, in `zirc-line`'s terms.
    fn identity(&self) -> Identity {
        Identity {
            nick: self.nick.clone(),
            user: self.user.clone(),
            realname: self.realname.clone(),
            server_password: self
                .password_maker
                .as_ref()
                .and_then(|make| make())
                .or_else(|| self.server_password.clone()),
            sasl: self
                .bearer
                .as_ref()
                .and_then(|make| make())
                .map(|token| Sasl::OAuthBearer { token })
                .or_else(|| {
                    self.sasl
                        .clone()
                        .map(|Credentials { account, password }| Sasl::Plain { account, password })
                }),
            caps: self.caps.clone(),
        }
    }
}

/// Why connecting or sending failed. An error that has a cause gives it as its
/// [`source`](std::error::Error::source), not in its message, so printing the chain, as
/// anyhow's `{:#}` does, says it once.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The network connection failed.
    #[error("connection failed")]
    Io(#[from] std::io::Error),
    /// TLS couldn't be set up.
    #[error("TLS failed: {0}")]
    Tls(String),
    /// The server sent a line that's too long or isn't UTF-8.
    #[error("unreadable line from the server")]
    Codec(#[from] LinesCodecError),
    /// Registration didn't finish in time.
    #[error("registration took longer than {0:?}")]
    Timeout(Duration),
    /// The server ended the connection during registration.
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
    /// A multiline message the server can't take, or doesn't take at all: this many bytes.
    #[error("message of {0} bytes is more than the server takes in one")]
    TooLong(usize),
    /// The line would break the protocol if sent, such as one with a line break in it.
    #[error("line can't be sent: {0}")]
    InvalidLine(&'static str),
    /// The connection has ended.
    #[error("not connected")]
    Disconnected,
}

impl From<registration::Error> for Error {
    fn from(err: registration::Error) -> Self {
        match err {
            registration::Error::Closed(reason) => Self::Closed(reason),
            registration::Error::BadPassword => Self::BadPassword,
            registration::Error::Sasl(reason) => Self::Sasl(reason),
            registration::Error::Nick(nick) => Self::Nick(nick),
        }
    }
}

/// Lines from the server after registration, in order, PINGs and PONGs excepted. Ends when the
/// connection does.
pub type Incoming = mpsc::Receiver<Line>;

/// A registered connection to send on. Cheap to clone; the connection stays open while a
/// `Client` or its [`Incoming`] is around, until the server ends it.
#[derive(Debug, Clone)]
pub struct Client {
    outgoing: mpsc::UnboundedSender<Line>,
    nick: Arc<str>,
    caps: Arc<BTreeSet<String>>,
    /// What the server takes in one multiline message, when it enabled `draft/multiline`.
    multiline: Option<multiline::Limits>,
    /// Names this client's multiline batches apart.
    batches: Arc<std::sync::atomic::AtomicU64>,
    /// Asks the reader to make sure the server is still there.
    probes: mpsc::UnboundedSender<Duration>,
    /// Why the connection ended, once it has.
    ended: Arc<OnceLock<String>>,
}

impl Client {
    /// The nick the server gave us, which may differ from the one asked for.
    #[must_use]
    pub fn nick(&self) -> &str {
        &self.nick
    }

    /// Whether the server enabled `cap`.
    #[must_use]
    pub fn has_cap(&self, cap: &str) -> bool {
        self.caps.contains(cap)
    }

    /// Queues `line` to be sent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidLine`] for a line that would break the protocol, and
    /// [`Error::Disconnected`] once the connection has ended.
    pub fn send(&self, line: Line) -> Result<(), Error> {
        check_sendable(&line)?;
        self.outgoing.send(line).map_err(|_| Error::Disconnected)
    }

    /// Joins `channel`.
    ///
    /// # Errors
    ///
    /// As [`Client::send`].
    pub fn join(&self, channel: &str) -> Result<(), Error> {
        self.send(Line::new("JOIN", [channel]))
    }

    /// Sends `text` to a channel or nick.
    ///
    /// # Errors
    ///
    /// As [`Client::send`].
    pub fn privmsg(&self, target: &str, text: &str) -> Result<(), Error> {
        self.send(Line::new("PRIVMSG", [target, text]))
    }

    /// Sends `text` to a channel or nick with `tags`, such as `+draft/reply`.
    ///
    /// # Errors
    ///
    /// As [`Client::send`].
    pub fn privmsg_tagged(
        &self,
        target: &str,
        text: &str,
        tags: Vec<(String, Option<String>)>,
    ) -> Result<(), Error> {
        let mut line = Line::new("PRIVMSG", [target, text]);
        line.tags = tags;
        self.send(line)
    }

    /// What the server takes in one multiline message, or `None` when it didn't enable
    /// `draft/multiline`.
    #[must_use]
    pub fn multiline(&self) -> Option<multiline::Limits> {
        self.multiline
    }

    /// Sends `text`, which may have line breaks or be longer than a line, to a channel or nick
    /// as one multiline message. See [`multiline`].
    ///
    /// # Errors
    ///
    /// [`Error::TooLong`] when the server didn't enable `draft/multiline` or `text` is more
    /// than it takes; otherwise as [`Client::send`].
    pub fn privmsg_multiline(&self, target: &str, text: &str) -> Result<(), Error> {
        self.privmsg_multiline_tagged(target, text, Vec::new())
    }

    /// The same, with `tags` on the batch's opening line, as `+draft/reply` goes for the
    /// whole message.
    ///
    /// # Errors
    ///
    /// As [`Client::privmsg_multiline`].
    pub fn privmsg_multiline_tagged(
        &self,
        target: &str,
        text: &str,
        tags: Vec<(String, Option<String>)>,
    ) -> Result<(), Error> {
        let n = self
            .batches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut lines = self
            .multiline
            .and_then(|limits| multiline::batch(&format!("ml{n}"), target, text, limits))
            .ok_or(Error::TooLong(text.len()))?;
        if let Some(first) = lines.first_mut() {
            first.tags.extend(tags);
        }
        for line in &lines {
            check_sendable(line)?;
        }
        for line in lines {
            self.outgoing.send(line).map_err(|_| Error::Disconnected)?;
        }
        Ok(())
    }

    /// Makes sure the server is still there: it's sent a `PING`, and unless it answers that
    /// within `within`, the connection counts as dead and [`Incoming`] ends. Only the answer
    /// counts: lines the server sent before it may still be arriving. For when the
    /// network may have changed under the connection, or the machine slept, and the
    /// keepalive would take long to notice.
    pub fn probe(&self, within: Duration) {
        let _ = self.probes.send(within);
    }

    /// Why the connection ended, once [`Incoming`] has: the server closed it, it failed, or
    /// the server stopped answering.
    #[must_use]
    pub fn ended(&self) -> Option<&str> {
        self.ended.get().map(String::as_str)
    }

    /// Leaves the server with `message`.
    ///
    /// # Errors
    ///
    /// As [`Client::send`].
    pub fn quit(&self, message: &str) -> Result<(), Error> {
        self.send(Line::new("QUIT", [message]))
    }
}

/// Connects to the server in `config` and registers.
///
/// # Errors
///
/// When the connection fails, the server refuses us, or registration takes too long.
pub async fn connect(config: &Config) -> Result<(Client, Incoming), Error> {
    match connect_once(config).await {
        // A device's token the server refused, as one that doesn't take device keys (any
        // more) does: the passphrase, where there is one.
        Err(Error::Sasl(reason)) if config.bearer.is_some() && config.sasl.is_some() => {
            tracing::debug!(%reason, "the device key was refused: the passphrase instead");
            let plain = Config {
                bearer: None,
                ..config.clone()
            };
            connect_once(&plain).await
        }
        other => other,
    }
}

async fn connect_once(config: &Config) -> Result<(Client, Incoming), Error> {
    if let Some(url) = &config.websocket {
        // Boxed, so a connection over TCP isn't as large a future.
        return Box::pin(crate::websocket::connect(url, config)).await;
    }
    let tcp = TcpStream::connect((config.host.as_str(), config.port)).await?;
    tcp.set_nodelay(true)?;
    if config.tls {
        register(
            tls_connector()?
                .connect(server_name(&config.host)?, tcp)
                .await?,
            config,
        )
        .await
    } else {
        register(tcp, config).await
    }
}

pub(crate) fn tls_connector() -> Result<TlsConnector, Error> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|err| Error::Tls(err.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

pub(crate) fn server_name(host: &str) -> Result<ServerName<'static>, Error> {
    ServerName::try_from(host.to_owned()).map_err(|err| Error::Tls(err.to_string()))
}

pub(crate) async fn register<S>(stream: S, config: &Config) -> Result<(Client, Incoming), Error>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = FramedRead::new(read, LinesCodec::new_with_max_length(MAX_LINE_BYTES));
    let mut registration = Registration::new(config.identity());

    for line in registration.opening() {
        write_line(&mut write, &line).await?;
    }
    let registered = tokio::time::timeout(REGISTRATION_TIMEOUT, async {
        loop {
            let Some(text) = lines.next().await.transpose()? else {
                return Err(Error::Closed("before registering".to_owned()));
            };
            let Ok(line) = text.parse::<Line>() else {
                continue;
            };
            match registration.step(&line)? {
                Step::Reply(replies) => {
                    for reply in replies {
                        write_line(&mut write, &reply).await?;
                    }
                }
                Step::Registered(registered) => return Ok(registered),
            }
        }
    })
    .await
    .map_err(|_| Error::Timeout(REGISTRATION_TIMEOUT))??;

    let (outgoing, mut queued) = mpsc::unbounded_channel::<Line>();
    let (deliver, incoming) = mpsc::channel(INCOMING_BUFFER);

    tokio::spawn(async move {
        while let Some(line) = queued.recv().await {
            if let Err(err) = write_line(&mut write, &line).await {
                tracing::debug!("writing failed, stopping: {err}");
                break;
            }
        }
    });

    // The reader answers PINGs through a weak sender, so it doesn't keep the writer alive
    // once every Client is gone.
    let (probes, asked) = mpsc::unbounded_channel::<Duration>();
    let ended = Arc::new(OnceLock::new());
    let reader = Reader {
        lines,
        writer: outgoing.downgrade(),
        asked,
        deliver,
        backlog: Backlog::default(),
        keepalive: config.keepalive,
        answer_within: config.answer_within,
    };
    let why = Arc::clone(&ended);
    tokio::spawn(async move {
        let _ = why.set(reader.run().await);
    });

    let client = Client {
        outgoing,
        nick: registered.nick.into(),
        caps: Arc::new(registered.caps),
        multiline: registered.multiline,
        batches: Arc::default(),
        probes,
        ended,
    };
    Ok((client, incoming))
}

/// What a probe's `PING` says, so its answer is told apart from others.
const PROBE: &str = "zirc-probe";

/// The connection's reading half, after registration.
struct Reader<S> {
    lines: FramedRead<ReadHalf<S>, LinesCodec>,
    /// Where PINGs and PONGs go, while a Client lives.
    writer: mpsc::WeakUnboundedSender<Line>,
    /// [`Client::probe`]s: how soon the server must answer.
    asked: mpsc::UnboundedReceiver<Duration>,
    deliver: mpsc::Sender<Line>,
    /// Lines read while [`Reader::deliver`] was full, oldest first.
    backlog: Backlog,
    keepalive: Duration,
    answer_within: Duration,
}

/// Lines waiting for room in [`Incoming`], and their size on the wire.
#[derive(Default)]
struct Backlog {
    lines: VecDeque<(Line, usize)>,
    bytes: usize,
}

impl<S: AsyncRead> Reader<S> {
    /// Passes lines on, answers PINGs and pings a quiet server, until the connection ends.
    /// Says why it did.
    async fn run(mut self) -> String {
        // When a PING is due or, once one is sent, when the server must have answered by.
        let mut deadline = Instant::now() + self.keepalive;
        let mut pinged: Option<Duration> = None;
        // Whether only the answer to a probe's PING will do.
        let mut probing = false;
        // When the last line came, for saying how long the server was quiet.
        let mut heard = Instant::now();
        loop {
            let text = tokio::select! {
                // What's held back goes first, so it doesn't pile up while lines keep coming.
                // Then lines: after a while this task couldn't run, what the server said
                // meanwhile, such as the answer to a PING, counts before the clock does.
                biased;
                permit = self.deliver.reserve(), if !self.backlog.lines.is_empty() => {
                    match permit {
                        Ok(permit) => {
                            if let Some((line, bytes)) = self.backlog.lines.pop_front() {
                                self.backlog.bytes -= bytes;
                                permit.send(line);
                            }
                        }
                        // Nobody reads anymore.
                        Err(_) => self.backlog = Backlog::default(),
                    }
                    continue;
                }
                text = self.lines.next() => text,
                () = tokio::time::sleep_until(deadline) => {
                    let late = Instant::now().saturating_duration_since(deadline);
                    // Woken long after the deadline, this task couldn't run: the server may
                    // have had no chance to answer what was sent late, so it gets another.
                    if late > self.answer_within / 2 {
                        tracing::warn!("couldn't run for {late:?}; waiting on the server again");
                        deadline = Instant::now() + pinged.unwrap_or(self.keepalive);
                        continue;
                    }
                    if let Some(within) = pinged {
                        tracing::warn!(
                            "the server hasn't answered a PING in {within:?}, nor said anything \
                             for {:?}; disconnecting",
                            heard.elapsed()
                        );
                        return format!("the server didn't answer in {within:?}");
                    }
                    self.write(Line::new("PING", ["zirc"]));
                    pinged = Some(self.answer_within);
                    deadline = Instant::now() + self.answer_within;
                    continue;
                }
                Some(within) = self.asked.recv() => {
                    // Sooner than an answer already awaited, it brings that closer.
                    let soon = Instant::now() + within;
                    if pinged.is_none() || soon < deadline {
                        self.write(Line::new("PING", [PROBE]));
                        pinged = Some(within);
                        deadline = soon;
                    }
                    probing = true;
                    continue;
                }
            };
            let text = match text {
                Some(Ok(text)) => text,
                Some(Err(LinesCodecError::Io(err))) => {
                    return format!("the connection failed: {err}");
                }
                Some(Err(err)) => {
                    tracing::debug!("skipping a line: {err}");
                    continue;
                }
                None => return "the server closed the connection".to_owned(),
            };
            heard = Instant::now();
            let bytes = text.len();
            let line = text.parse::<Line>();
            let answered = line.as_ref().is_ok_and(|line| {
                line.command == "PONG" && line.params.last().is_some_and(|token| token == PROBE)
            });
            if !probing || answered {
                probing = false;
                pinged = None;
                deadline = Instant::now() + self.keepalive;
            }
            let Ok(line) = line else {
                tracing::debug!("skipping an unparseable line");
                continue;
            };
            tracing::trace!("<< {}", loggable(&line));
            match line.command.as_str() {
                "PING" => self.write(Line::new("PONG", line.params)),
                // The answer to our own keepalive PING.
                "PONG" => {}
                _ => {
                    if let Some(why) = self.pass_on(line, bytes) {
                        return why;
                    }
                }
            }
        }
    }

    /// Passes `line` on, or holds it back while [`Incoming`] is full. Says why the
    /// connection must end when too much is held back.
    fn pass_on(&mut self, line: Line, bytes: usize) -> Option<String> {
        if !self.backlog.lines.is_empty() {
            return self.hold(line, bytes);
        }
        match self.deliver.try_send(line) {
            Err(mpsc::error::TrySendError::Full(line)) => self.hold(line, bytes),
            // Nobody reading anymore is fine: keep answering PINGs while a Client lives.
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => None,
        }
    }

    /// Holds `line`, `bytes` long on the wire, back until [`Incoming`] has room.
    fn hold(&mut self, line: Line, bytes: usize) -> Option<String> {
        if self.deliver.is_closed() {
            self.backlog = Backlog::default();
            return None;
        }
        self.backlog.bytes += bytes;
        self.backlog.lines.push_back((line, bytes));
        if self.backlog.bytes > MAX_BACKLOG_BYTES {
            let lines = self.backlog.lines.len() + INCOMING_BUFFER;
            tracing::warn!("{lines} lines received and not yet read; disconnecting");
            return Some(format!("fell behind reading, {lines} lines unread"));
        }
        None
    }

    fn write(&self, line: Line) {
        if let Some(writer) = self.writer.upgrade() {
            let _ = writer.send(line);
        }
    }
}

async fn write_line<W: AsyncWrite>(write: &mut WriteHalf<W>, line: &Line) -> Result<(), Error> {
    tracing::trace!(">> {}", loggable(line));
    write.write_all(format!("{line}\r\n").as_bytes()).await?;
    write.flush().await?;
    Ok(())
}

/// Refuses lines whose text would end the line early or change its shape on the wire.
fn check_sendable(line: &Line) -> Result<(), Error> {
    let forbidden = |text: &str| text.contains(['\r', '\n', '\0']);
    if line.command.is_empty() || line.command.contains(' ') || forbidden(&line.command) {
        return Err(Error::InvalidLine("command must be one word"));
    }
    if line.params.iter().any(|param| forbidden(param)) {
        return Err(Error::InvalidLine(
            "parameters can't contain line breaks or NUL",
        ));
    }
    let middle = line.params.len().saturating_sub(1);
    if line.params[..middle]
        .iter()
        .any(|param| param.is_empty() || param.starts_with(':') || param.contains(' '))
    {
        return Err(Error::InvalidLine(
            "only the last parameter may be empty, start with ':' or contain spaces",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_with_its_config_as_sasl_plain() {
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.server_password = Some("gate".into());
        config.sasl = Some(Credentials {
            account: "acct".into(),
            password: "pw".into(),
        });
        let identity = config.identity();
        assert_eq!(identity.nick, "me");
        assert_eq!(identity.server_password.as_deref(), Some("gate"));
        assert!(matches!(
            identity.sasl,
            Some(Sasl::Plain { account, password }) if account == "acct" && password == "pw"
        ));
        assert_eq!(identity.caps, DEFAULT_CAPS);
    }

    #[test]
    fn a_password_maker_makes_the_password_at_each_connection() {
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.server_password = Some("gate".into());
        let made = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = Arc::clone(&made);
        config.password_maker = Some(Arc::new(move || {
            let n = counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(format!("grant proof-{n}"))
        }));
        assert_eq!(
            config.identity().server_password.as_deref(),
            Some("grant proof-0")
        );
        assert_eq!(
            config.identity().server_password.as_deref(),
            Some("grant proof-1")
        );
        // One that makes none leaves the password set.
        config.password_maker = Some(Arc::new(|| None));
        assert_eq!(config.identity().server_password.as_deref(), Some("gate"));
        assert!(!format!("{config:?}").contains("proof"));
    }

    #[test]
    fn refuses_lines_that_would_break_the_protocol() {
        let ok = Line::new("PRIVMSG", ["#api", "hello there"]);
        assert!(check_sendable(&ok).is_ok());
        for bad in [
            Line::new("PRIVMSG", ["#api", "two\r\nQUIT"]),
            Line::new("PRIVMSG", ["#a b", "x"]),
            Line::new("PRIVMSG", ["", "x"]),
            Line::new("PRIV MSG", ["x"]),
        ] {
            assert!(
                matches!(check_sendable(&bad), Err(Error::InvalidLine(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn says_a_cause_once() {
        let err = Error::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "Connection refused",
        ));
        assert_eq!(err.to_string(), "connection failed");
        let source = std::error::Error::source(&err).map(ToString::to_string);
        assert_eq!(source.as_deref(), Some("Connection refused"));
    }

    #[test]
    fn keeps_passwords_out_of_debug_output() {
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.server_password = Some("gate-secret".into());
        config.sasl = Some(Credentials {
            account: "me".into(),
            password: "sasl-secret".into(),
        });
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret"), "{debug}");
    }
    #[tokio::test]
    async fn ends_a_connection_the_server_stopped_answering() {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        let (ours, theirs) = tokio::io::duplex(4096);
        let (read, mut write) = tokio::io::split(theirs);
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.caps.clear();
        config.keepalive = Duration::from_millis(100);
        config.answer_within = Duration::from_millis(100);
        let server = tokio::spawn(async move {
            let mut read = BufReader::new(read).lines();
            write.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
            // Then it hangs, hearing everything and saying nothing.
            let mut heard = Vec::new();
            while let Ok(Some(line)) = read.next_line().await {
                heard.push(line);
            }
            heard
        });
        let (client, mut incoming) = register(ours, &config).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), incoming.recv()).await;
        assert!(matches!(ended, Ok(None)), "{ended:?}");
        drop(client);
        let heard = server.await.unwrap();
        assert!(heard.iter().any(|line| line == "PING zirc"), "{heard:?}");
    }

    #[tokio::test]
    async fn a_while_it_couldnt_run_isnt_the_servers_silence() {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        let (ours, theirs) = tokio::io::duplex(4096);
        let (read, mut write) = tokio::io::split(theirs);
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.caps.clear();
        config.keepalive = Duration::from_millis(100);
        config.answer_within = Duration::from_millis(100);
        let (pinged, mut was_pinged) = mpsc::unbounded_channel();
        let _server = tokio::spawn(async move {
            let mut read = BufReader::new(read).lines();
            write.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
            // It answers every PING, a moment later.
            while let Ok(Some(line)) = read.next_line().await {
                if let Some(token) = line.strip_prefix("PING ") {
                    let _ = pinged.send(());
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    let pong = format!(":srv PONG srv {token}\r\n");
                    write.write_all(pong.as_bytes()).await.unwrap();
                }
            }
        });
        let (_client, mut incoming) = register(ours, &config).await.unwrap();
        was_pinged.recv().await.unwrap();
        // As a busy machine does: nothing on this runtime runs for a while, the server's
        // answer included, while the PING waits for one.
        std::thread::sleep(Duration::from_millis(400));
        let alive = tokio::time::timeout(Duration::from_millis(500), incoming.recv()).await;
        assert!(alive.is_err(), "the connection ended: {alive:?}");
    }

    #[tokio::test]
    async fn only_the_answer_to_a_probe_counts() {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        let (ours, theirs) = tokio::io::duplex(4096);
        let (read, mut write) = tokio::io::split(theirs);
        let mut heard = BufReader::new(read).lines();
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.caps.clear();
        write.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
        let (client, mut incoming) = register(ours, &config).await.unwrap();
        let mut pinged = async || {
            while let Some(line) = heard.next_line().await.unwrap() {
                if line == "PING zirc-probe" {
                    return;
                }
            }
        };

        // Answered: the connection lives on past the probe's time.
        client.probe(Duration::from_millis(100));
        pinged().await;
        write
            .write_all(b":srv PONG srv zirc-probe\r\n")
            .await
            .unwrap();
        let quiet = tokio::time::timeout(Duration::from_millis(300), incoming.recv()).await;
        assert!(quiet.is_err(), "{quiet:?}");

        // Anything else isn't an answer, as it may have been sent before the PING arrived.
        client.probe(Duration::from_millis(100));
        pinged().await;
        write
            .write_all(b":srv NOTICE me :sent earlier\r\n")
            .await
            .unwrap();
        let earlier = incoming.recv().await.unwrap();
        assert_eq!(earlier.command, "NOTICE");
        let ended = tokio::time::timeout(Duration::from_secs(5), incoming.recv()).await;
        assert!(matches!(ended, Ok(None)), "{ended:?}");
    }

    #[tokio::test]
    async fn a_probe_ends_a_dead_connection_without_waiting_for_the_keepalive() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (_read, mut write) = tokio::io::split(theirs);
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.caps.clear();
        write.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
        let (client, mut incoming) = register(ours, &config).await.unwrap();
        assert_eq!(client.ended(), None);
        client.probe(Duration::from_millis(100));
        let ended = tokio::time::timeout(Duration::from_secs(5), incoming.recv()).await;
        assert!(matches!(ended, Ok(None)), "{ended:?}");
        assert_eq!(client.ended(), Some("the server didn't answer in 100ms"));
    }

    /// `count` NOTICEs, numbered, with `pad` bytes of text each.
    fn notices(count: usize, pad: usize) -> Vec<u8> {
        let text = "x".repeat(pad);
        (0..count)
            .flat_map(|n| format!(":srv NOTICE me :{n} {text}\r\n").into_bytes())
            .collect()
    }

    #[tokio::test]
    async fn answers_pings_while_nobody_reads() {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        let (ours, theirs) = tokio::io::duplex(65536);
        let (read, mut write) = tokio::io::split(theirs);
        let mut heard = BufReader::new(read).lines();
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.caps.clear();
        write.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
        let (_client, mut incoming) = register(ours, &config).await.unwrap();

        // Three times what Incoming holds, and nobody reads it, as when zircd is busy.
        let flood = 3 * INCOMING_BUFFER;
        write.write_all(&notices(flood, 10)).await.unwrap();
        write.write_all(b"PING :still-there\r\n").await.unwrap();
        let pong = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(line) = heard.next_line().await.unwrap() {
                if line.starts_with("PONG") {
                    return line;
                }
            }
            panic!("the connection ended");
        })
        .await;
        assert_eq!(pong.as_deref(), Ok("PONG still-there"));

        // Nothing held back is lost or reordered.
        for n in 0..flood {
            let line = incoming.recv().await.unwrap();
            assert!(line.params[1].starts_with(&format!("{n} ")), "{line:?}");
        }
    }

    #[tokio::test]
    async fn notices_a_quiet_server_while_nobody_reads() {
        let (ours, theirs) = tokio::io::duplex(65536);
        let (read, mut write) = tokio::io::split(theirs);
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.caps.clear();
        config.keepalive = Duration::from_millis(100);
        config.answer_within = Duration::from_millis(100);
        // Hears what's sent, and never answers.
        let _server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut read = read;
            let mut buffer = [0; 1024];
            while read.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
        });
        write.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
        let (client, _incoming) = register(ours, &config).await.unwrap();
        write
            .write_all(&notices(2 * INCOMING_BUFFER, 10))
            .await
            .unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(why) = client.ended() {
                    return why.to_owned();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert_eq!(ended.as_deref(), Ok("the server didn't answer in 100ms"));
    }

    #[tokio::test]
    async fn ends_the_connection_once_too_much_is_unread() {
        let (ours, theirs) = tokio::io::duplex(65536);
        let (_read, mut write) = tokio::io::split(theirs);
        let mut config = Config::new("irc.zirc.test", 6697, "me");
        config.caps.clear();
        write.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
        let (client, mut incoming) = register(ours, &config).await.unwrap();
        let pad = 8000;
        let count = INCOMING_BUFFER + MAX_BACKLOG_BYTES / pad + 10;
        // The reader stops reading once it gives up, so the rest may never be written.
        let _flood = tokio::spawn(async move { write.write_all(&notices(count, pad)).await });
        let mut read = 0;
        let ended = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(why) = client.ended() {
                    return why.to_owned();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(ended.starts_with("fell behind reading, "), "{ended}");
        // What was passed on before it gave up can still be read, then Incoming ends.
        while incoming.recv().await.is_some() {
            read += 1;
        }
        assert_eq!(read, INCOMING_BUFFER);
    }
}
