//! Keeps one organisation's IRC connection up: connects, passes everything on to its
//! daemon, and when the connection ends, waits and connects again, backing off from one
//! second to a minute, each wait cut by up to half at random so machines that lost the
//! server together don't come back together. The backoff starts over only once a
//! connection has lasted a minute: a server that takes connections and drops them at once
//! isn't retried every second. Each attempt reads `config.toml` afresh, so new credentials
//! take effect on the next.
//!
//! The waits are zirc-core's [`Backoff`], as every app's are. zircd keeps a loop of its own
//! rather than zirc-core's `Supervisor`, which keeps one config and never stops trying:
//! zircd reads its config afresh for each attempt, renews credentials and guests' grants
//! between them, and stops altogether while the web app refuses the machine.
//!
//! When the server refuses the machine's account, as after its owner's handle changed, zircd
//! asks the web app the machine logged in at for its current credentials (see
//! [`zirc_proto::login`]), saves them, and tries again at once.
//!
//! A guest's terminal renews its grant there instead ([`zirc_proto::guest_terminal`]): hours
//! before it ends, and when the gateway refuses it. Once the web app lets it in no more, as
//! when its guest was banned or forgotten, it stops trying until `config.toml` changes.

use crate::clock::unix as now;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
use zirc_core::connection::{self, Backoff, Policy};
use zirc_core::ipc;
use zirc_core::login::Client;
use zirc_core::settings::{self, Server, Settings};
use zirc_core::telemetry;
use zirc_proto::telemetry::Failure;

use crate::config;
use crate::daemon::Input;
use crate::router::Call;

/// How it backs off, and how long an attempt may take: a TCP connect to a vanished host can
/// hang for minutes. The rest of a [`Policy`] is the `Supervisor`'s, which zircd doesn't use.
fn policy() -> Policy {
    Policy {
        first_retry: Duration::from_secs(1),
        max_retry: Duration::from_secs(60),
        connect_within: Duration::from_secs(45),
        stable: Duration::from_secs(60),
        ..Policy::default()
    }
}
/// How often a machine whose account is gone checks whether it's been logged in again.
const CONFIG_CHECK: Duration = Duration::from_secs(2);

/// Counts a failure to connect, unless the attempt before failed too.
fn failed(failing: &mut bool, reason: Failure) {
    if !std::mem::replace(failing, true) {
        telemetry::record(&telemetry::connection_failed(reason));
    }
}

/// Runs until the daemon goes away, connecting to the organisation `org`, the machine's
/// `first` or not, as `config.toml` at `path` says.
pub async fn supervise(
    path: PathBuf,
    org: String,
    first: bool,
    inputs: UnboundedSender<Input>,
    calls: UnboundedSender<Call>,
) {
    supervise_with(policy(), path, org, first, inputs, calls).await;
}

/// As [`supervise`], backing off as `policy` says.
async fn supervise_with(
    policy: Policy,
    path: PathBuf,
    org: String,
    first: bool,
    inputs: UnboundedSender<Input>,
    calls: UnboundedSender<Call>,
) {
    let started = Instant::now();
    let clock = || started.elapsed();
    let mut backoff = Backoff::default();
    // Whether the last attempt failed too: only the first failure in a row is counted.
    let mut failing = false;
    // Whether a guest's terminal's grant was renewed since it last connected: a refusal
    // then isn't the grant's, so it isn't renewed again for it.
    let mut renewed = false;
    loop {
        if let Some(guest) = config::guest(&path, &org, first)
            && zirc_core::guest::renew_due(&guest, now())
        {
            match renew(&path, &org, first).await {
                Renewal::New => renewed = true,
                Renewal::Gone => {
                    stop_guest(&path, &org, &guest, &inputs, &calls).await;
                    continue;
                }
                Renewal::Not => {}
            }
        }
        let config = match config::load(&path, &org, first) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!("{err:#}");
                tokio::time::sleep(policy.max_retry).await;
                continue;
            }
        };
        match tokio::time::timeout(policy.connect_within, zirc_irc::connect(&config)).await {
            Ok(Ok((client, mut incoming))) => {
                tracing::info!(nick = client.nick(), "connected to {}", config.host);
                backoff.connected(clock());
                failing = false;
                renewed = false;
                if inputs.send(Input::Connected(client)).is_err() {
                    return;
                }
                if !pass_on(&mut incoming, &inputs).await {
                    return;
                }
                tracing::warn!("disconnected from {}", config.host);
                if inputs.send(Input::Disconnected).is_err() {
                    return;
                }
            }
            Ok(Err(zirc_irc::Error::Sasl(reason))) => {
                tracing::warn!("{} refused this machine's account: {reason}", config.host);
                match refresh(&path, &org, first).await {
                    Refreshed::New => {
                        backoff = Backoff::default();
                        continue;
                    }
                    Refreshed::Revoked => {
                        // Retrying can't help: only logging in again can, which rewrites
                        // config.toml and restarts zircd. Wait for that.
                        tracing::warn!("{}", ipc::REVOKED);
                        if inputs.send(Input::Revoked).is_err() {
                            return;
                        }
                        config_changed(&path).await;
                        backoff = Backoff::default();
                        continue;
                    }
                    Refreshed::Not => failed(&mut failing, Failure::Sasl),
                }
            }
            // The gateway refused a guest's terminal: maybe its grant, which is renewed.
            Ok(Err(zirc_irc::Error::Closed(reason))) if config.websocket.is_some() && !renewed => {
                tracing::warn!("{} refused this guest's terminal: {reason}", config.host);
                match renew(&path, &org, first).await {
                    Renewal::New => {
                        renewed = true;
                        continue;
                    }
                    Renewal::Gone => {
                        if let Some(guest) = config::guest(&path, &org, first) {
                            stop_guest(&path, &org, &guest, &inputs, &calls).await;
                        }
                        continue;
                    }
                    Renewal::Not => failed(&mut failing, Failure::Closed),
                }
            }
            Ok(Err(err)) => {
                failed(&mut failing, telemetry::failure(&err));
                tracing::warn!(
                    "connecting to {} failed: {:#}",
                    config.host,
                    anyhow::Error::from(err)
                );
            }
            Err(_) => {
                failed(&mut failing, Failure::Timeout);
                tracing::warn!("connecting to {} timed out", config.host);
            }
        }
        let wait = backoff.lost(&policy, clock(), connection::random());
        tracing::debug!("connecting to {} again in {wait:?}", config.host);
        tokio::time::sleep(wait).await;
    }
}

/// Passes the connection's lines on to the daemon until it ends. Says whether the daemon is
/// still there.
async fn pass_on(incoming: &mut zirc_irc::Incoming, inputs: &UnboundedSender<Input>) -> bool {
    // A multiline message reaches the daemon as one line.
    let mut batches = zirc_irc::multiline::Batches::default();
    while let Some(line) = incoming.recv().await {
        let Some(line) = batches.feed(line) else {
            continue;
        };
        if inputs.send(Input::Line(line)).is_err() {
            return false;
        }
    }
    true
}

/// What renewing a guest's terminal's grant came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Renewal {
    /// A new grant, now saved.
    New,
    /// The web app lets it in no more.
    Gone,
    /// Nothing changed, for a reason that's logged, such as the web app away.
    Not,
}

/// What a guest's terminal the web app lets in no more says: that it's gone from this machine.
pub fn gone(org: &str, guest: &settings::GuestCredentials) -> String {
    format!(
        "{org}: the web app lets this guest's terminal into {} no more ({} was banned or \
         forgotten, or the channel takes terminals no more), so it's gone from this machine",
        guest.public, guest.guest
    )
}

/// The guest is gone: takes their terminal off this machine, as `zirc logout` would, and
/// has zircd start again without it, or stop once nothing's left. Should
/// `config.toml` not change, it waits until it does, as when `zirc login` gives it a new grant.
async fn stop_guest(
    path: &Path,
    org: &str,
    guest: &settings::GuestCredentials,
    inputs: &UnboundedSender<Input>,
    calls: &UnboundedSender<Call>,
) {
    let message = gone(org, guest);
    tracing::warn!("{message}");
    match forget(path, org) {
        Ok(()) => {
            let _ = calls.send(Call::Forgotten(message));
        }
        Err(err) => {
            tracing::warn!("{org}: couldn't take the guest's terminal off this machine: {err:#}");
            let _ = inputs.send(Input::Refused(message));
        }
    }
    config_changed(path).await;
}

/// Takes the organisation `org` out of `config.toml` at `path`, as `zirc logout` does.
fn forget(path: &Path, org: &str) -> anyhow::Result<()> {
    let home = path.parent().unwrap_or(path);
    let mut settings =
        settings(path).ok_or_else(|| anyhow::anyhow!("reading {}", path.display()))?;
    let plan =
        zirc_core::signout::Plan::new(&settings, &zirc_core::signout::Scope::One(org.to_owned()))?;
    zirc_core::signout::forget(home, path, &mut settings, &plan)?;
    Ok(())
}

/// Renews the grant of the guest's terminal `org` (the `first` or not) at its web app, and
/// saves the new one.
async fn renew(path: &Path, org: &str, first: bool) -> Renewal {
    let _refreshing = REFRESHING.lock().await;
    let Some(organisation) = settings(path).and_then(|it| config::find(&it, org, first)) else {
        return Renewal::Not;
    };
    let (Some(guest), Some(web)) = (organisation.guest.clone(), organisation.web.clone()) else {
        return Renewal::Not;
    };
    let home = path.parent().unwrap_or(Path::new("."));
    let key = zirc_core::device_key::Software::machine(home);
    let Some(request) = zirc_core::guest::renew_request(&key, &guest) else {
        tracing::warn!("{org}: this machine's key can't sign, or the grant isn't one");
        return Renewal::Not;
    };
    let answer = match Client::new(&web) {
        Ok(client) => client.renew_guest(&request).await,
        Err(err) => Err(err),
    };
    let renewed = match answer {
        Ok(Some(renewed)) => renewed,
        Ok(None) => return Renewal::Gone,
        Err(err) => {
            tracing::warn!("{org}: renewing the guest's grant at {web}: {err:#}");
            return Renewal::Not;
        }
    };
    // As the file is now, which `zirc login` or `logout` may have changed meanwhile.
    let Some(mut settings) = settings(path) else {
        return Renewal::Not;
    };
    let Some(mut organisation) = config::find(&settings, org, first) else {
        return Renewal::Not;
    };
    if organisation.guest.as_ref().map(|it| &it.grant) != Some(&guest.grant) {
        return Renewal::Not;
    }
    let name = organisation.name();
    organisation.guest = Some(renewed);
    settings.set_organisation(&name, organisation);
    match settings.save(path) {
        Ok(()) => {
            tracing::info!("{org}: renewed the guest's grant");
            Renewal::New
        }
        Err(err) => {
            tracing::warn!("saving {org}'s renewed grant to {}: {err}", path.display());
            Renewal::Not
        }
    }
}

/// What asking the web app for new credentials came to.
enum Refreshed {
    /// It had new ones, now saved.
    New,
    /// It no longer takes this machine's account: renamed or revoked.
    Revoked,
    /// Nothing changed, for a reason that's logged: there's no account or web app to ask,
    /// the web app can't be reached, or it has nothing newer.
    Not,
}

/// Waits until `path` reads differently from now, as when `zirc login` rewrites it.
async fn config_changed(path: &Path) {
    let before = std::fs::read(path).ok();
    loop {
        tokio::time::sleep(CONFIG_CHECK).await;
        if std::fs::read(path).ok() != before {
            return;
        }
    }
}

/// One organisation's credentials are asked for and saved at a time, so two refused at once
/// don't save over each other.
static REFRESHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether `a` and `b` are the same server: the same host and port.
fn same_server(a: &Server, b: &Server) -> bool {
    let port = |server: &Server| server.port.unwrap_or(settings::DEFAULT_PORT);
    a.host == b.host && port(a) == port(b)
}

/// Reads `config.toml` at `path`.
fn settings(path: &Path) -> Option<Settings> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| Settings::from_toml(&text).ok())
}

/// Asks the web app for the machine's current credentials in `org` (the `first` or not) and
/// saves them. On a machine in several organisations, credentials for another server are
/// refused: that would send this organisation's repositories to another's server.
async fn refresh(path: &Path, org: &str, first: bool) -> Refreshed {
    let _refreshing = REFRESHING.lock().await;
    let Some(organisation) = settings(path).and_then(|it| config::find(&it, org, first)) else {
        return Refreshed::Not;
    };
    let (Some(account), Some(web)) = (organisation.account.clone(), organisation.web.clone())
    else {
        tracing::warn!("no web app to ask for new credentials: run `zirc login` again");
        return Refreshed::Not;
    };
    let answer = match Client::new(&web) {
        Ok(client) => client.machine(&account).await,
        Err(err) => Err(err),
    };
    let credentials = match answer {
        Ok(Some(credentials)) => credentials,
        Ok(None) => {
            tracing::warn!("{web} no longer takes this machine's account");
            return Refreshed::Revoked;
        }
        Err(err) => {
            tracing::warn!("asking {web} for new credentials: {err:#}");
            return Refreshed::Not;
        }
    };
    // As the file is now, which `zirc login` may have changed while the web app answered.
    let Some(mut settings) = settings(path) else {
        return Refreshed::Not;
    };
    let Some(mut organisation) = config::find(&settings, org, first) else {
        return Refreshed::Not;
    };
    if credentials.account.as_ref() == Some(&account) && credentials.server == organisation.server {
        tracing::warn!("{web} has no newer credentials for this machine: run `zirc login` again");
        return Refreshed::Not;
    }
    if settings.organisations().len() > 1 && !same_server(&credentials.server, &organisation.server)
    {
        tracing::warn!(
            "{web} answered for {org} with another server, {}: not using it; run `zirc login \
             --org {org}` again",
            credentials.server.host
        );
        return Refreshed::Not;
    }
    match &credentials.account {
        Some(new) if new.name != account.name => {
            tracing::info!("{web} renamed this machine's account to {}", new.name);
        }
        _ => tracing::info!("{web} gave this machine a new passphrase"),
    }
    let name = organisation.name();
    organisation.nick = credentials.nick.or_else(|| {
        credentials
            .account
            .as_ref()
            .map(|account| account.name.clone())
    });
    organisation.server = credentials.server;
    organisation.account = credentials.account;
    organisation.owner = credentials.owner.or(organisation.owner);
    if !first {
        // Named as it was, even if its server's host changed.
        organisation.name = Some(name.clone());
    }
    settings.set_organisation(&name, organisation);
    match settings.save(path) {
        Ok(()) => Refreshed::New,
        Err(err) => {
            tracing::warn!("saving new credentials to {}: {err}", path.display());
            Refreshed::Not
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use super::*;

    /// A server on loopback that welcomes whoever connects, and closes the connection
    /// `after` that.
    async fn server(after: Duration) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    stream.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
                    tokio::time::sleep(after).await;
                    // Closed cleanly: dropped with what the client sent unread, it'd be reset.
                    let _ = stream.shutdown().await;
                    let mut buffer = [0; 1024];
                    while stream.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
                });
            }
        });
        port
    }

    /// The time between each of zircd's first `count` connections to the server on `port`,
    /// backing off as `policy` says.
    async fn gaps(port: u16, policy: Policy, count: usize) -> Vec<Duration> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config =
            format!("nick = \"me\"\n[server]\nhost = \"127.0.0.1\"\nport = {port}\ntls = false\n");
        std::fs::write(&path, config).unwrap();
        let (inputs, mut received) = mpsc::unbounded_channel();
        let (calls, _called) = mpsc::unbounded_channel();
        let supervisor = tokio::spawn(supervise_with(
            policy,
            path,
            "test".into(),
            true,
            inputs,
            calls,
        ));
        let mut connected = Vec::new();
        while connected.len() < count {
            let input = tokio::time::timeout(Duration::from_secs(20), received.recv()).await;
            match input {
                Ok(Some(Input::Connected(_))) => connected.push(Instant::now()),
                Ok(Some(Input::Disconnected)) => {}
                _ => panic!("expected connections and disconnections"),
            }
        }
        supervisor.abort();
        connected.windows(2).map(|at| at[1] - at[0]).collect()
    }

    fn quick() -> Policy {
        Policy {
            first_retry: Duration::from_millis(40),
            max_retry: Duration::from_secs(10),
            connect_within: Duration::from_secs(5),
            stable: Duration::from_secs(60),
            ..Policy::default()
        }
    }

    #[tokio::test]
    async fn backs_off_from_a_server_that_drops_it_right_after_registering() {
        let port = server(Duration::ZERO).await;
        let policy = quick();
        let gaps = gaps(port, policy, 7).await;
        // The wait doubles each time, registering or not: it never starts over.
        let doubled: Vec<_> = (0..gaps.len())
            .map(|n| policy.first_retry * (1 << n))
            .collect();
        for (gap, full) in gaps.iter().zip(&doubled) {
            assert!(*gap >= *full / 2, "{gaps:?} against {doubled:?}");
        }
        // Each wait is cut at random: a sleep never ends early, so a gap shorter than the
        // whole wait was cut. Each is cut by up to half, so all of them not being cut, beyond
        // the connection's own few milliseconds, is vanishingly unlikely.
        assert!(
            gaps.iter().zip(&doubled).any(|(gap, full)| gap < full),
            "{gaps:?} against {doubled:?}"
        );
    }

    #[tokio::test]
    async fn starts_over_after_a_connection_that_lasted() {
        let up = Duration::from_millis(200);
        let port = server(up).await;
        let policy = Policy {
            stable: Duration::from_millis(100),
            ..quick()
        };
        let gaps = gaps(port, policy, 6).await;
        // Each connection lasted past `stable`, so each wait is the first's, not doubled:
        // by the fifth, a doubling wait would be 640 ms.
        let most = up + policy.first_retry + Duration::from_millis(150);
        assert!(gaps.iter().all(|gap| *gap < most), "{gaps:?}");
    }
}
