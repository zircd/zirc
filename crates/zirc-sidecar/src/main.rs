//! `zirc-sidecar`: the sidecar beside one Ergo, as a program of its own. zirc's hosted servers
//! run the same [`zirc_sidecar::run`] from their controller's binary.

use std::io::IsTerminal as _;

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Serves one Ergo's account endpoint, history search, guests' gateway and metrics, beside it.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(flatten)]
    options: zirc_sidecar::Options,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        // Colours only on a terminal: in a container the log is a file.
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
    zirc_sidecar::run_until_stopped(cli.options).await
}
