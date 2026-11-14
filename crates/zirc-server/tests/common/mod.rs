//! A server as its container runs it, behind a proxy (`--tls off`), on free loopback ports,
//! against the pinned Ergo, for the tests. Each test file says `mod common;`, and what it
//! doesn't use is allowed to be dead there.
#![allow(
    dead_code,
    reason = "each test file says `mod common;` and uses its own part of it"
)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::task::JoinHandle;
use zirc_server::admin::{self, Client};
use zirc_server::run::{Inside, Options, Tls};
use zirc_server::secrets::Secrets;
use zirc_test_ergo::free_ports;

/// The pinned Ergo and its default.yaml, as Bazel hands them to tests; none outside Bazel.
pub fn ergo() -> Option<(PathBuf, PathBuf)> {
    let ergo = std::env::var_os("ZIRC_ERGO")?;
    let config = std::env::var_os("ZIRC_ERGO_CONFIG")?;
    Some((ergo.into(), config.into()))
}

fn at(port: u16) -> SocketAddr {
    (Ipv4Addr::LOCALHOST, port).into()
}

/// A running server, stopped when it's dropped.
pub struct Running {
    pub options: Options,
    /// Its account endpoint.
    pub accounts: Client,
    /// Its HTTPS side, plain HTTP here, as zirc.dev reaches it.
    pub https: String,
    /// IRC's port on loopback.
    pub irc: u16,
    task: JoinHandle<anyhow::Result<()>>,
    _data: tempfile::TempDir,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn options(data: &Path, ergo: PathBuf, ergo_config: PathBuf) -> Options {
    let [irc, https, oper, api, metrics, accounts, guest] = free_ports();
    Options {
        host: "irc.acme.test".into(),
        name: Some("acme".into()),
        data: data.to_owned(),
        tls: Tls::Off,
        acme_email: None,
        acme_directory: String::new(),
        acme_listen: at(0),
        irc_listen: Some(format!("127.0.0.1:{irc}")),
        proxy_protocol: false,
        proxy_from: vec![],
        web_listen: Some(at(https)),
        join_token: None,
        web: "http://127.0.0.1:1".into(),
        public_irc_port: irc,
        debug: false,
        ergo,
        ergo_config,
        s3: zirc_server::store::S3 {
            bucket: None,
            endpoint: None,
            region: "auto".into(),
            path_style: false,
        },
        beside_listen: None,
        beside_url: "http://127.0.0.1:1".into(),
        imgproxy: None,
        media_worker: None,
        inside: Inside {
            oper_listen: at(oper),
            api_listen: at(api),
            metrics_listen: at(metrics),
            accounts_listen: at(accounts),
            guest_listen: at(guest),
        },
    }
}

/// Starts a server on a fresh data directory, and answers once it takes requests; `None`
/// outside Bazel, where there's no Ergo.
pub async fn start() -> Option<Running> {
    start_with(|_| {}).await
}

/// The same, its options changed by `change` first.
pub async fn start_with(change: impl FnOnce(&mut Options)) -> Option<Running> {
    let Some((ergo, ergo_config)) = ergo() else {
        eprintln!("ZIRC_ERGO isn't set, so there's no Ergo to start: run it with Bazel");
        return None;
    };
    let data = tempfile::tempdir().unwrap();
    let mut options = options(data.path(), ergo, ergo_config);
    change(&mut options);
    let task = tokio::spawn(zirc_server::run::run(options.clone()));
    for _ in 0..200 {
        if let Ok(secrets) = Secrets::read(&options.data) {
            let accounts = Client::new(
                options.inside.accounts_listen,
                secrets.accounts_token.clone(),
            );
            if admin::list(&accounts).await.is_ok() {
                let https = format!("http://{}", options.web_listen.unwrap());
                let irc = options.public_irc_port;
                return Some(Running {
                    options,
                    accounts,
                    https,
                    irc,
                    task,
                    _data: data,
                });
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the server's account endpoint never answered");
}
