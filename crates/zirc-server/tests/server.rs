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

/// An Ed25519 key's public half as PEM, as zirc.dev lists a server's `tokenKeys`.
fn public_pem(key: &ring::signature::Ed25519KeyPair) -> String {
    use base64::Engine as _;
    use ring::signature::KeyPair as _;
    let mut der = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    der.extend_from_slice(key.public_key().as_ref());
    let pem = base64::engine::general_purpose::STANDARD.encode(der);
    format!("-----BEGIN PUBLIC KEY-----\n{pem}\n-----END PUBLIC KEY-----\n")
}

/// A browser login's token for `account` on `server`, as zirc.dev signs it (lib/irc-tokens.ts).
fn browser_token(key: &ring::signature::Ed25519KeyPair, account: &str, server: &str) -> String {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let part = |v: serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    let data = format!(
        "{}.{}",
        part(json!({"alg": "EdDSA", "typ": "JWT"})),
        part(json!({"account": account, "aud": server, "exp": exp}))
    );
    format!(
        "{data}.{}",
        URL_SAFE_NO_PAD.encode(key.sign(data.as_bytes()).as_ref())
    )
}

/// The server answering content for zirc.dev's pages, its browser logins signed with the key
/// it returns, and Bob, with an account of his own; Alice has none yet, as when her only
/// sign-in is a machine's.
async fn content_for_zirc_dev_with_bob(
    server: &common::Running,
) -> ring::signature::Ed25519KeyPair {
    let secrets = Secrets::read(&server.options.data).unwrap();
    let http = reqwest::Client::new();
    let sidecar = format!("{}{SIDECAR_PATH}", server.https);
    let rng = ring::rand::SystemRandom::new();
    let key = ring::signature::Ed25519KeyPair::from_pkcs8(
        ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let teams = json!({"channels": [], "public": {"keys": [public_pem(&key)]}, "content": {"server": "127.0.0.1", "origins": ["https://zirc.dev"]}});
    let teams_set = http
        .put(format!("{sidecar}/v1/teams"))
        .bearer_auth(&secrets.controller_token)
        .json(&teams)
        .send()
        .await
        .unwrap();
    assert!(teams_set.status().is_success(), "{}", teams_set.status());
    let body = json!({"passphrase": "bob-correct-horse-battery"});
    let made = http
        .put(format!("{sidecar}/v1/accounts/bob"))
        .bearer_auth(&secrets.accounts_token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(made.status().is_success(), "{}", made.status());
    key
}

/// A page's ask about Alice's conversation with Bob: the server, Bob connected and listening,
/// Alice's token, and where the conversation is.
struct Asking {
    _server: common::Running,
    http: reqwest::Client,
    alice: String,
    conversation: String,
    heard: zirc_irc::Incoming,
    _bob: zirc_irc::Client,
}

async fn alice_asking_about_bob() -> Option<Asking> {
    let server = common::start().await?;
    let key = content_for_zirc_dev_with_bob(&server).await;
    let mut config = Config::new("127.0.0.1", server.irc, "bob");
    config.tls = false;
    config.sasl = Some(Credentials {
        account: "bob".into(),
        password: "bob-correct-horse-battery".into(),
    });
    let (bob, heard) = zirc_irc::connect(&config).await.unwrap();
    Some(Asking {
        http: reqwest::Client::new(),
        alice: browser_token(&key, "alice", "127.0.0.1"),
        conversation: format!("{}/api/v1/conversations/bob", server.https),
        heard,
        _bob: bob,
        _server: server,
    })
}

impl Asking {
    /// Alice writes `text` to Bob on a page.
    async fn write(&self, text: &str) -> reqwest::Response {
        self.http
            .post(&self.conversation)
            .bearer_auth(&self.alice)
            .json(&json!({ "text": text }))
            .send()
            .await
            .unwrap()
    }

    /// The first message Bob hears.
    async fn bob_hears(&mut self) -> zirc_irc::Line {
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(line) = self.heard.recv().await {
                if line.command == "PRIVMSG" {
                    return line;
                }
            }
            panic!("bob's connection ended");
        })
        .await
        .expect("bob heard it")
    }

    /// The conversation, once the history has something in it.
    async fn read_once_kept(&self) -> serde_json::Value {
        let mut every = tokio::time::interval(Duration::from_millis(200));
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                every.tick().await;
                let read: serde_json::Value = self
                    .http
                    .get(&self.conversation)
                    .bearer_auth(&self.alice)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                if read["entries"].as_array().is_some_and(|e| !e.is_empty()) {
                    return read;
                }
            }
        })
        .await
        .expect("the history keeps it")
    }
}

/// zirc.dev's page asks across origins: the preflight passes through the HTTPS side.
#[tokio::test(flavor = "multi_thread")]
async fn zirc_devs_preflight_passes_through_the_https_side() {
    let Some(asking) = alice_asking_about_bob().await else {
        return;
    };

    let preflight = asking
        .http
        .request(reqwest::Method::OPTIONS, &asking.conversation)
        .header("Origin", "https://zirc.dev")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "authorization, content-type",
        )
        .send()
        .await
        .unwrap();

    assert_eq!(preflight.status(), 204);
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "https://zirc.dev"
    );
}

/// A page writes as its person (`zirc_proto::conversation`): the server signs in to its own
/// Ergo as Alice, who has no account yet, with a pass of its own, so Bob hears it from her.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_writes_as_its_person_and_the_other_end_hears_it_from_them() {
    let Some(mut asking) = alice_asking_about_bob().await else {
        return;
    };

    let sent = asking.write("  Hi Bob, are you there? ").await;

    assert_eq!(sent.status(), 200, "{}", sent.text().await.unwrap());
    let said = asking.bob_hears().await;
    let from = said.source.as_deref().unwrap_or_default();
    assert!(from.starts_with("alice!"), "{said:?}");
    assert_eq!(
        said.params.last().map(String::as_str),
        Some("Hi Bob, are you there?")
    );
}

/// What a page wrote reads back as the person's own, with the composer's words.
#[tokio::test(flavor = "multi_thread")]
async fn what_a_page_wrote_reads_back_as_the_persons_own() {
    let Some(asking) = alice_asking_about_bob().await else {
        return;
    };

    asking.write("Hi Bob").await;

    let read = asking.read_once_kept().await;
    assert_eq!(read["placeholder"], "Message bob");
    assert_eq!(read["entries"][0]["kind"], "said");
    assert_eq!(read["entries"][0]["mine"], true);
    assert_eq!(read["entries"][0]["who"], "You");
    assert_eq!(read["entries"][0]["text"], "Hi Bob");
}
