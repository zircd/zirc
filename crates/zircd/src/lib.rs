//! zircd: the per-machine daemon that holds zirc's IRC connections, one per organisation
//! the machine is signed in to, watches repositories for edits, commits and pushes, and
//! serves the `zirc` CLI over a Unix socket.

mod cli;
mod clock;
mod config;
mod daemon;
mod git;
mod irc;
mod left;
mod log;
mod procs;
mod room;
mod router;
mod socket;
mod update;

use std::future::Future;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;
use zirc_core::settings::Settings;
use zirc_core::telemetry::{self, Telemetry};
use zirc_core::{paths, settings};
use zirc_proto::telemetry::{AppOpened, Client, Event};

pub use crate::cli::Cli;
pub use crate::clock::Clock;
use crate::daemon::{Daemon, Input, Org};
use crate::router::{Call, Router};

/// How long a restarting zircd waits for the requests it's answering.
const RESTART_SETTLE: std::time::Duration = std::time::Duration::from_secs(10);

/// Tells the router the release `updated` gives was installed, as the daily check does.
async fn updated_by(
    updated: tokio::sync::oneshot::Receiver<zirc_core::update::Version>,
    calls: mpsc::UnboundedSender<Call>,
) {
    if let Ok(version) = updated.await {
        let _ = calls.send(Call::Updated(version));
    }
}

/// Before a restart, answers the requests already `called`, rather than drop them: what's
/// waiting gets the old zircd's answer.
async fn answer_waiting(router: &mut Router, called: &mut mpsc::UnboundedReceiver<Call>) {
    while let Ok(call) = called.try_recv() {
        if let Call::Request(..) = call {
            router.handle(call);
        }
    }
    router.settle(RESTART_SETTLE).await;
}

/// How [`run`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Stopped, by `shutdown` or a client.
    Stopped,
    /// To start again from its binary, which may be a new release's: zircd's `main` does.
    Restart,
}

/// For each of `orgs` in `config.toml`'s `settings`, the guest's terminal it is, if it is one.
fn guests_of(settings: &Settings, orgs: &[String]) -> Vec<Option<daemon::Guest>> {
    orgs.iter()
        .enumerate()
        .map(|(at, org)| {
            let guest = config::find(settings, org, at == 0).and_then(|org| org.guest);
            guest.map(|guest| daemon::Guest {
                room: guest.channel.clone(),
                of: zirc_core::guest::of(&guest),
            })
        })
        .collect()
}

/// Whether the machine is signed out of everything, `config.toml` gone, as when the last of
/// its organisations was a guest's terminal zirc.dev forgot. zircd then stops, so a service
/// manager that runs it at login leaves it stopped rather than starting it again and again.
fn nothing_to_connect_to(config: &Path) -> bool {
    let gone = !config.exists();
    if gone {
        tracing::info!("{} is gone: nothing to connect to", config.display());
    }
    gone
}

/// Runs the daemon with its files in `home` until `shutdown` completes or a client asks
/// it to stop or restart. Installed by zirc's installer, it updates itself about once a
/// day unless `config.toml` says `auto_update = false`, and then ends to restart.
///
/// # Errors
///
/// When `config.toml` can't be read, or another zircd already serves `home`.
pub async fn run(home: PathBuf, shutdown: impl Future<Output = ()>) -> Result<Ended> {
    run_with(home, shutdown, Hooks::default()).await
}

/// What a test stands in for, as [`run_with`] takes it, rather than wait for the real thing.
#[derive(Default)]
pub struct Hooks {
    /// Says a release was installed, as the daily check does once it has installed one: zircd
    /// then restarts as after an update.
    pub updated: Option<tokio::sync::oneshot::Receiver<zirc_core::update::Version>>,
    /// The clock zircd's long windows are counted by, which the test can move ahead.
    pub clock: Clock,
    /// How long a request waits on the server before it fails, in place of 15 seconds.
    pub ask_wait: Option<std::time::Duration>,
}

/// [`run`], with `hooks` in place of the update check and the real clock.
///
/// # Errors
///
/// As [`run`].
pub async fn run_with(
    home: PathBuf,
    shutdown: impl Future<Output = ()>,
    hooks: Hooks,
) -> Result<Ended> {
    std::fs::create_dir_all(&home)?;
    // The directory holds passwords and the socket: for its owner only.
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    // Held until `run` returns. Rust opens files close-on-exec, so a restart's new binary
    // takes it again.
    let _lock = socket::lock(&home).await?;
    let config_path = paths::config(&home);
    if nothing_to_connect_to(&config_path) {
        return Ok(Ended::Stopped);
    }
    // Read once, for all that's decided at start.
    let settings = config::read(&config_path)?;
    let orgs = config::organisations(&settings, &config_path)?;
    // What it kept of organisations it's no longer in, as one it just left, goes.
    left::forget(&home, &orgs);
    let mut configs = Vec::new();
    for (at, org) in orgs.iter().enumerate() {
        configs.push(config::load_in(&settings, &config_path, org, at == 0)?);
    }
    let socket_path = paths::socket(&home);
    let listener = socket::bind(&socket_path).await?;

    let (calls, mut called) = mpsc::unbounded_channel::<Call>();
    // The connections and the socket live exactly as long as this future: dropping the
    // set, however `run` ends, aborts them.
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(socket::serve(listener, calls.clone()));
    tasks.spawn(log::keep_small(home.clone()));
    // zircd sends the analytics the CLI queues, as well as its own.
    let telemetry = telemetry::init(Telemetry::load(&home, Client::Zircd));
    telemetry.record(&Event::AppOpened(AppOpened {}));
    tasks.spawn(telemetry::send_every(telemetry, telemetry::EVERY));
    if config::auto_update(&settings) {
        tasks.spawn(update::check_daily(home.clone(), calls.clone()));
    }
    if let Some(updated) = hooks.updated {
        tasks.spawn(updated_by(updated, calls.clone()));
    }

    // A daemon and a connection per organisation.
    let announce_every = config::announce_every(&settings);
    let request_hold = config::request_hold(&settings);
    let wake_limits = config::wake_limits(&settings);
    // Guests' terminals aside: a machine's repositories say whose they are only when it's in
    // several organisations of its own.
    let guests = guests_of(&settings, &orgs);
    let several = guests.iter().filter(|guest| guest.is_none()).count() > 1;
    let book = daemon::book::Shared::load(&home, daemon::now());
    let mut router = Router::new();
    let mut daemons = tokio::task::JoinSet::new();
    for (at, ((name, config), guest)) in orgs.into_iter().zip(configs).zip(guests).enumerate() {
        let (inputs, received) = mpsc::unbounded_channel::<Input>();
        tasks.spawn(irc::supervise(
            config_path.clone(),
            name.clone(),
            at == 0,
            inputs.clone(),
            calls.clone(),
        ));
        let org = Org {
            first: at == 0,
            several,
            state: if at == 0 {
                home.clone()
            } else {
                paths::org(&home, &name)
            },
            name: name.clone(),
            guest: guest.clone(),
        };
        std::fs::create_dir_all(&org.state)?;
        let server = format!("{}:{}", config.host, config.port);
        // The machine's real name is its owner's handle, when it has an owner.
        let owner = (config.realname != config.nick).then(|| config.realname.clone());
        let mut daemon = Daemon::new(home.clone(), org, server, owner, inputs.clone());
        daemon.announce_every = announce_every;
        daemon.request_hold = request_hold;
        daemon.breaker = daemon::breaker::Breaker::with_limits(wake_limits.0, wake_limits.1);
        daemon.clock = hooks.clock.clone();
        // One book of what sessions go by, whichever organisation's repository they work in.
        daemon.book = book.clone();
        if let Some(wait) = hooks.ask_wait {
            daemon.ask_wait = wait;
        }
        daemons.spawn(daemon::run(daemon, received));
        // zirc.dev's public server, by the name every client gives it, watches no repositories.
        let public = name == settings::PUBLIC_ORG;
        router.add(name, public, guest.is_some(), inputs);
    }

    tokio::pin!(shutdown);
    while !router.stopping {
        tokio::select! {
            Some(call) = called.recv() => router.handle(call),
            () = router.answered() => {}
            () = &mut shutdown => break,
        }
    }

    if router.restarting {
        answer_waiting(&mut router, &mut called).await;
    }
    router.stop();
    while daemons.join_next().await.is_some() {}
    tasks.shutdown().await;
    if router.restarting {
        // No longer listening, so a watch can't watch this one again: long enough for the
        // socket's clients to send what they were given, such as watches being told it's
        // restarting.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let _ = std::fs::remove_file(socket_path);
    Ok(if router.restarting {
        Ended::Restart
    } else {
        Ended::Stopped
    })
}
