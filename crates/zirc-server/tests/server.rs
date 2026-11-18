//! The server as its container runs it, against the pinned Ergo: zirc.dev makes an account
//! through its HTTPS side, as it manages a hosted server's sidecar, and that account signs in;
//! nobody else does, and nobody without the server's token makes one. Behind a proxy
//! (`--tls off`), so no certificate is needed.

mod common;

use std::time::Duration;

use serde_json::json;
use zirc_irc::{Config, Credentials};
use zirc_proto::join::SIDECAR_PATH;
use zirc_server::secrets::Secrets;

/// Connects as `nick`, with `sasl` when given, and says whether the server let it register.
async fn let_in(irc: u16, nick: &str, sasl: Option<Credentials>) -> bool {
    let mut config = Config::new("127.0.0.1", irc, nick);
    config.tls = false;
    config.sasl = sasl;
    let connected = tokio::time::timeout(Duration::from_secs(10), zirc_irc::connect(&config)).await;
    matches!(connected, Ok(Ok(_)))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_zirc_dev_makes_signs_in_and_no_one_else_does() {
    let Some(server) = common::start().await else {
        return;
    };
    let token = Secrets::read(&server.options.data).unwrap().accounts_token;
    let http = reqwest::Client::new();
    let account = format!("{}{SIDECAR_PATH}/v1/accounts/alice", server.https);
    let body = json!({"passphrase": "correct-horse-battery-staple"});

    let refused = http.put(&account).json(&body).send().await.unwrap();
    assert_eq!(refused.status(), 401, "an account made without the token");
    let made = http
        .put(&account)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(made.status().is_success(), "{}", made.status());

    let alice = Credentials {
        account: "alice".into(),
        password: "correct-horse-battery-staple".into(),
    };
    assert!(
        let_in(server.irc, "alice", Some(alice)).await,
        "alice wasn't let in"
    );
    let wrong = Credentials {
        account: "alice".into(),
        password: "not-her-passphrase".into(),
    };
    assert!(!let_in(server.irc, "mallory", Some(wrong)).await);
    assert!(!let_in(server.irc, "mallory", None).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_content_api_is_on_the_https_side() {
    let Some(server) = common::start().await else {
        return;
    };
    // Until zirc.dev turns content on for it, the sidecar says it serves none: its answer, not
    // a route that isn't there.
    let answer = reqwest::get(format!("{}/api/v1/history", server.https))
        .await
        .unwrap();
    assert_eq!(answer.status(), 404);
    let said = answer.text().await.unwrap();
    assert!(said.contains("doesn't serve content here"), "{said}");
    let health = reqwest::get(format!("{}/healthz", server.https))
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
}

/// Ergo runs on without its API when another process has the API's port: the server stops,
/// saying so, rather than run without it.
#[tokio::test(flavor = "multi_thread")]
async fn a_taken_api_port_stops_the_server_saying_why() {
    let Some((ergo, config)) = common::ergo() else {
        return;
    };
    // On fresh ports again when another test took one of the others first, as `started` does:
    // the sidecar's, so the server stopped for it before Ergo's API was looked for (build 4168).
    let mut ended = String::new();
    for _ in 0..3 {
        let data = tempfile::tempdir().unwrap();
        let options = common::options(data.path(), ergo.clone(), config.clone());
        let _taken = std::net::TcpListener::bind(options.inside.api_listen).unwrap();
        ended = tokio::time::timeout(Duration::from_secs(60), zirc_server::run::run(options))
            .await
            .expect("the server stopped")
            .unwrap_err()
            .to_string();
        if ended.contains("Ergo's API never answered") || !ended.contains("AddrInUse") {
            break;
        }
    }

    assert!(ended.contains("Ergo's API never answered"), "{ended}");
}
