//! Signing in as the person through a stand-in for zirc.dev, then connecting to a real Ergo
//! with the account it handed over.

use clap::Parser as _;
use zirc_core::secrets::{File, Secrets as _};
use zirc_irc::Line;
use zirc_test_ergo::{require_ergo, web};
use zirc_test_irc::{expect, person, register};
use zirc_tui::{App, Cli, Session, signin};

#[tokio::test]
async fn signs_in_as_the_person_and_connects_as_their_account() {
    let ergo = require_ergo!();
    register(&ergo, "mattias", "right-horse-battery").await;

    let started = r#"{"id":"L1","code":"WDJB-MJHT","url":"https://zirc.test/login/WDJB-MJHT","interval":1,"expires_in":600}"#;
    let done = format!(
        r#"{{"status":"done","server":{{"host":"127.0.0.1","port":{},"tls":false}},"account":{{"name":"mattias","password":"right-horse-battery"}},"nick":"mattias"}}"#,
        ergo.port()
    );
    let (web, served) = web::serve(vec![(200, started.to_owned()), (200, done)]);

    let home = tempfile::tempdir().unwrap();
    let secrets = File(home.path().join("secrets.toml"));
    let mut out = Vec::new();
    let profile = signin::sign_in(&web, false, home.path(), &secrets, &mut out)
        .await
        .unwrap();
    let said = String::from_utf8(out).unwrap();
    assert!(
        said.contains("https://zirc.test/login/WDJB-MJHT") && said.contains("WDJB-MJHT"),
        "{said}"
    );
    // words-guard: the terminal's own line of where it signed in (src/signin.rs), not Settings' label
    assert!(said.contains("Signed in to 127.0.0.1 as mattias"), "{said}");
    let requests = served.join().unwrap();
    assert!(
        requests[0].starts_with("POST /api/cli/login "),
        "{}",
        requests[0]
    );
    assert!(
        requests[0].contains(r#""kind":"person""#),
        "{}",
        requests[0]
    );

    // The passphrase is in the secret store, not the file.
    assert_eq!(
        secrets.get("mattias@127.0.0.1").as_deref(),
        Some("right-horse-battery")
    );
    let kept = std::fs::read_to_string(signin::path(home.path())).unwrap();
    assert!(!kept.contains("right-horse"), "{kept}");
    assert!(profile.account);

    // Running it now connects as the account, with no sign-in.
    let mut out = Vec::new();
    let config = Cli::parse_from(["zirc-tui"])
        .connection(home.path(), &secrets, &mut out)
        .await
        .unwrap();
    assert!(out.is_empty());
    let session = Session::connect(App::new(&config.nick, &config.host, Vec::new()), config)
        .await
        .unwrap();
    assert_eq!(session.app.nick(), "mattias");

    // Anyone can see it's logged in to the account.
    let (ada, mut heard) = person(&ergo, "ada").await;
    ada.send(Line::new("WHOIS", ["mattias"])).unwrap();
    let account = expect(&mut heard, "330").await;
    assert_eq!(account.params[2], "mattias");
}
