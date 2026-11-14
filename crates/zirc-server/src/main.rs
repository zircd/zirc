//! `zirc-server`: a zirc server of your own, linked to zirc.dev, as one program. See README.md.

use std::io::IsTerminal as _;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use zirc_server::{admin, run, secrets};

#[derive(Parser)]
#[command(
    version,
    about = "A zirc server of your own: Ergo and its sidecar, in one program."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Runs the server until it's stopped.
    Run(Box<run::Options>),
    /// The server's accounts, as it runs: one for each person and each machine, which
    /// zirc.dev makes.
    #[command(subcommand)]
    User(User),
}

#[derive(Subcommand)]
enum User {
    /// Lists the accounts, and whether each is connected.
    List {
        #[command(flatten)]
        server: Local,
    },
}

/// The running server an admin command asks, from inside its container.
#[derive(clap::Args)]
struct Local {
    /// The server's data directory, which has its secrets.
    #[arg(long, env = "ZIRC_DATA", default_value = run::DATA)]
    data: PathBuf,
    /// Where its account endpoint listens.
    #[arg(long, env = "ZIRC_ACCOUNTS_LISTEN", default_value = run::ACCOUNTS_LISTEN)]
    accounts: SocketAddr,
}

impl Local {
    fn client(&self) -> anyhow::Result<admin::Client> {
        let secrets = secrets::Secrets::read(&self.data)?;
        Ok(admin::Client::new(self.accounts, secrets.accounts_token))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        // Colours only on a terminal: in a container the log is a file.
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
    match cli.command {
        Command::Run(options) => run::run(*options).await,
        Command::User(user) => {
            let out = match user {
                User::List { server } => admin::list(&server.client()?).await?,
            };
            println!("{out}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    #[test]
    fn the_command_line_is_well_formed() {
        super::Cli::command().debug_assert();
    }
}
