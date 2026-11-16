//! Keeping the connection to the server up, the same way in every app. A [`Supervisor`]
//! connects, notices quickly when the connection has died, connects again with a capped,
//! jittered backoff, and says where it is in a [`Status`] for the UI to show.
//!
//! A connection that dies without closing, as when a laptop sleeps or the Wi-Fi changes,
//! is noticed by the keepalive: a server silent for [`Policy::keepalive`] is pinged and must
//! answer within [`Policy::answer_within`]. The machine waking is noticed by itself, as the
//! wall clock jumping ahead of the monotonic one, which stops while asleep; an app that hears
//! of it, or of the network changing, from its OS says so through [`Nudge`]. Either way the
//! connection is probed at once, and a wait between attempts is cut short.
//!
//! What the connection carries is the app's business: hand [`Event::Connected`]'s client to
//! [`crate::chat::Chat::connected`], as [`link`] describes it, its lines to [`crate::chat::Chat::receive`], and
//! [`Event::Offline`] to [`crate::chat::Chat::disconnected`]. The chat joins its channels
//! again and catches up their history from the last message it has.

use std::collections::hash_map::RandomState;
use std::future::Future;
use std::hash::{BuildHasher as _, Hasher as _};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::Notify;
use tokio::time::{Instant, Interval, MissedTickBehavior, Sleep};
use zirc_irc::{Client, Config, Error, Incoming, Line};

use crate::chat::Link;
use crate::clock;

pub use zirc_chat::connection::{Backoff, Policy};

/// Where the connection is, for the UI: zirc-chat's, which the web client shares.
pub use zirc_chat::connection::{
    Banner, BannerKind, NO_NETWORK, QUIET_OFFLINE, SIGNED_OUT, Status,
};

/// What happened to the connection.
#[derive(Debug)]
pub enum Event {
    /// An attempt to connect started.
    Connecting,
    /// Connected and registered: send with [`Supervisor::client`].
    Connected(Client),
    /// A line from the server.
    Line(Line),
    /// The connection was lost, or an attempt failed, and why; another attempt follows.
    Offline(String),
    /// The server refused the account's login (SASL), and why: as [`Event::Offline`], another
    /// attempt follows, but it's the account that's wrong, not the network. A person's app
    /// asks zirc.dev what it is now ([`crate::signin::SignIn::renew`]), as after a handle
    /// change elsewhere, and connects with [`Supervisor::set_config`].
    Refused(String),
}

/// Tells the [`Supervisor`] that the network changed or the machine woke, from any thread:
/// a connection is probed and a wait between attempts ends now.
#[derive(Debug, Clone)]
pub struct Nudge(Arc<Notify>);

impl Nudge {
    /// The network changed, or the machine woke.
    pub fn now(&self) {
        self.0.notify_one();
    }

    /// A nudge no supervisor hears, and what it notifies: for a connection that's scripted,
    /// as the phones' fake network is (`zirc_app::net::scripted`).
    #[must_use]
    pub fn unsupervised() -> (Self, Arc<Notify>) {
        let notify = Arc::new(Notify::new());
        (Self(Arc::clone(&notify)), notify)
    }
}

type Attempt = Pin<Box<dyn Future<Output = Result<(Client, Incoming), Error>> + Send>>;

/// Makes an attempt to connect with a config. [`zirc_irc::connect`], but for tests.
pub type Connector = Box<dyn Fn(Config) -> Attempt + Send + Sync>;

/// What [`Supervisor::next`] waited for.
enum Happened {
    /// Time for an attempt.
    Due,
    /// An attempt finished, or ran out of time.
    Attempted(Result<Result<(Client, Incoming), Error>, tokio::time::error::Elapsed>),
    /// A line, or the end of the connection.
    Line(Option<Line>),
    /// The time to check for sleep came, and whether the machine slept; or a nudge.
    Woke(bool),
}

enum State {
    /// Not started: the first [`Supervisor::next`] connects.
    Idle,
    Waiting(Pin<Box<Sleep>>),
    Connecting(Attempt, Instant),
    Up {
        client: Client,
        incoming: Incoming,
        since: Instant,
    },
}

/// Keeps a connection up. Call [`Supervisor::next`] in a loop, or in `select!`, for what
/// happens.
pub struct Supervisor {
    config: Config,
    policy: Policy,
    connect: Connector,
    state: State,
    status: Status,
    /// Failed attempts since the last stable connection.
    failures: u32,
    nudged: Arc<Notify>,
    wake: Wake,
}

impl Supervisor {
    /// Keeps a connection with `config` up, with [`Policy::default`].
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self::with(
            config,
            Policy::default(),
            Box::new(|config| Box::pin(async move { zirc_irc::connect(&config).await })),
        )
    }

    /// Keeps a connection with `config` up, as `policy` says, connecting with `connect`.
    #[must_use]
    pub fn with(mut config: Config, policy: Policy, connect: Connector) -> Self {
        config.keepalive = policy.keepalive;
        config.answer_within = policy.answer_within;
        Self {
            config,
            policy,
            connect,
            state: State::Idle,
            status: Status::Connecting {
                attempt: 1,
                was_up: false,
            },
            failures: 0,
            nudged: Arc::new(Notify::new()),
            wake: Wake::new(policy.wake_check),
        }
    }

    /// Where the connection is.
    #[must_use]
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The connection, while there is one.
    #[must_use]
    pub fn client(&self) -> Option<&Client> {
        match &self.state {
            State::Up { client, .. } => Some(client),
            _ => None,
        }
    }

    /// Something to tell this supervisor that the network changed or the machine woke.
    #[must_use]
    pub fn nudge(&self) -> Nudge {
        Nudge(Arc::clone(&self.nudged))
    }

    /// The network changed, or the machine woke: as [`Nudge::now`].
    pub fn nudged(&self) {
        self.nudged.notify_one();
    }

    /// Connects once, before anything else, for an app that says plainly when the first
    /// connection fails, as with a wrong server or password, rather than retrying it. From
    /// then on, [`Supervisor::next`] keeps it up.
    ///
    /// # Errors
    ///
    /// When connecting fails or takes longer than [`Policy::connect_within`].
    pub async fn first(&mut self) -> Result<Client, Error> {
        let attempt = (self.connect)(self.config.clone());
        let (client, incoming) = tokio::time::timeout(self.policy.connect_within, attempt)
            .await
            .map_err(|_| Error::Timeout(self.policy.connect_within))??;
        let Event::Connected(client) = self.up(client, incoming) else {
            unreachable!("up is connected");
        };
        Ok(client)
    }

    /// A line that's already arrived, without waiting: to take in what came together
    /// before drawing again.
    pub fn try_line(&mut self) -> Option<Line> {
        match &mut self.state {
            State::Up { incoming, .. } => incoming.try_recv().ok(),
            _ => None,
        }
    }

    /// Waits for what happens next. Safe to cancel, as in `select!`.
    pub async fn next(&mut self) -> Event {
        loop {
            let connect_within = self.policy.connect_within;
            let happened = match &mut self.state {
                State::Idle => Happened::Due,
                State::Waiting(sleep) => tokio::select! {
                    () = sleep.as_mut() => Happened::Due,
                    () = self.nudged.notified() => Happened::Woke(true),
                    woke = self.wake.tick() => Happened::Woke(woke),
                },
                State::Connecting(attempt, deadline) => tokio::select! {
                    result = tokio::time::timeout_at(*deadline, attempt.as_mut()) => {
                        Happened::Attempted(result)
                    }
                    () = self.nudged.notified() => Happened::Woke(true),
                    woke = self.wake.tick() => Happened::Woke(woke),
                },
                State::Up { incoming, .. } => tokio::select! {
                    line = incoming.recv() => Happened::Line(line),
                    () = self.nudged.notified() => Happened::Woke(true),
                    woke = self.wake.tick() => Happened::Woke(woke),
                },
            };
            match happened {
                Happened::Due => return self.attempt(),
                Happened::Attempted(Ok(Ok((client, incoming)))) => {
                    return self.up(client, incoming);
                }
                Happened::Attempted(Ok(Err(err))) => {
                    self.failed(crate::telemetry::failure(&err));
                    let event = self.down(&format!("couldn't connect: {}", Chain(&err)), None);
                    return match (err, event) {
                        (Error::Sasl(_), Event::Offline(why)) => Event::Refused(why),
                        (_, event) => event,
                    };
                }
                Happened::Attempted(Err(_)) => {
                    self.failed(zirc_proto::telemetry::Failure::Timeout);
                    let why = format!("couldn't connect in {connect_within:?}");
                    return self.down(&why, None);
                }
                Happened::Line(Some(line)) => return Event::Line(line),
                Happened::Line(None) => {
                    let State::Up { client, since, .. } = &self.state else {
                        unreachable!("lines come only while up");
                    };
                    let why = client
                        .ended()
                        .unwrap_or("the connection was lost")
                        .to_owned();
                    let since = *since;
                    return self.down(&why, Some(since));
                }
                Happened::Woke(false) => {}
                Happened::Woke(true) => {
                    tracing::info!("the network changed or the machine woke");
                    self.failures = 0;
                    match &self.state {
                        State::Up { client, .. } => client.probe(self.policy.probe_within),
                        // What was being tried on the old network may never finish.
                        State::Connecting(..) | State::Waiting(_) => return self.attempt(),
                        State::Idle => {}
                    }
                }
            }
        }
    }

    /// What it connects with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Connects with `config` from the next attempt on, as after the person's handle changed
    /// ([`Event::Refused`]), and makes that attempt now. A connection that's up stays as it is.
    pub fn set_config(&mut self, mut config: Config) {
        config.keepalive = self.config.keepalive;
        config.answer_within = self.config.answer_within;
        self.config = config;
        self.failures = 0;
        self.nudged.notify_one();
    }

    /// Waits up to `grace` for the server to close the connection, as after a `QUIT`, so
    /// it's seen as leaving rather than timing out.
    pub async fn finish(mut self, grace: Duration) {
        if let State::Up { incoming, .. } = &mut self.state {
            let _ = tokio::time::timeout(grace, async { while incoming.recv().await.is_some() {} })
                .await;
        }
    }

    fn attempt(&mut self) -> Event {
        self.state = State::Connecting(
            (self.connect)(self.config.clone()),
            Instant::now() + self.policy.connect_within,
        );
        self.status = Status::Connecting {
            attempt: self.failures + 1,
            was_up: self.status.was_up(),
        };
        Event::Connecting
    }

    fn up(&mut self, client: Client, incoming: Incoming) -> Event {
        tracing::info!("connected to {} as {}", self.config.host, client.nick());
        self.status = Status::Connected {
            since: clock::now(),
        };
        self.state = State::Up {
            client: client.clone(),
            incoming,
            since: Instant::now(),
        };
        Event::Connected(client)
    }

    /// Counts an attempt that failed for `reason`, if it's the first since the connection
    /// was last stable.
    fn failed(&self, reason: zirc_proto::telemetry::Failure) {
        if self.failures == 0 {
            crate::telemetry::record(&crate::telemetry::connection_failed(reason));
        }
    }

    /// The connection, up since `since` if it was, ended for `why`.
    fn down(&mut self, why: &str, since: Option<Instant>) -> Event {
        tracing::warn!("offline: {why}");
        if since.is_some_and(|since| since.elapsed() >= self.policy.stable) {
            self.failures = 0;
        }
        let wait = self.backoff();
        self.failures = self.failures.saturating_add(1);
        let lost = match &self.status {
            Status::Offline { since, .. } => *since,
            _ => clock::now(),
        };
        self.status = Status::Offline {
            since: lost,
            why: why.to_owned(),
            retry_at: clock::now() + wait,
            was_up: self.status.was_up(),
        };
        self.state = State::Waiting(Box::pin(tokio::time::sleep(wait)));
        Event::Offline(why.to_owned())
    }

    /// The wait before the next attempt, as [`Policy::backoff`] says, at random.
    fn backoff(&self) -> Duration {
        self.policy.backoff(self.failures, random())
    }
}

/// A number from 0 up to 1, at random: the `fraction` [`Policy::backoff`] takes.
#[must_use]
pub fn random() -> f64 {
    let mut hasher = RandomState::new().build_hasher();
    // Each RandomState has keys of its own, so the same input hashes differently.
    hasher.write_u8(0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "53 bits, which a double holds exactly"
    )]
    let fraction = (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64;
    fraction
}

/// Notices the machine waking from sleep: the monotonic clock stops while it sleeps, and
/// the wall clock doesn't.
struct Wake {
    interval: Interval,
    last: (Instant, SystemTime),
    gap: Duration,
}

impl Wake {
    fn new(every: Duration) -> Self {
        let mut interval = tokio::time::interval_at(Instant::now() + every, every);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            interval,
            last: (Instant::now(), SystemTime::now()),
            gap: every * 2,
        }
    }

    /// Waits for the next check, saying whether the machine slept since the last.
    async fn tick(&mut self) -> bool {
        self.interval.tick().await;
        let now = (Instant::now(), SystemTime::now());
        let monotonic = now.0 - self.last.0;
        let wall = now.1.duration_since(self.last.1).unwrap_or_default();
        self.last = now;
        wall > monotonic + self.gap
    }
}

/// An error and its causes, for `{:#}`, as anyhow prints them.
struct Chain<'a>(&'a Error);

impl std::fmt::Display for Chain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = std::error::Error::source(self.0);
        while let Some(cause) = source {
            write!(f, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

/// What an app that keeps a [`crate::chat::Chat`] asks for on top of zirc-irc's
/// capabilities, as zirc-chat declares them for every app.
pub use zirc_chat::chat::APP_CAPS;

/// What `client`'s connection allows, for [`crate::chat::Chat::connected`].
#[must_use]
pub fn link(client: &Client) -> Link {
    Link::new(client.nick(), |cap| client.has_cap(cap), client.multiline())
}

#[cfg(test)]
mod tests;
