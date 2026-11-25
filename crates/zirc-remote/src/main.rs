//! `zirc-remote`: serves the commands in a JSON config to those who may call them.

use std::collections::BTreeMap;
use std::io::BufRead as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;
use zirc_remote::{Config, Options, run};

/// Serves the commands in a JSON config to the owner's agents and apps, over zirc.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// The JSON config: the commands this host serves, and who may call them.
    #[arg(long)]
    config: PathBuf,
    /// Only check the config, say what it serves, and exit.
    #[arg(long)]
    check: bool,
    /// The IRC server's host name.
    #[arg(long, required_unless_present = "check")]
    server: Option<String>,
    /// Its port.
    #[arg(long, default_value_t = 6697)]
    port: u16,
    /// Connect without TLS, for a local server only.
    #[arg(long)]
    no_tls: bool,
    /// The machine account to log in as, such as `build-2300`. Its passphrase is the first
    /// line of standard input, so it is in no file and no environment; the lines after it are
    /// secrets for the commands that name them, `NAME=value`.
    #[arg(long, required_unless_present = "check")]
    account: Option<String>,
    /// Whose machine it is, its owner's handle, as zirc.dev said when it gave the machine its
    /// credentials: who it answers.
    #[arg(long, required_unless_present = "check")]
    owner: Option<String>,
    /// The machine this host runs on, when it is another of its owner's (`ci-1`, a warm CI
    /// agent's host, for a landing's): apps draw this host under it.
    #[arg(long)]
    on: Option<String>,
    /// Count an account's host as saying whose machine it is on a server that isn't
    /// zirc.dev's: only where its operator alone sets hosts and makes accounts. On zirc.dev's
    /// servers it always does.
    #[arg(long)]
    trust_hosts: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    zirc_proto::version::set(env!("CARGO_PKG_VERSION"));
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let text = std::fs::read_to_string(&args.config)
        .with_context(|| format!("couldn't read {}", args.config.display()))?;
    let config = Config::parse(&text)?;
    if args.check {
        let names: Vec<&str> = config.commands.keys().map(String::as_str).collect();
        println!(
            "{}: serves {} command(s): {}",
            args.config.display(),
            names.len(),
            names.join(", ")
        );
        return Ok(());
    }
    let (Some(server), Some(account), Some(owner)) = (args.server, args.account, args.owner) else {
        anyhow::bail!("--server, --account and --owner are needed to serve");
    };
    let mut passphrase = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut passphrase)
        .context("couldn't read the passphrase from standard input")?;
    let passphrase = passphrase.trim_end().to_owned();
    anyhow::ensure!(
        !passphrase.is_empty(),
        "the first line of standard input is the passphrase"
    );
    // The rest of standard input is the secrets the host is started with, `NAME=value` a
    // line, for the commands whose config names them.
    let mut secrets = BTreeMap::new();
    for line in std::io::stdin().lock().lines() {
        let line = line.context("couldn't read the secrets from standard input")?;
        if line.trim().is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .context("a line of secrets is NAME=value")?;
        secrets.insert(name.to_owned(), value.to_owned());
    }
    run(
        Options {
            server,
            port: args.port,
            tls: !args.no_tls,
            account,
            owner,
            on: args.on,
            passphrase,
            secrets,
            trust_hosts: args.trust_hosts,
        },
        config,
    )
    .await
}
