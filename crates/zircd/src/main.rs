//! zircd's entry point: its command line (see [`zircd::Cli`]), logging, then
//! [`zircd::run`] until SIGINT or SIGTERM, starting again from its binary when it ends to
//! restart, as after an update, once every child it started has ended
//! ([`zircd::children`]).

use std::io::IsTerminal as _;
use std::os::unix::process::CommandExt as _;
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory as _, FromArgMatches as _};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

/// How long a restart waits for the runtime's tasks to stop, and then for the children they
/// started, killed as their tasks were dropped, to end.
const WINDING_DOWN: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    // Dead children a zircd that started this one by exec left unreaped, as one without
    // zircd::children did: nothing else of this process waits for a child yet.
    let _ = zircd::children::reap(Duration::ZERO);
    // Found now: once an update renames a new binary over this one, Linux names this
    // process's "(deleted)".
    let exe = std::env::current_exe();
    // This file alone is compiled with zirc's version (bazel/rust.bzl).
    let version = env!("CARGO_PKG_VERSION");
    zirc_proto::version::set(version);
    let matches = zircd::Cli::command().version(version).get_matches();
    let cli = zircd::Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit());
    let filter = match EnvFilter::try_new(&cli.log) {
        Ok(filter) => filter,
        Err(err) => {
            eprintln!("zircd: --log {}: {err}", cli.log);
            return ExitCode::FAILURE;
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        // Colours only on a terminal: under launchd or systemd the log is a file.
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    let home = cli.home.unwrap_or_else(zirc_core::paths::home);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("starting tokio's runtime");
    let ended = runtime.block_on(zircd::run(home, stopped()));
    // Its tasks dropped, and with them the children they ran, killed (`kill_on_drop`).
    runtime.shutdown_timeout(WINDING_DOWN);
    match ended {
        Ok(zircd::Ended::Stopped) => ExitCode::SUCCESS,
        Ok(zircd::Ended::Restart) => {
            // Reaped now: once this process is another binary, nothing would.
            let reaped = zircd::children::reap(WINDING_DOWN);
            tracing::info!(reaped, "starting again");
            // The same process, so launchd, systemd or whoever started it keeps it.
            let err = match exe {
                Ok(exe) => std::process::Command::new(exe)
                    .args(std::env::args_os().skip(1))
                    .exec(),
                Err(err) => err,
            };
            eprintln!("zircd: starting again: {err}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("zircd: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn stopped() {
    let mut terminate = signal(SignalKind::terminate()).expect("listening for SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
