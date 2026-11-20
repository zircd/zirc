//! A machine as a guest's terminal (`zirc login acme/web`): it reaches the organisation's
//! server through a gateway's WebSocket, as the browser does, is in its guest's room alone,
//! posts there, hears what others say there, and renews its grant.

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as Upgrade, Response as Upgraded,
};
use zirc_core::ipc::{Heard, Request, Response, Target, Why};
use zirc_core::paths;
use zirc_core::settings::Settings;
use zirc_irc::Line;
use zirc_proto::Event;
use zirc_proto::guest_terminal::{grant_claims, split_pass};
use zirc_test_ergo::Ergo;
use zirc_test_irc::{eventually, next_matching, person};

mod common;
use common::{Machine, WAIT};

const ROOM: &str = "#web/guest-21042";
const GUEST: &str = "guest-21042";
/// What the gateway names the terminal: its guest's name and its machine's.
const TERMINAL: &str = "guest-21042-mbp";

/// A stand-in for the tenant's guest gateway: it takes the terminal's first line's password
/// and lets it in to Ergo as [`TERMINAL`], as the real one does, keeping each password and
/// every line it sends after it.
struct Gateway {
    url: String,
    passes: Arc<Mutex<Vec<String>>>,
    sent: Arc<Mutex<Vec<Line>>>,
}

impl Gateway {
    async fn start(ergo: &Ergo) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/guest", listener.local_addr().unwrap());
        let passes: Arc<Mutex<Vec<String>>> = Arc::default();
        let sent: Arc<Mutex<Vec<Line>>> = Arc::default();
        let port = ergo.port();
        let (kept, heard) = (Arc::clone(&passes), Arc::clone(&sent));
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                tokio::spawn(session(tcp, port, kept.clone(), heard.clone()));
            }
        });
        Self { url, passes, sent }
    }

    fn passes(&self) -> Vec<String> {
        self.passes.lock().unwrap().clone()
    }

    fn sent(&self) -> Vec<Line> {
        self.sent.lock().unwrap().clone()
    }
}

/// Takes a terminal's WebSocket, in IRC's subprotocol.
#[expect(
    clippy::result_large_err,
    clippy::unnecessary_wraps,
    reason = "tungstenite's callback's type"
)]
fn protocol(_: &Upgrade, mut response: Upgraded) -> Result<Upgraded, ErrorResponse> {
    response
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "text.ircv3.net".parse().unwrap());
    Ok(response)
}

async fn session(
    tcp: TcpStream,
    ergo: u16,
    passes: Arc<Mutex<Vec<String>>>,
    sent: Arc<Mutex<Vec<Line>>>,
) {
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(tcp, protocol).await else {
        return;
    };
    // Its CAP lines wait for Ergo; its password comes first otherwise.
    let mut early = Vec::new();
    loop {
        let Some(Ok(Message::Text(text))) = socket.next().await else {
            return;
        };
        let Ok(line) = text.parse::<Line>() else {
            continue;
        };
        match line.command.as_str() {
            "PASS" => {
                passes.lock().unwrap().push(line.params[0].clone());
                break;
            }
            "CAP" => early.push(text.to_string()),
            _ => {}
        }
    }
    let ergo = TcpStream::connect(("127.0.0.1", ergo)).await.unwrap();
    let (from_ergo, mut to_ergo) = ergo.into_split();
    let mut from_ergo = BufReader::new(from_ergo).lines();
    early.push(format!("NICK {TERMINAL}"));
    early.push("USER guest 0 * :guest".to_owned());
    for line in early {
        to_ergo
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .unwrap();
    }
    loop {
        tokio::select! {
            message = socket.next() => {
                let Some(Ok(Message::Text(text))) = message else { return };
                let Ok(line) = text.parse::<Line>() else { continue };
                // Who it is is the gateway's word, not its own.
                if matches!(line.command.as_str(), "NICK" | "USER" | "PASS") {
                    continue;
                }
                sent.lock().unwrap().push(line);
                if to_ergo.write_all(format!("{text}\r\n").as_bytes()).await.is_err() {
                    return;
                }
            }
            line = from_ergo.next_line() => {
                let Ok(Some(line)) = line else { return };
                if socket.send(Message::text(line)).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// A grant for the guest's terminal, its id `gid`, good until `exp`: zircd reads its claims
/// and never checks it, which the gateway does.
fn grant(gid: &str, exp: u64) -> String {
    zirc_test_ergo::jwt::unsigned(&format!(
        r#"{{"sub":"{GUEST}","room":"{ROOM}","machine":"mbp","key":"k","gid":"{gid}","exp":{exp}}}"#
    ))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The `[guest]` table's settings for the gateway at `url`, a `grant` good until `expires`.
fn guest_settings(url: &str, grant: &str, expires: u64) -> String {
    format!(
        "url = \"{url}\"\norg = \"acme\"\nguest = \"{GUEST}\"\nchannel = \"{ROOM}\"\n\
         public = \"#web\"\ngrant = \"{grant}\"\nexpires = {expires}\n"
    )
}

/// That `pass` is a grant whose id is `gid` and a token for the guest naming it.
fn proves(pass: &str, gid: &str) -> bool {
    let Some((grant, proof)) = split_pass(pass) else {
        return false;
    };
    let claims = zirc_proto::device_key::parse(proof).unwrap().claims;
    grant_claims(grant).unwrap().gid == gid && claims.aud == gid && claims.sub == GUEST
}

#[tokio::test]
async fn a_guests_terminal_talks_in_its_guests_room_alone() {
    let Some(ergo) = Ergo::start() else {
        return;
    };
    let gateway = Gateway::start(&ergo).await;
    // The machine's own organisation first, on the same server, and the terminal after it.
    let machine = Machine::with_config(&format!(
        "nick = \"m\"\nannounce_every = 0\n[server]\nhost = \"127.0.0.1\"\nport = {}\n\
         tls = false\n\n[[org]]\nname = \"acme\"\nweb = \"http://127.0.0.1:9\"\n\
         [org.server]\nhost = \"127.0.0.1\"\n[org.guest]\n{}",
        ergo.port(),
        guest_settings(&gateway.url, &grant("g-1", now() + 86_400), now() + 86_400)
    ))
    .await;
    machine
        .eventually(
            |status| {
                status.orgs.iter().any(|org| {
                    org.name == "acme"
                        && org.connected
                        && org.nick.as_deref() == Some(TERMINAL)
                        && org.channels == [ROOM]
                        && org.guest.as_deref() == Some("acme/web")
                })
            },
            "the terminal in its guest's room",
        )
        .await;
    assert!(gateway.passes().iter().all(|pass| proves(pass, "g-1")));

    let (ada, mut heard_by_ada) = person(&ergo, "ada").await;
    ada.join(ROOM).unwrap();
    next_matching(&mut heard_by_ada, |line| line.command == "366").await;
    let mut watching = machine.watch(Some("cc-1")).await;

    // What someone else says in the room is the session's conversation.
    ada.privmsg(ROOM, "can you look at the build?").unwrap();
    let (said, why) = tokio::time::timeout(WAIT, async {
        loop {
            if let Heard::Message(delivery) = watching.next().await.unwrap().unwrap()
                && delivery.channel == ROOM
            {
                return (delivery.message.event, delivery.why);
            }
        }
    })
    .await
    .unwrap();
    assert!(
        matches!(&said, Event::Text { body } if body == "can you look at the build?"),
        "{said:?}"
    );
    assert_eq!(why, Some(Why::Direct));

    // A post goes to the room whatever it's asked for, and its echo wakes nobody.
    let somewhere = tempfile::tempdir().unwrap();
    let posted = machine
        .request_in(
            Some("acme"),
            Request::Post {
                repo: somewhere.path().to_owned(),
                session: "cc-1".into(),
                text: "on it".into(),
                to: Target::Org,
                app: None,
                everyone: false,
                reply: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(posted, Response::channel(ROOM));
    let line = next_matching(&mut heard_by_ada, |line| line.command == "PRIVMSG").await;
    assert_eq!(line.params[0], ROOM);
    assert!(line.params[1].contains("on it"), "{line}");
    let own = tokio::time::timeout(WAIT, async {
        loop {
            if let Heard::Message(delivery) = watching.next().await.unwrap().unwrap()
                && delivery.from == TERMINAL
            {
                return delivery;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(own.why, None);

    refuses_all_but_its_room(&machine, somewhere.path()).await;
    said_only_in_the_room(&gateway);
}

/// What the gateway would refuse isn't sent: it's refused here. A repository added
/// without naming an organisation is the machine's own organisation's.
async fn refuses_all_but_its_room(machine: &Machine, somewhere: &std::path::Path) {
    for request in [
        Request::Join {
            repo: somewhere.to_owned(),
            topic: "releases".into(),
            session: Some("cc-1".into()),
        },
        Request::Intend {
            repo: somewhere.to_owned(),
            session: "cc-1".into(),
            summary: "the build".into(),
            paths: Vec::new(),
            seconds: 60,
        },
    ] {
        let refused = machine
            .request_in(Some("acme"), request)
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("guest's terminal"), "{refused}");
    }
    let clone = zirc_test_ergo::git::clone("git@github.com:acme/rockets.git");
    let refused = machine
        .add_repo(Some("acme"), clone.path())
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("guest's terminal"), "{refused}");
    machine.add_repo(None, clone.path()).await.unwrap();
    machine
        .eventually(
            |status| status.channels.iter().any(|it| it == "#rockets"),
            "the machine's own organisation in the repository's channel",
        )
        .await;
}

/// That the terminal said nothing anywhere but the room.
fn said_only_in_the_room(gateway: &Gateway) {
    for line in gateway.sent() {
        let target = line.params.first().map(String::as_str);
        match line.command.as_str() {
            "JOIN" | "PRIVMSG" | "TAGMSG" | "PART" => {
                assert_eq!(target, Some(ROOM), "{line}");
            }
            "METADATA" => assert_ne!(line.params.get(1).map(String::as_str), Some("SET")),
            "CAP" | "PING" | "PONG" | "BATCH" | "CHATHISTORY" | "MARKREAD" | "WHO" => {}
            other => panic!("the gateway would refuse {other}: {line}"),
        }
    }
}

#[tokio::test]
async fn a_guests_terminal_renews_its_grant_and_leaves_the_machine_once_its_guest_is_gone() {
    let Some(ergo) = Ergo::start() else {
        return;
    };
    let gateway = Gateway::start(&ergo).await;
    let renewed = format!(
        r##"{{"url":"{}","org":"acme","guest":"{GUEST}","channel":"{ROOM}","public":"#web","grant":"{}","expires":{}}}"##,
        gateway.url,
        grant("g-2", now() + 86_400),
        now() + 86_400
    );
    let (web, served) = zirc_test_ergo::web::serve(vec![(200, renewed)]);
    // A terminal alone, its grant nearly ended.
    let soon = now() + 60;
    let machine = Machine::with_config(&format!(
        "announce_every = 0\nweb = \"{web}\"\n[server]\nhost = \"127.0.0.1\"\n[guest]\n{}",
        guest_settings(&gateway.url, &grant("g-1", soon), soon)
    ))
    .await;
    let status = machine.connected().await;
    assert_eq!(status.guest.as_deref(), Some("acme/web"));
    let asked = served.join().unwrap();
    assert!(asked[0].contains("\"proof\":"), "{}", asked[0]);
    // It connected with the new grant, saved for next time.
    assert!(
        gateway.passes().iter().all(|pass| proves(pass, "g-2")),
        "{:?}",
        gateway.passes().len()
    );
    let text = std::fs::read_to_string(paths::config(machine.home.path())).unwrap();
    let saved = Settings::from_toml(&text).unwrap();
    assert_eq!(
        saved
            .guest
            .as_ref()
            .map(|guest| grant_claims(&guest.grant).unwrap().gid),
        Some("g-2".into())
    );

    // Once the web app lets it in no more, it takes the terminal off the machine, as
    // `zirc logout acme` would, and starts again without it.
    let (web, _served) = zirc_test_ergo::web::serve(vec![(403, r#"{"error":"gone"}"#.into())]);
    let banned = Machine::spawn(&format!(
        "announce_every = 0\nweb = \"{web}\"\n[server]\nhost = \"127.0.0.1\"\n[guest]\n{}",
        guest_settings(&gateway.url, &grant("g-3", soon), soon)
    ));
    let config = paths::config(banned.home.path());
    eventually("the terminal gone from config.toml", || async {
        !config.exists()
    })
    .await;
    let ended = tokio::time::timeout(WAIT, banned.daemon)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(ended, zircd::Ended::Restart);
    assert!(!gateway.passes().iter().any(|pass| proves(pass, "g-3")));
    // Started again, there's nothing left to connect to: it stops, so a login service leaves
    // it stopped.
    let again = zircd::run(banned.home.path().to_path_buf(), std::future::pending()).await;
    assert_eq!(again.unwrap(), zircd::Ended::Stopped);
}
