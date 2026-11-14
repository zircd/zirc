//! Running the server: Ergo as a child process in the data directory, the sidecar beside it in
//! this one, and the certificate kept fresh. The server stops when either does, so whatever
//! runs it (Docker, systemd) starts it again.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context as _, bail};
use clap::ValueEnum;
use tokio::process::Command;
use zirc_proto::desired::{Multiline, RateLimit, Settings};
use zirc_sidecar::config::{self, Held, KEEP_EVERYTHING};
use zirc_sidecar::environment::{self, Value};

use axum_server::tls_rustls::RustlsConfig;

use crate::secrets::Secrets;
use crate::tls::{self, Acme, Challenges};

/// Where the server keeps everything, in its container.
pub const DATA: &str = "/data";
/// Where its account endpoint listens: on loopback, for the server's own use.
pub const ACCOUNTS_LISTEN: &str = "127.0.0.1:9101";
/// zirc.dev, which the server joins and is managed by.
pub const ZIRC_DEV: &str = "https://zirc.dev";
/// The socket Ergo hands sign-ins with a device's key to, for the sidecar to check, in the
/// data directory.
const AUTH_SOCKET: &str = "zirc-auth.sock";

/// How the server's IRC is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Tls {
    /// TLS on 6697 with a certificate from Let's Encrypt, which needs port 80 open.
    Acme,
    /// TLS on 6697 with a certificate Ergo makes itself, which clients must be told to trust:
    /// for trying the server out.
    SelfSigned,
    /// Plain IRC on 6667, for a proxy in front that ends TLS.
    Off,
}

/// How the server is run.
#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    /// The host name people connect to, such as irc.example.com.
    #[arg(long, env = "ZIRC_HOST")]
    pub host: String,
    /// The organisation's name, which the network takes. The host name's first label unless
    /// set.
    #[arg(long, env = "ZIRC_NAME")]
    pub name: Option<String>,
    /// Where everything is kept: accounts, history, secrets, the certificate.
    #[arg(long, env = "ZIRC_DATA", default_value = DATA)]
    pub data: PathBuf,
    /// How IRC is reached.
    #[arg(long, env = "ZIRC_TLS", value_enum, default_value_t = Tls::Acme)]
    pub tls: Tls,
    /// The address Let's Encrypt can write to about the certificate.
    #[arg(long, env = "ZIRC_ACME_EMAIL")]
    pub acme_email: Option<String>,
    /// The ACME directory, Let's Encrypt's unless set (its staging one, say).
    #[arg(long, env = "ZIRC_ACME_DIRECTORY", default_value = tls::LETS_ENCRYPT)]
    pub acme_directory: String,
    /// Where ACME's challenges are answered: port 80 of the host.
    #[arg(long, env = "ZIRC_ACME_LISTEN", default_value = "0.0.0.0:80")]
    pub acme_listen: SocketAddr,
    /// Where IRC is served: 6697 with TLS, 6667 without, unless set.
    #[arg(long, env = "ZIRC_IRC_LISTEN")]
    pub irc_listen: Option<String>,
    /// With `--tls off`: the proxy in front sends a PROXY header first, so Ergo sees who
    /// connected.
    #[arg(long, env = "ZIRC_PROXY_PROTOCOL")]
    pub proxy_protocol: bool,
    /// Who may send a PROXY header, besides loopback.
    #[arg(long, env = "ZIRC_PROXY_FROM", value_delimiter = ',')]
    pub proxy_from: Vec<String>,
    /// Where the HTTPS side listens, which zirc.dev manages the server through and apps read
    /// history from: 443 with TLS, 8080 without, unless set.
    #[arg(long, env = "ZIRC_WEB_LISTEN")]
    pub web_listen: Option<SocketAddr>,
    /// The join token zirc.dev showed when the organisation was made "on my own server": the
    /// server joins it on its first start.
    #[arg(long, env = "ZIRC_JOIN_TOKEN", hide_env_values = true)]
    pub join_token: Option<String>,
    /// zirc.dev, or another of its environments, such as staging's.
    #[arg(long, env = "ZIRC_WEB", default_value = ZIRC_DEV)]
    pub web: String,
    /// IRC's port as people reach it, 6697 unless set: behind a proxy, the proxy's.
    #[arg(long, env = "ZIRC_PUBLIC_IRC_PORT", default_value_t = 6697)]
    pub public_irc_port: u16,
    /// Ergo's log at debug.
    #[arg(long, env = "ZIRC_DEBUG")]
    pub debug: bool,
    /// The Ergo binary.
    #[arg(long, env = "ZIRC_ERGO", default_value = "/usr/local/bin/ergo")]
    pub ergo: PathBuf,
    /// The `default.yaml` of the same Ergo release.
    #[arg(
        long,
        env = "ZIRC_ERGO_CONFIG",
        default_value = "/usr/local/share/ergo/default.yaml"
    )]
    pub ergo_config: PathBuf,
    /// The organisation's own S3 bucket, to keep files in rather than on the server's disk.
    #[command(flatten)]
    pub s3: crate::store::S3,
    /// Where imgproxy and the media worker, beside the server, read files and put what
    /// they make: plain HTTP, for links the server signed, never published. Off unless set,
    /// as the image sets it.
    #[arg(long, env = "ZIRC_BESIDE_LISTEN")]
    pub beside_listen: Option<SocketAddr>,
    /// The server's address as imgproxy and the media worker reach it, `beside_listen`'s:
    /// compose's service name.
    #[arg(long, env = "ZIRC_BESIDE_URL", default_value = "http://zirc:8090")]
    pub beside_url: String,
    /// imgproxy, which resizes images people share, beside the server: compose's. Images are
    /// served as they were sent without it.
    #[arg(long, env = "ZIRC_IMGPROXY")]
    pub imgproxy: Option<String>,
    /// The media worker, which makes videos' posters and the copies people play, beside the
    /// server: compose's. Video is shared as a file without it.
    #[arg(long, env = "ZIRC_MEDIA_WORKER")]
    pub media_worker: Option<String>,
    /// Loopback addresses inside the server, for running more than one on a host (tests).
    #[command(flatten)]
    pub inside: Inside,
}

/// Where the server's parts listen for each other, on loopback.
#[derive(Clone, Debug, clap::Args)]
pub struct Inside {
    /// Ergo's listener for the sidecar's operator.
    #[arg(long, hide = true, default_value = "127.0.0.1:6668")]
    pub oper_listen: SocketAddr,
    /// Ergo's HTTP API.
    #[arg(long, hide = true, default_value = environment::API_LISTENER)]
    pub api_listen: SocketAddr,
    /// The sidecar's metrics.
    #[arg(long, hide = true, default_value = "127.0.0.1:9100")]
    pub metrics_listen: SocketAddr,
    /// The sidecar's account endpoint.
    #[arg(long, env = "ZIRC_ACCOUNTS_LISTEN", hide = true, default_value = ACCOUNTS_LISTEN)]
    pub accounts_listen: SocketAddr,
    /// The sidecar's guest gateway.
    #[arg(long, hide = true, default_value = "127.0.0.1:9102")]
    pub guest_listen: SocketAddr,
}

/// What a self-hosted server's settings come to: a paid hosted server's defaults, with all
/// history kept. Bound by nothing but Ergo.
fn held() -> Held {
    Held {
        retention: KEEP_EVERYTHING.to_owned(),
        line_length: 512,
        multiline: Multiline {
            max_bytes: 16_384,
            max_lines: 200,
        },
        rate: RateLimit::USUAL,
    }
}

/// A self-hosted server's settings: everyone's first connection joins `#general`
/// ([`zirc_proto::ORG_CHANNEL`]), as on a hosted server, and the first to come makes it, so
/// nobody lands in a server with no channel (the user's choice, 13 Nov 2026). Anyone may make
/// more.
fn settings() -> Settings {
    Settings {
        default_channels: vec![zirc_proto::ORG_CHANNEL.to_owned()],
        ..Settings::default()
    }
}

impl Options {
    /// What the server tells zirc.dev as it joins with `token`.
    fn join_request(&self, token: String) -> zirc_proto::join::JoinRequest {
        zirc_proto::join::JoinRequest {
            token,
            host: self.host.clone(),
            port: self.public_irc_port,
            version: format!("zirc-server {}", env!("CARGO_PKG_VERSION")),
        }
    }

    fn web_listen(&self) -> SocketAddr {
        self.web_listen.unwrap_or_else(|| match self.tls {
            Tls::Acme | Tls::SelfSigned => SocketAddr::from(([0, 0, 0, 0], 443)),
            Tls::Off => SocketAddr::from(([0, 0, 0, 0], 8080)),
        })
    }

    fn network(&self) -> &str {
        self.name
            .as_deref()
            .unwrap_or_else(|| self.host.split('.').next().unwrap_or(&self.host))
    }

    /// Ergo's `server.listeners`: IRC as `tls` says, and the operator's on loopback.
    fn listeners(&self) -> String {
        let oper = self.inside.oper_listen.to_string();
        let irc = match self.tls {
            Tls::Acme | Tls::SelfSigned => {
                let at = self.irc_listen.as_deref().unwrap_or(":6697");
                (
                    at,
                    serde_json::json!({
                        "tls": {"cert": tls::CHAIN, "key": tls::KEY},
                        "min-tls-version": "1.2",
                    }),
                )
            }
            Tls::Off => (
                self.irc_listen.as_deref().unwrap_or(":6667"),
                serde_json::json!({"proxy": self.proxy_protocol}),
            ),
        };
        let mut listeners = serde_json::Map::new();
        listeners.insert(irc.0.to_owned(), irc.1);
        listeners.insert(oper, serde_json::json!({}));
        serde_json::Value::Object(listeners).to_string()
    }

    /// Ergo's environment, its secrets filled in from `secrets`.
    fn ergo_env(&self, data: &Path, secrets: &Secrets) -> Vec<(&'static str, String)> {
        let mut proxy_allowed_from = vec!["localhost".to_owned()];
        proxy_allowed_from.extend(self.proxy_from.iter().cloned());
        let api = self.inside.api_listen.to_string();
        let socket = data.join(AUTH_SOCKET).display().to_string();
        let subscriber = format!("{}/", self.web.trim_end_matches('/'));
        let server = environment::Server {
            network: self.network(),
            name: &self.host,
            listeners: self.listeners(),
            proxy_allowed_from,
            require_sasl: true,
            web_origins: vec![self.web.clone()],
            // zirc.dev relays its pushes to people's phones, as a hosted server's.
            push_subscriber: Some(&subscriber),
            token_keys: vec![],
            // Each of a person's apps signs in to their account with a key of its own.
            auth_socket: Some(&socket),
            api_listener: &api,
            debug: self.debug,
        };
        environment::ergo(&server)
            .into_iter()
            .map(|(name, value)| {
                let value = match value {
                    Value::Plain(value) => value,
                    Value::Secret(secret) => secrets.for_ergo(secret),
                };
                (name, value)
            })
            .collect()
    }
}

/// Runs the server until Ergo or the sidecar stops, or it's asked to stop.
///
/// # Errors
///
/// When it can't start: its data directory, its certificate, Ergo.
pub async fn run(options: Options) -> anyhow::Result<()> {
    // Ergo takes nothing else as its name, and says only that its config didn't load.
    if !options.host.contains('.') {
        bail!(
            "the host name must be a full one, such as irc.example.com, not {}",
            options.host
        );
    }
    let data = absolute(&options.data)?;
    std::fs::create_dir_all(&data)
        .with_context(|| format!("making the data directory {}", data.display()))?;
    let mut secrets = Secrets::read_or_make(&data)?;
    let ergo = absolute(&options.ergo)?;
    let default_yaml = std::fs::read_to_string(&options.ergo_config).with_context(|| {
        format!(
            "reading Ergo's default.yaml at {}",
            options.ergo_config.display()
        )
    })?;
    let config = config::without_listeners(&config::file(&default_yaml, &settings(), &held()));
    std::fs::write(data.join(config::CONFIG), config)?;
    std::fs::write(
        data.join(config::MOTD),
        format!("zirc · {}'s server\n", options.network()),
    )?;
    let env = options.ergo_env(&data, &secrets);
    let ergo_command = |args: &[&str]| {
        let mut command = Command::new(&ergo);
        command
            .args(args)
            .args(["--conf", config::CONFIG])
            .current_dir(&data)
            .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
            .stdin(Stdio::null());
        command
    };

    let acme = Acme {
        directory: options.acme_directory.clone(),
        email: options.acme_email.clone(),
    };
    let challenges = Challenges::default();
    certificate(&options, &data, &acme, &challenges).await?;
    if !data.join("ircd.db").exists() {
        quietly(ergo_command(&["initdb", "--quiet"])).await?;
    }
    // The HTTPS side first: zirc.dev reaches the server there before it lets it join.
    let rustls = https_config(options.tls, &data).await?;
    let (router, beside_side) = web_side(&options, &data)?;
    let web_side = tokio::spawn(serve_web(options.web_listen(), router, rustls.clone()));
    if let Some(token) = options.join_token.clone() {
        crate::join::join(&options.web, &options.join_request(token), &data).await?;
        secrets = Secrets::read(&data)?;
    }

    let mut ergo = ergo_command(&["run"])
        .kill_on_drop(true)
        .spawn()
        .context("starting Ergo")?;
    let sidecar = tokio::spawn(zirc_sidecar::run(sidecar_options(
        &options, &data, &secrets,
    )));
    let operator = zirc_sidecar::ergo::Operator {
        host: options.inside.oper_listen.ip().to_string(),
        port: options.inside.oper_listen.port(),
        server_password: Some(secrets.gate.password.clone()),
        password: secrets.oper.password.clone(),
    };
    let renewing = (options.tls == Tls::Acme).then(|| {
        let (host, data) = (options.host.clone(), data.clone());
        tokio::spawn(keep_renewed(host, data, acme, challenges, operator, rustls))
    });
    tracing::info!("serving {} as {}", options.host, options.network());

    let outcome = tokio::select! {
        status = ergo.wait() => Err(anyhow::anyhow!("Ergo stopped: {}", status?)),
        ended = sidecar => Err(anyhow::anyhow!("the sidecar stopped: {:?}", ended?)),
        ended = web_side => Err(anyhow::anyhow!("the HTTPS side stopped: {:?}", ended?)),
        ended = async {
            match beside_side {
                Some(side) => side.await,
                None => std::future::pending().await,
            }
        } => Err(anyhow::anyhow!("the side for imgproxy and the media worker stopped: {:?}", ended?)),
        () = stopped() => Ok(()),
    };
    if let Some(renewing) = renewing {
        renewing.abort();
    }
    // Ergo saves its state as it goes; stopping it is enough.
    let _ = ergo.start_kill();
    let _ = ergo.wait().await;
    outcome
}

/// The server's certificate, as `--tls` says: from Let's Encrypt (with port 80 answering its
/// challenges, and sending everything else to HTTPS, for as long as the server runs), made here,
/// or none.
async fn certificate(
    options: &Options,
    data: &Path,
    acme: &Acme,
    challenges: &Challenges,
) -> anyhow::Result<()> {
    match options.tls {
        Tls::Acme => {
            tokio::spawn(
                challenges
                    .clone()
                    .serve(options.acme_listen, options.host.clone()),
            );
            if tls::due(data) {
                tls::obtain(&options.host, data, acme, challenges).await?;
            }
        }
        // Ergo's own `mkcerts` makes one for the listeners in its file, and ours are in its
        // environment.
        Tls::SelfSigned if tls::days_left(data).is_none() => {
            tls::self_signed(&options.host, data)?;
        }
        Tls::SelfSigned | Tls::Off => {}
    }
    Ok(())
}

/// The HTTPS side's TLS, from the certificate in `data`, unless it's behind a proxy.
async fn https_config(mode: Tls, data: &Path) -> anyhow::Result<Option<RustlsConfig>> {
    Ok(match mode {
        Tls::Off => None,
        Tls::Acme | Tls::SelfSigned => Some(
            RustlsConfig::from_pem_file(data.join(tls::CHAIN), data.join(tls::KEY))
                .await
                .context("reading the certificate for the HTTPS side")?,
        ),
    })
}

/// The sidecar's options, for this server's Ergo.
fn sidecar_options(options: &Options, data: &Path, secrets: &Secrets) -> zirc_sidecar::Options {
    let inside = &options.inside;
    zirc_sidecar::Options {
        metrics: zirc_sidecar::exporter::Options {
            listen: inside.metrics_listen,
            ergo_api: format!("http://{}", inside.api_listen),
            token: secrets.api_token.clone(),
            history: data.join("ergo_history.db"),
        },
        accounts: zirc_sidecar::accounts::Options {
            accounts_listen: inside.accounts_listen,
            accounts_token: secrets.accounts_token.clone(),
            irc: inside.oper_listen,
            oper_password: secrets.oper.password.clone(),
            gate_password: Some(secrets.gate.password.clone()),
            controller_token: Some(secrets.controller_token.clone()),
            guest_listen: inside.guest_listen,
            server_name: Some(options.host.clone()),
            auth_socket: Some(data.join(AUTH_SOCKET)),
        },
    }
}

/// Renews the certificate when it's due, then has Ergo read it again.
async fn keep_renewed(
    host: String,
    data: PathBuf,
    acme: Acme,
    challenges: Challenges,
    operator: zirc_sidecar::ergo::Operator,
    rustls: Option<RustlsConfig>,
) {
    loop {
        tokio::time::sleep(tls::CHECK_EVERY).await;
        if !tls::due(&data) {
            continue;
        }
        match tls::obtain(&host, &data, &acme, &challenges).await {
            Ok(()) => {
                if let Err(e) = operator.rehash().await {
                    tracing::warn!("Ergo didn't read the new certificate: {e}");
                }
                if let Some(rustls) = &rustls
                    && let Err(e) = rustls
                        .reload_from_pem_file(data.join(tls::CHAIN), data.join(tls::KEY))
                        .await
                {
                    tracing::warn!("the HTTPS side didn't read the new certificate: {e}");
                }
            }
            Err(e) => tracing::warn!("couldn't renew the certificate, trying again later: {e:#}"),
        }
    }
}

/// Serves `router` on `listen` over plain HTTP.
async fn serve(listen: SocketAddr, router: axum::Router) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, router).await
}

/// Serves the HTTPS side on `listen`, over TLS with `rustls` when there's a certificate.
async fn serve_web(
    listen: SocketAddr,
    router: axum::Router,
    rustls: Option<RustlsConfig>,
) -> std::io::Result<()> {
    match rustls {
        Some(rustls) => {
            axum_server::bind_rustls(listen, rustls)
                .serve(router.into_make_service())
                .await
        }
        None => serve(listen, router).await,
    }
}

/// The HTTPS side's routes, and the side for imgproxy and the media worker, serving, if it's
/// on: the files' store in `data` or the bucket, which both reach.
fn web_side(
    options: &Options,
    data: &Path,
) -> anyhow::Result<(
    axum::Router,
    Option<tokio::task::JoinHandle<std::io::Result<()>>>,
)> {
    let store = std::sync::Arc::new(crate::store::Store::new(
        data,
        &options.s3,
        format!("https://{}", options.host),
        options.beside_url.clone(),
    )?);
    let beside_side = options.beside_listen.map(|listen| {
        let router = crate::store::router(store.clone());
        tokio::spawn(serve_web(listen, router, None))
    });
    let beside = crate::media::Beside {
        imgproxy: options.imgproxy.clone(),
        worker: options.media_worker.clone(),
    };
    let media = crate::media::Media::new(
        store.clone(),
        beside,
        &options.host,
        data,
        format!("http://{}", options.inside.api_listen),
        format!("http://{}", options.inside.accounts_listen),
    );
    let router = crate::proxy::router(&options.inside, std::sync::Arc::new(media), store);
    Ok((router, beside_side))
}

/// Runs a one-off Ergo command, saying what it wrote only when it fails.
async fn quietly(mut command: Command) -> anyhow::Result<()> {
    let output = command.output().await.context("running Ergo")?;
    if !output.status.success() {
        bail!(
            "Ergo failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Ends when the process is asked to stop: Ctrl-C, or SIGTERM, as `docker stop` sends.
async fn stopped() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// `path` from the working directory, as Ergo runs in the data directory instead.
fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(std::env::current_dir()?.join(path))
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;

    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        options: Options,
    }

    fn options(args: &[&str]) -> Options {
        let mut all = vec!["zirc-server", "--host", "irc.acme.test"];
        all.extend(args);
        Cli::try_parse_from(all).unwrap().options
    }

    #[test]
    fn the_network_is_the_host_s_first_label_unless_named() {
        assert_eq!(options(&[]).network(), "irc");
        assert_eq!(options(&["--name", "acme"]).network(), "acme");
    }

    #[test]
    fn everyone_s_first_connection_joins_general_and_anyone_may_make_a_channel() {
        let settings = settings();
        assert_eq!(settings.default_channels, ["#general"]);
        assert_eq!(
            settings.channel_creation,
            Settings::default().channel_creation
        );
    }

    #[test]
    fn with_tls_irc_is_on_6697_with_the_certificate() {
        let listeners: serde_json::Value = serde_json::from_str(&options(&[]).listeners()).unwrap();
        assert_eq!(listeners[":6697"]["tls"]["cert"], tls::CHAIN);
        assert_eq!(listeners["127.0.0.1:6668"], serde_json::json!({}));
    }

    #[test]
    fn behind_a_proxy_irc_is_plain_on_6667() {
        let listeners: serde_json::Value =
            serde_json::from_str(&options(&["--tls", "off", "--proxy-protocol"]).listeners())
                .unwrap();
        assert_eq!(listeners[":6667"]["proxy"], true);
    }

    #[test]
    fn ergo_gets_secrets_filled_in_and_pushes_through_zirc_dev() {
        let data = tempfile::tempdir().unwrap();
        let secrets = Secrets::read_or_make(data.path()).unwrap();
        let env = options(&[]).ergo_env(data.path(), &secrets);
        let get = |name: &str| env.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone());
        assert_eq!(get("ERGO__SERVER__PASSWORD"), Some(secrets.gate.hash));
        assert_eq!(get("ERGO__WEBPUSH__ENABLED").as_deref(), Some("true"));
        assert_eq!(
            get("ERGO__WEBPUSH__SUBSCRIBER").as_deref(),
            Some("https://zirc.dev/")
        );
        assert_eq!(
            get("ERGO__ACCOUNTS__REQUIRE_SASL__ENABLED").as_deref(),
            Some("true")
        );
    }
}
