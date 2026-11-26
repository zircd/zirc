//! The stand-in zirc.dev signs a person's app in for real: the account its login answers is
//! one the Ergo takes with SASL. And it serves a shared video as zirc.dev does.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use zirc_proto::login::{LoginStarted, LoginStatus, START_PATH, status_path};
use zirc_proto::onboarding::{STEPS_PATH, progress};
use zirc_proto::profile::{ME_PATH, Me};
use zirc_test_ergo::require_ergo;
use zirc_test_ergo::signin::{self, Options};

/// Asks the stand-in at `web`, and gives the answer's status, head and body.
fn ask(web: &str, method: &str, path: &str, body: &str, basic: bool) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(web.trim_start_matches("http://")).expect("connecting");
    let auth = if basic {
        "authorization: Basic ZGFuYTp4\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\n{auth}content-length: {}\r\n\r\n{body}",
        body.len()
    )
    .expect("asking");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).expect("an answer");
    let cut = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a head");
    let head = String::from_utf8_lossy(&answer[..cut]).into_owned();
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .expect("a status");
    (status, head, answer[cut + 4..].to_vec())
}

fn options(media: Option<&std::path::Path>) -> Options {
    serde_json::from_value(json!({ "handle": "dana", "pending": 2, "media": media }))
        .expect("options")
}

#[test]
fn a_login_registers_the_account_the_server_then_takes() {
    let ergo = require_ergo!();
    let web = signin::start(options(None), ergo.port()).expect("the stand-in");

    // A machine's login isn't for this stand-in.
    let machine = json!({ "client": "zirc 0", "host": "mac" }).to_string();
    assert_eq!(ask(&web, "POST", START_PATH, &machine, false).0, 400);

    let request = json!({ "client": "zirc-android 0", "host": "pixel", "kind": "person" });
    let (status, _, body) = ask(&web, "POST", START_PATH, &request.to_string(), false);
    assert_eq!(status, 200);
    let started: LoginStarted = serde_json::from_slice(&body).expect("a login started");
    assert!(started.url.ends_with("/login"), "{}", started.url);

    // Pending as often as asked, then done, once.
    let poll = || {
        let (_, _, body) = ask(&web, "GET", &status_path(&started.id), "", false);
        serde_json::from_slice::<LoginStatus>(&body).expect("a status")
    };
    assert_eq!(poll(), LoginStatus::Pending);
    assert_eq!(poll(), LoginStatus::Pending);
    let LoginStatus::Done(credentials) = poll() else {
        panic!("the third poll isn't done");
    };
    assert_eq!(
        ask(&web, "GET", &status_path(&started.id), "", false).0,
        404
    );
    assert_eq!(credentials.server.port, Some(ergo.port()));
    assert_eq!(credentials.server.tls, Some(false));
    let account = credentials.account.expect("an account");
    assert_eq!(credentials.nick.as_deref(), Some("dana"));

    // The server takes that account with SASL, as the app signs in.
    let stream = TcpStream::connect(("127.0.0.1", ergo.port())).expect("connecting");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("a timeout");
    let mut write = stream.try_clone().expect("a second handle");
    let plain = base64::engine::general_purpose::STANDARD
        .encode(format!("{0}\0{0}\0{1}", account.name, account.password));
    write!(
        write,
        "CAP REQ :sasl\r\nNICK dana\r\nUSER dana 0 * :dana\r\nAUTHENTICATE PLAIN\r\n\
         AUTHENTICATE {plain}\r\nCAP END\r\n"
    )
    .expect("signing in");
    let mut heard = String::new();
    for line in BufReader::new(stream).lines() {
        let line = line.unwrap_or_else(|error| panic!("never signed in: {error}\n{heard}"));
        assert!(
            !line.contains(" 904 "),
            "the server refused it:\n{heard}{line}"
        );
        if line.contains(" 903 ") {
            break;
        }
        heard.push_str(&line);
        heard.push('\n');
    }

    // A second login finds the account registered already, and is done all the same.
    let (_, _, body) = ask(&web, "POST", START_PATH, &request.to_string(), false);
    let again: LoginStarted = serde_json::from_slice(&body).expect("a login started");
    assert_ne!(again.id, started.id);
}

#[test]
fn a_profile_is_the_accounts_with_its_handle() {
    let web = signin::start(options(None), 0).expect("the stand-in");
    let (_, _, body) = ask(&web, "GET", ME_PATH, "", false);
    let me = serde_json::from_slice::<Me>(&body).expect("a profile");
    assert_eq!(me.orgs[0].handle, "dana");
    assert_eq!(ask(&web, "GET", "/api/media/avatar/dana", "", false).0, 404);
}

/// Getting started's steps are not found unless the stand-in is told which are reached; then
/// they're zirc-proto's progress for those, the next one marked.
#[test]
fn the_steps_are_those_it_is_told_are_reached() {
    let none = signin::start(options(None), 0).expect("the stand-in");
    assert_eq!(ask(&none, "GET", STEPS_PATH, "", false).0, 404);
    let told: Options =
        serde_json::from_value(json!({ "handle": "dana", "steps": [1, 2, 3] })).expect("options");
    let web = signin::start(told, 0).expect("the stand-in");
    let (status, _, body) = ask(&web, "GET", STEPS_PATH, "", false);
    assert_eq!(status, 200);
    let said: Value = serde_json::from_slice(&body).expect("the steps");
    assert_eq!(
        said,
        serde_json::to_value(progress(&[1, 2, 3])).expect("progress")
    );
}

#[test]
fn a_video_is_made_once_then_plays_from_its_copy() {
    let dir = tempfile::tempdir().expect("a directory");
    for (name, bytes) in [
        ("demo.mp4", "video"),
        ("demo-poster.png", "poster"),
        ("party.gif", "gif"),
    ] {
        std::fs::write(dir.path().join(name), bytes).expect("writing");
    }
    let web = signin::start(options(Some(dir.path())), 0).expect("the stand-in");

    // Only an app's account is served a file.
    assert_eq!(ask(&web, "GET", "/m/zirc/demo?w=640", "", false).0, 401);
    // Its picture: the poster, or the file itself when it's a GIF.
    let (_, head, body) = ask(&web, "GET", "/m/zirc/demo?w=640", "", true);
    assert!(head.contains("image/png") && body == b"poster", "{head}");
    let (_, head, body) = ask(&web, "GET", "/m/zirc/party?w=640", "", true);
    assert!(head.contains("image/gif") && body == b"gif", "{head}");
    // To play: being made at first, then where the copy is, which needs no account.
    let (status, _, body) = ask(&web, "GET", "/m/zirc/demo?r=720", "", true);
    assert_eq!(status, 202);
    let making: Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(making, json!({ "processing": true }));
    let (status, head, _) = ask(&web, "GET", "/m/zirc/demo?r=720", "", true);
    assert_eq!(status, 302);
    let copy = head
        .lines()
        .find_map(|line| line.strip_prefix("location: "))
        .expect("a location");
    assert_eq!(copy, format!("{web}/v/demo.mp4"));
    let (status, head, body) = ask(&web, "GET", "/v/demo.mp4", "", false);
    assert!(status == 200 && head.contains("video/mp4") && body == b"video");
    // Nothing above the directory is read.
    assert_eq!(ask(&web, "GET", "/m/zirc/..?w=1", "", true).0, 404);
}
