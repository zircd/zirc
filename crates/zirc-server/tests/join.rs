//! Joining zirc.dev on the first start, against a stand-in for it: the server says where it
//! is, takes the tokens zirc.dev gives as its own, and only then lets anyone in; once joined
//! it says so. A token zirc.dev refuses stops the server, saying why.

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, routing::post};
use zirc_proto::join::{JOIN_PATH, JoinRequest, Joined, SIDECAR_PATH};
use zirc_server::join;

type Heard = Arc<Mutex<Vec<JoinRequest>>>;

/// A stand-in for zirc.dev's join endpoint, taking `good` as the one token it knows.
async fn zirc_dev(good: &'static str) -> (String, Heard) {
    let heard = Heard::default();
    let app = Router::new()
        .route(
            JOIN_PATH,
            post(
                move |State(heard): State<Heard>, Json(request): Json<JoinRequest>| async move {
                    let ok = request.token == good;
                    heard.lock().unwrap().push(request);
                    if ok {
                        Json(Joined {
                            organisation: "acme".into(),
                            accounts_token: "accounts-token-from-zirc-dev".into(),
                            controller_token: "controller-token-from-zirc-dev".into(),
                            media_settings: serde_json::json!({"video": true}),
                        })
                        .into_response()
                    } else {
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                },
            ),
        )
        .with_state(heard.clone());
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let at = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{at}"), heard)
}

#[tokio::test(flavor = "multi_thread")]
async fn it_joins_once_and_is_managed_with_zirc_dev_s_tokens() {
    let (web, heard) = zirc_dev("good-token").await;
    let Some(server) = common::start_with(|o| {
        o.web = web.clone();
        o.join_token = Some("good-token".into());
    })
    .await
    else {
        return;
    };
    let noted = join::joined(&server.options.data).expect("joined");
    assert_eq!(noted.organisation, "acme");
    let request = heard.lock().unwrap()[0].clone();
    assert_eq!(
        (request.host.as_str(), request.port),
        ("irc.acme.test", server.irc)
    );

    let list = |token: &'static str| {
        reqwest::Client::new()
            .get(format!("{}{SIDECAR_PATH}/v1/accounts", server.https))
            .bearer_auth(token)
            .send()
    };
    assert_eq!(
        list("accounts-token-from-zirc-dev").await.unwrap().status(),
        200
    );
    assert_eq!(list("a-token-it-made-itself").await.unwrap().status(), 401);
    // The organisation's files settings came with the tokens.
    let kept = std::fs::read(server.options.data.join("media/settings.json")).unwrap();
    let kept: serde_json::Value = serde_json::from_slice(&kept).unwrap();
    assert_eq!(kept["video"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_token_stops_the_server() {
    let Some((ergo, ergo_config)) = common::ergo() else {
        return;
    };
    let (web, heard) = zirc_dev("good-token").await;
    let data = tempfile::tempdir().unwrap();
    let mut options = common::options(data.path(), ergo, ergo_config);
    options.web = web;
    options.join_token = Some("used-token".into());
    let ran = tokio::time::timeout(Duration::from_secs(30), zirc_server::run::run(options))
        .await
        .expect("it stopped");
    let said = format!("{:#}", ran.unwrap_err());
    assert!(said.contains("doesn't take this join token"), "{said}");
    assert_eq!(heard.lock().unwrap().len(), 1);
    assert!(join::joined(data.path()).is_none());
}
