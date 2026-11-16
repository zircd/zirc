use super::*;
use crate::trim::testing::{add, add_direct, add_direct_item, add_item, history};

const SECOND: i64 = 1_000_000_000;

fn phone(account: &str, settings: &serde_json::Value) -> Phone {
    Phone::of(Subscription {
        account: account.into(),
        endpoint: format!("https://zirc.dev/api/push/{account}"),
        p256dh: "BP".into(),
        auth: "au".into(),
        settings: settings.to_string(),
    })
}

/// Every push the history's rows after `from` make for `phones`, as `pushes_after` decides
/// them, as (account, why, line).
fn decided(path: &Path, from: i64, phones: &[Phone]) -> Vec<(String, String, String)> {
    let connection = crate::trim::open(path).unwrap();
    let (rows, _) = kept::after(&connection, from).unwrap();
    rows.iter()
        .flat_map(|row| {
            let answered = row
                .said
                .tags
                .get("+draft/reply")
                .and_then(|msgid| kept::by_msgid(&connection, msgid).unwrap());
            decide(row, phones, answered.as_ref(), 1_800_000_000)
        })
        .map(|push| {
            let line: Line = push.line.parse().unwrap();
            let why = line.tag(WHY_TAG).unwrap().to_owned();
            (push.account, why, push.line)
        })
        .collect()
}

#[test]
fn pushes_mentions_replies_reactions_and_direct_messages_to_whom_they_concern() {
    let dir = tempfile::tempdir().unwrap();
    let path = history(dir.path());
    add(
        &path,
        "m0",
        "#api",
        SECOND,
        "ada",
        "deploying the login fix",
    );
    add(&path, "m1", "#api", 2 * SECOND, "bob", "ada: does it work?");
    add(&path, "m2", "#api", 3 * SECOND, "bob", "nothing for anyone");
    add_item(
        &path,
        "m3",
        "#api",
        4 * SECOND,
        &serde_json::json!({
            "Type": 1, "Nick": "cy!u@h", "AccountName": "cy",
            "Message": {"Message": "it does"},
            "Tags": {"+draft/reply": "m0", "+zirc/push-why": "all"},
        }),
    );
    add_item(
        &path,
        "m4",
        "#api",
        5 * SECOND,
        &serde_json::json!({
            "Type": 8, "Nick": "bob!u@h", "AccountName": "bob",
            "Message": {"Message": ""}, "Tags": {"+draft/reply": "m0", "+draft/react": "👍"},
        }),
    );
    add_direct(&path, "m5", 6 * SECOND, "bob", "ada", "lunch?");
    add_direct(&path, "m6", 7 * SECOND, "bob", "cy", "not for ada");
    let phones = [
        phone("ada", &serde_json::json!({})),
        phone("dee", &serde_json::json!({})),
    ];
    let pushed = decided(&path, 1, &phones);
    let whys: Vec<(&str, &str)> = pushed
        .iter()
        .map(|(account, why, _)| (account.as_str(), why.as_str()))
        .collect();
    assert_eq!(
        whys,
        [
            ("ada", "mention"),
            ("ada", "reply"),
            ("ada", "reaction"),
            ("ada", "direct"),
        ]
    );
    let lines: Vec<Line> = pushed
        .iter()
        .map(|(_, _, line)| line.parse().unwrap())
        .collect();
    assert_eq!(lines[0].source.as_deref(), Some("bob!u@h"));
    assert_eq!(lines[0].tag("msgid"), Some("m1"));
    assert_eq!(lines[0].tag("account"), Some("bob"));
    assert_eq!(lines[0].tag("time"), Some("1970-01-01T00:00:02.000Z"));
    assert_eq!(lines[0].params, ["#api", "ada: does it work?"]);
    assert_eq!(lines[2].command, "TAGMSG");
    assert_eq!(lines[2].tag("+draft/react"), Some("👍"));
    assert_eq!(lines[2].tag(QUOTE_TAG), Some("deploying the login fix"));
    // Why is the pusher's word alone, never the sender's.
    assert_eq!(
        lines[1]
            .tags
            .iter()
            .filter(|(key, _)| key == WHY_TAG)
            .count(),
        1
    );
    // A direct message as it reaches the person: to their nick.
    assert_eq!(lines[3].params, ["ada", "lunch?"]);
}

#[test]
fn keeps_to_each_phones_levels_and_never_pushes_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let path = history(dir.path());
    add(&path, "m1", "#ops", SECOND, "bob", "deployed");
    add(
        &path,
        "m2",
        "#ops",
        2 * SECOND,
        "ada",
        "ada here, deploying",
    );
    add_direct(&path, "m3", 3 * SECOND, "bob", "ada", "psst");
    let all = phone(
        "ada",
        &serde_json::json!({"levels": {"#ops": "all"}, "muted": {"bob": {"until": "off"}}}),
    );
    let pushed = decided(&path, 0, &[all]);
    let whys: Vec<&str> = pushed.iter().map(|(_, why, _)| why.as_str()).collect();
    assert_eq!(
        whys,
        ["all"],
        "their own words and a muted DM aren't pushed"
    );
}

#[test]
fn reads_only_what_apps_say_to_it() {
    let read = Said::Read {
        target: "#api".into(),
        at: "2026-11-16T12:00:00.000Z".into(),
    };
    let mut line = Line::new("TAGMSG", [PUSHER]);
    line.source = Some("ada!u@h".into());
    line.tags = vec![
        ("account".into(), Some("Ada".into())),
        (TAG.into(), Some(read.tag())),
    ];
    assert_eq!(
        told(&line),
        Some(("ada".into(), "ada".into(), read.clone()))
    );
    let mut anonymous = line.clone();
    anonymous.tags[0].1 = Some("*".into());
    assert_eq!(told(&anonymous), None, "Ergo vouches for no one");
    let mut elsewhere = line.clone();
    elsewhere.params = vec!["#api".into()];
    assert_eq!(told(&elsewhere), None);
}

#[test]
fn starts_from_the_end_of_the_history_and_reads_on() {
    let dir = tempfile::tempdir().unwrap();
    let path = history(dir.path());
    let store = store::path_for(&path);
    add(&path, "m1", "#api", SECOND, "bob", "ada: before the pusher");
    assert!(pushes_after(&path, &store).unwrap().is_empty());
    store::register(
        &store::open(&store).unwrap(),
        &phone("ada", &serde_json::json!({})).subscription,
        1,
    )
    .unwrap();
    add_direct_item(
        &path,
        "m2",
        2 * SECOND,
        "bob",
        "ada",
        &serde_json::json!({"Type": 1, "Nick": "bob!u@h", "AccountName": "bob", "Message": {"Message": "since"}}),
    );
    let pushes = pushes_after(&path, &store).unwrap();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].conversation, "bob");
    assert_eq!(pushes[0].channel, None);
    assert!(pushes_after(&path, &store).unwrap().is_empty(), "each once");
}

/// The file events the pusher waits on come for writes to the history, not to its own store
/// beside it.
#[tokio::test]
async fn hears_the_history_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = history(dir.path());
    let (_watcher, mut written) = watch(&path).unwrap();
    while tokio::time::timeout(Duration::from_millis(300), written.recv())
        .await
        .is_ok()
    {}
    store::open(&store::path_for(&path)).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(500), written.recv())
            .await
            .is_err(),
        "its own store isn't news"
    );
    add(&path, "m1", "#api", SECOND, "bob", "hello");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), written.recv())
            .await
            .is_ok()
    );
}

/// A stand-in push endpoint: answers every POST 201, and hands on each one's headers and body.
async fn endpoint() -> (
    String,
    tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)>,
) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/push/ada", listener.local_addr().unwrap());
    let (posted, posts) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 8192];
            let (head, body) = loop {
                let n = stream.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..n]);
                let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map_or(0, |value| value.trim().parse().unwrap());
                if request.len() >= end + 4 + length || n == 0 {
                    break (head, request[end + 4..].to_vec());
                }
            };
            stream
                .write_all(
                    b"HTTP/1.1 201 Created\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            if posted.send((head, body)).is_err() {
                return;
            }
        }
    });
    (url, posts)
}

/// The pusher against a real Ergo, as a tenant's sidecar runs it: a phone registers over
/// IRC, a direct message to its person is pushed to its endpoint, encrypted for it and signed
/// with the pusher's key, and one read on another device before the delay isn't, while the
/// phone is told to take down what it was pushed.
#[tokio::test]
async fn pushes_through_a_real_ergo_unless_read_elsewhere_first() {
    let Some(ergo) = zirc_test_ergo::Ergo::start_with(crate::environment::HISTORY) else {
        return;
    };
    // What the pusher says of itself, in the test's output when it fails.
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let token = "token";
    let config = crate::say::config("127.0.0.1", ergo.port(), None, token, PUSHER);
    let passphrase = config.sasl.as_ref().unwrap().password.clone();
    zirc_test_irc::register(&ergo, PUSHER, &passphrase).await;
    let (ada, mut ada_hears) = zirc_test_irc::integration(&ergo, "ada").await;
    let (bob, _bob_hears) = zirc_test_irc::integration(&ergo, "bob").await;
    let (url, mut posts) = endpoint().await;
    let pusher = Pusher {
        host: "127.0.0.1".into(),
        port: ergo.port(),
        gate: None,
        token: token.into(),
        history: ergo.history(),
        api: crate::ergo::Api::new("http://127.0.0.1:1", "unused"),
        subscriber: "https://zirc.test/".into(),
        delay: Duration::from_secs(1),
    };
    tokio::spawn(pusher.run(|_| async { true }));

    let (phone, auth) = registered(&ada, &mut ada_hears, &url).await;

    bob.privmsg("ada", "lunch?").unwrap();
    let (head, body) = tokio::time::timeout(Duration::from_secs(20), posts.recv())
        .await
        .expect("a push")
        .unwrap();
    assert!(head.contains("content-encoding: aes128gcm"), "{head}");
    let key = public_key(&ergo.history()).unwrap().to_lowercase();
    assert!(
        head.contains(&format!(", k={key}")),
        "signed with its key: {head}"
    );
    let line: Line = String::from_utf8(webpush::decrypt(&phone, &auth, &body).unwrap())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(line.params, ["ada", "lunch?"]);
    assert_eq!(line.tag(WHY_TAG), Some("direct"));
    assert!(line.source.as_deref().unwrap().starts_with("bob!"));

    // Read on another device before the delay: not pushed, and the phone takes down "lunch?".
    bob.privmsg("ada", "or later?").unwrap();
    let mut read = Line::new("TAGMSG", [PUSHER]);
    let at = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(60);
    let said = Said::Read {
        target: "bob".into(),
        at: format!("{at:.3}"),
    };
    read.tags.push((TAG.into(), Some(said.tag())));
    ada.send(read).unwrap();
    let (_, body) = tokio::time::timeout(Duration::from_secs(20), posts.recv())
        .await
        .expect("the phone told")
        .unwrap();
    let told: Line = String::from_utf8(webpush::decrypt(&phone, &auth, &body).unwrap())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(told.command, "MARKREAD");
    assert_eq!(told.params[0], "bob");
    assert!(
        tokio::time::timeout(Duration::from_secs(3), posts.recv())
            .await
            .is_err(),
        "what was read isn't pushed"
    );
}

/// A phone's keys, registered for `ada`'s account with the pusher, trying until it's there to
/// answer.
async fn registered(
    ada: &zirc_irc::Client,
    ada_hears: &mut zirc_irc::Incoming,
    url: &str,
) -> (p256::SecretKey, [u8; 16]) {
    // The phone's keys, and its registration, until the pusher is there to answer it.
    let phone = p256::SecretKey::from_slice(&[7; 32]).unwrap();
    let public = phone.public_key();
    let p256dh = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        p256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&public, false).as_bytes(),
    );
    let auth = [9_u8; 16];
    let register = Said::Register {
        endpoint: url.to_owned(),
        p256dh,
        auth: base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, auth),
        settings: serde_json::json!({}),
    };
    let answered = |line: &Line| {
        line.command == "TAGMSG"
            && line
                .source
                .as_deref()
                .is_some_and(|s| s.starts_with(PUSHER))
            && line.tag(TAG).and_then(Said::of)
                == Some(Said::Registered {
                    endpoint: url.to_owned(),
                })
    };
    let registered = async {
        loop {
            let mut line = Line::new("TAGMSG", [PUSHER]);
            line.tags.push((TAG.into(), Some(register.tag())));
            ada.send(line).unwrap();
            let heard = tokio::time::timeout(
                Duration::from_millis(500),
                zirc_test_irc::next_matching(ada_hears, answered),
            )
            .await;
            if heard.is_ok() {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), registered)
        .await
        .expect("the pusher to take the phone");
    (phone, auth)
}
