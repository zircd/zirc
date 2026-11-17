//! The sidecar that runs beside each zirc server's Ergo, in its pod: its account endpoint, the
//! history search, the public channels' gateway, guests, usage counts and metrics. See
//! README.md.

pub mod accounts;
pub mod activity;
pub mod config;
pub mod environment;
pub mod ergo;
pub mod exporter;
pub mod gateway;
pub mod guests;
pub mod hints;
pub mod history;
pub mod index;
pub mod janitor;
pub mod limits;
pub mod machines;
pub mod profiles;
pub mod pusher;
pub mod recorder;
pub mod say;
pub mod secrets;
pub mod signin;
pub mod suggestions;
pub mod teams;
pub mod trim;
pub mod usage;
pub mod visitors;
pub mod workflows;

/// Everything the sidecar is told, on its command line or in its environment.
#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    /// The metrics endpoint's, and how Ergo's API and history are reached.
    #[command(flatten)]
    pub metrics: exporter::Options,
    /// The account endpoint's.
    #[command(flatten)]
    pub accounts: accounts::Options,
}

/// Runs the sidecar beside an Ergo until one of its endpoints stops: the metrics and account
/// endpoints, and what keeps Ergo's history scrubbed, indexed and recorded.
///
/// # Errors
///
/// When an endpoint can't listen.
pub async fn run(options: Options) -> std::io::Result<()> {
    let Options { metrics, accounts } = options;
    let api = metrics.api();
    let history = metrics.history.clone();
    tokio::spawn(trim::keep_scrubbed(history.clone()));
    tokio::spawn(index::keep_indexed(history.clone()));
    tokio::spawn(activity::keep_recorded(history.clone()));
    tokio::try_join!(
        exporter::run(metrics),
        accounts::run(accounts, api, history)
    )?;
    Ok(())
}

/// The HMAC-SHA256 of `body` under `secret`, in lowercase hex: what the sidecar signs with
/// and derives passphrases by.
#[must_use]
pub fn hmac_hex(secret: &str, body: &[u8]) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    zirc_proto::hex(ring::hmac::sign(&key, body).as_ref())
}

/// Installs ring as rustls's crypto provider, once: reqwest is built without one
/// (`rustls-no-provider`, as zirc's crates have it), and so is Sentry's transport in the
/// controller, which builds an HTTP client when it starts and panics without a provider.
pub fn install_tls_provider() {
    static PROVIDER: std::sync::Once = std::sync::Once::new();
    PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// An HTTP client, with the TLS provider installed first ([`install_tls_provider`]). It
/// follows no redirects: what it asks are zirc's own endpoints, which send none, and one of
/// them is a self-hosted server's, on its owner's machine, which could otherwise send the
/// controller's request on to an address inside the cluster.
#[must_use]
pub fn http() -> reqwest::Client {
    install_tls_provider();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    /// The command line as the sidecar's own binary and the controller's have it.
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        options: super::Options,
    }

    /// What clap only checks at run time in a debug build, such as two flattened groups with
    /// the same name, which once kept every tenant's sidecar from starting.
    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn it_takes_its_options() {
        let cli = Cli::try_parse_from([
            "zirc-sidecar",
            "--token",
            "api",
            "--accounts-token",
            "accounts",
            "--oper-password",
            "oper",
        ])
        .unwrap();
        assert_eq!(cli.options.accounts.oper_password, "oper");
    }
}
