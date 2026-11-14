//! Registration against a real Ergo, driven by plain blocking I/O, as any transport can
//! drive it.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;

use zirc_line::Line;
use zirc_line::registration::{Error, Identity, Registered, Registration, Sasl, Step};
use zirc_test_ergo::jwt::Issuer;
use zirc_test_ergo::{Ergo, require_ergo, web};

/// Registers on `port` as `identity`.
fn register(port: u16, identity: Identity) -> Result<Registered, Error> {
    let stream = TcpStream::connect(("127.0.0.1", port)).expect("connecting");
    let mut write = stream.try_clone().expect("a second handle");
    let mut send = |line: &Line| write!(write, "{line}\r\n").expect("writing");
    let mut registration = Registration::new(identity);
    registration.opening().iter().for_each(&mut send);
    for text in BufReader::new(stream).lines() {
        let line: Line = text.expect("reading").parse().expect("a line");
        match registration.step(&line)? {
            Step::Reply(lines) => lines.iter().for_each(&mut send),
            Step::Registered(registered) => return Ok(registered),
        }
    }
    panic!("the server closed the connection");
}

fn caps() -> Vec<String> {
    ["batch", "message-tags", "server-time", "draft/multiline"]
        .map(String::from)
        .to_vec()
}

/// Ergo checking bearer tokens with the web app at `web`, as its oauth2 introspection does.
fn introspecting(web: &str) -> Option<Ergo> {
    let introspection = format!("{web}/introspect");
    Ergo::start_with(&[
        ("ERGO__ACCOUNTS__OAUTH2__ENABLED", "true"),
        ("ERGO__ACCOUNTS__OAUTH2__INTROSPECTION_URL", &introspection),
    ])
}

fn bearer(nick: &str, token: &str) -> Identity {
    let mut identity = Identity::new(nick, caps());
    identity.sasl = Some(Sasl::OAuthBearer {
        token: token.into(),
    });
    identity
}

#[test]
fn registers_with_caps_and_multiline_limits() {
    let ergo = require_ergo!();
    let registered = register(ergo.port(), Identity::new("alice", caps())).unwrap();
    assert_eq!(registered.nick, "alice");
    assert!(registered.caps.contains("server-time"), "{registered:?}");
    assert!(registered.multiline.is_some(), "{registered:?}");
}

#[test]
fn logs_in_with_a_bearer_token_the_server_introspects() {
    let (web, asked) = web::serve(vec![(
        200,
        r#"{"active": true, "username": "alice"}"#.to_owned(),
    )]);
    let Some(ergo) = introspecting(&web) else {
        return;
    };
    let registered = register(ergo.port(), bearer("alice", "web-token"))
        .unwrap_or_else(|err| panic!("{err}\n{}", ergo.log()));
    assert!(registered.caps.contains("sasl"), "{registered:?}");
    let asked = asked.join().unwrap();
    assert!(asked[0].contains("token=web-token"), "{asked:?}");
}

#[test]
fn a_refused_token_fails_registration() {
    let (web, _asked) = web::serve(vec![(200, r#"{"active": false}"#.to_owned())]);
    let Some(ergo) = introspecting(&web) else {
        return;
    };
    assert!(matches!(
        register(ergo.port(), bearer("mallory", "stale")),
        Err(Error::Sasl(_))
    ));
}

/// Seconds since 1970, `ahead` from now.
fn in_secs(ahead: i64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    now.saturating_add_signed(ahead)
}

/// Ergo trusting the JWTs `issuer` signs, as team servers trust zirc.dev's.
fn trusting(issuer: &Issuer) -> Option<Ergo> {
    Ergo::start_with(&[
        ("ERGO__ACCOUNTS__JWT_AUTH__ENABLED", "true"),
        ("ERGO__ACCOUNTS__JWT_AUTH__TOKENS", &issuer.ergo_tokens()),
    ])
}

fn jwt(nick: &str, token: String) -> Identity {
    let mut identity = Identity::new(nick, caps());
    identity.sasl = Some(Sasl::Jwt { token });
    identity
}

#[test]
fn logs_in_with_a_jwt_the_server_checks_itself() {
    let issuer = Issuer::new();
    let Some(ergo) = trusting(&issuer) else {
        return;
    };
    let registered = register(
        ergo.port(),
        jwt("alice", issuer.token("alice", in_secs(300))),
    )
    .unwrap_or_else(|err| panic!("{err}\n{}", ergo.log()));
    assert!(registered.caps.contains("sasl"), "{registered:?}");
}

#[test]
fn an_expired_or_foreign_jwt_fails_registration() {
    let issuer = Issuer::new();
    let Some(ergo) = trusting(&issuer) else {
        return;
    };
    let expired = issuer.token("alice", in_secs(-60));
    assert!(matches!(
        register(ergo.port(), jwt("alice", expired)),
        Err(Error::Sasl(_))
    ));
    let foreign = Issuer::new().token("alice", in_secs(300));
    assert!(matches!(
        register(ergo.port(), jwt("alice", foreign)),
        Err(Error::Sasl(_))
    ));
}
