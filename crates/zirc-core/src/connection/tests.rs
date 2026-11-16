use std::time::Duration;

use chrono::{TimeDelta, Utc};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use super::*;

/// Every wait short, so the tests are quick.
fn quick() -> Policy {
    Policy {
        first_retry: Duration::from_millis(50),
        max_retry: Duration::from_millis(400),
        connect_within: Duration::from_millis(500),
        keepalive: Duration::from_secs(60),
        answer_within: Duration::from_secs(60),
        probe_within: Duration::from_millis(200),
        stable: Duration::from_secs(60),
        wake_check: Duration::from_secs(60),
    }
}

/// A server on loopback that welcomes whoever connects and then does as `then` says.
async fn server(then: Then) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            if then == Then::Mute {
                tokio::spawn(async move { hold(stream).await });
                continue;
            }
            stream.write_all(b":srv 001 me :welcome\r\n").await.unwrap();
            match then {
                // Closed cleanly: dropped with what the client sent unread, it would be reset.
                Then::Close => {
                    stream.shutdown().await.unwrap();
                    tokio::spawn(async move { hold(stream).await });
                }
                Then::Mute | Then::Hang => {
                    tokio::spawn(async move { hold(stream).await });
                }
            }
        }
    });
    (port, task)
}

/// Reads and says nothing, until the client goes.
async fn hold(mut stream: TcpStream) {
    let mut buffer = [0; 1024];
    while stream.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    /// Welcomes, then closes the connection.
    Close,
    /// Welcomes, then never says anything again.
    Hang,
    /// Never even welcomes.
    Mute,
}

fn supervisor(port: u16, policy: Policy) -> Supervisor {
    let mut config = Config::new("127.0.0.1", port, "me");
    config.tls = false;
    config.caps.clear();
    Supervisor::with(
        config,
        policy,
        Box::new(|config| Box::pin(async move { zirc_irc::connect(&config).await })),
    )
}

async fn next(supervisor: &mut Supervisor) -> Event {
    tokio::time::timeout(Duration::from_secs(5), supervisor.next())
        .await
        .expect("something happens in time")
}

#[tokio::test]
async fn connects_again_when_the_server_closes_the_connection() {
    let (port, _server) = server(Then::Close).await;
    let mut supervisor = supervisor(port, quick());
    assert!(matches!(next(&mut supervisor).await, Event::Connecting));
    assert!(matches!(next(&mut supervisor).await, Event::Connected(_)));
    assert!(supervisor.status().is_connected());
    assert!(supervisor.client().is_some());

    let Event::Offline(why) = next(&mut supervisor).await else {
        panic!("not offline");
    };
    assert_eq!(why, "the server closed the connection");
    let Status::Offline {
        since, retry_at, ..
    } = supervisor.status().clone()
    else {
        panic!("{:?}", supervisor.status());
    };
    assert!(
        retry_at - since <= TimeDelta::milliseconds(60),
        "{since} {retry_at}"
    );
    assert!(supervisor.client().is_none());

    assert!(matches!(next(&mut supervisor).await, Event::Connecting));
    assert_eq!(
        supervisor.status(),
        &Status::Connecting {
            attempt: 2,
            was_up: true
        }
    );
    assert!(matches!(next(&mut supervisor).await, Event::Connected(_)));
}

#[tokio::test]
async fn backs_off_while_it_cant_connect_and_says_why() {
    // A port nothing listens on, and that no test is ever given: below every ephemeral range,
    // and listening on it takes root. A port freed by dropping its listener was given to
    // another test's web server, which this test's client then reached (build 3841); one
    // bound and not listening refuses on Linux, but times out on macOS.
    let port = 1;
    let mut supervisor = supervisor(port, quick());
    let mut waits = Vec::new();
    for _ in 0..5 {
        assert!(matches!(next(&mut supervisor).await, Event::Connecting));
        let Event::Offline(why) = next(&mut supervisor).await else {
            panic!("connected to nothing");
        };
        assert!(
            why.starts_with("couldn't connect: connection failed: "),
            "{why}"
        );
        let Status::Offline { retry_at, .. } = supervisor.status() else {
            panic!("{:?}", supervisor.status());
        };
        waits.push((*retry_at - Utc::now()).num_milliseconds());
    }
    // 50, 100, 200, 400, 400ms, each less up to half.
    for (wait, most) in waits.iter().zip([50, 100, 200, 400, 400]) {
        assert!((most / 2 - 5..=most).contains(wait), "{waits:?}");
    }
}

#[tokio::test]
async fn a_nudge_probes_a_dead_connection_and_ends_it_quickly() {
    let (port, _server) = server(Then::Hang).await;
    let mut supervisor = supervisor(port, quick());
    assert!(matches!(next(&mut supervisor).await, Event::Connecting));
    assert!(matches!(next(&mut supervisor).await, Event::Connected(_)));

    let nudge = supervisor.nudge();
    let started = Instant::now();
    nudge.now();
    let Event::Offline(why) = next(&mut supervisor).await else {
        panic!("still connected");
    };
    assert_eq!(why, "the server didn't answer in 200ms");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_nudge_ends_the_wait_between_attempts() {
    let (port, _server) = server(Then::Close).await;
    let mut supervisor = supervisor(
        port,
        Policy {
            first_retry: Duration::from_secs(60),
            ..quick()
        },
    );
    assert!(matches!(next(&mut supervisor).await, Event::Connecting));
    assert!(matches!(next(&mut supervisor).await, Event::Connected(_)));
    assert!(matches!(next(&mut supervisor).await, Event::Offline(_)));

    supervisor.nudged();
    assert!(matches!(next(&mut supervisor).await, Event::Connecting));
    assert_eq!(
        supervisor.status(),
        &Status::Connecting {
            attempt: 1,
            was_up: true
        }
    );
    assert!(matches!(next(&mut supervisor).await, Event::Connected(_)));
}

#[tokio::test]
async fn gives_up_an_attempt_that_takes_too_long() {
    let (port, _server) = server(Then::Mute).await;
    let mut supervisor = supervisor(port, quick());
    assert!(matches!(next(&mut supervisor).await, Event::Connecting));
    let Event::Offline(why) = next(&mut supervisor).await else {
        panic!("connected to a mute server");
    };
    assert_eq!(why, "couldn't connect in 500ms");
}

#[test]
fn says_where_it_is() {
    let now = Utc::now();
    assert_eq!(
        Status::Connecting {
            attempt: 1,
            was_up: false
        }
        .describe(now),
        "connecting…"
    );
    assert_eq!(
        Status::Connecting {
            attempt: 3,
            was_up: false
        }
        .describe(now),
        "connecting… (attempt 3)"
    );
    assert_eq!(Status::Connected { since: now }.describe(now), "connected");
    let offline = Status::Offline {
        since: now - TimeDelta::seconds(150),
        why: "the server closed the connection".into(),
        retry_at: now + TimeDelta::milliseconds(7200),
        was_up: true,
    };
    assert_eq!(
        offline.describe(now),
        "offline for 2m (the server closed the connection), retrying in 8s"
    );
    assert!(
        offline
            .describe(now + TimeDelta::seconds(9))
            .ends_with("retrying now")
    );
}

#[test]
fn says_it_on_a_banner_and_in_a_word() {
    let now = Utc::now();
    assert_eq!(Status::Connected { since: now }.banner(now), None);
    assert_eq!(Status::Connected { since: now }.short(), None);
    assert_eq!(
        Status::Connecting {
            attempt: 2,
            was_up: true
        }
        .banner(now)
        .map(|banner| banner.text)
        .as_deref(),
        Some("Reconnecting…")
    );
    let offline = |for_ms: i64, retry_ms: i64| Status::Offline {
        since: now - TimeDelta::milliseconds(for_ms),
        why: "connection reset by peer".into(),
        retry_at: now + TimeDelta::milliseconds(retry_ms),
        was_up: true,
    };
    // Just dropped: most come back, so no news yet, and never why.
    assert_eq!(
        offline(500, 0)
            .banner(now)
            .map(|banner| banner.text)
            .as_deref(),
        Some("Reconnecting…")
    );
    assert_eq!(
        offline(4000, 2200)
            .banner(now)
            .map(|banner| banner.text)
            .as_deref(),
        Some("Offline · retrying in 3s")
    );
    assert_eq!(
        offline(4000, -10)
            .banner(now)
            .map(|banner| banner.text)
            .as_deref(),
        Some("Offline · retrying now")
    );
    assert_eq!(offline(4000, 0).short(), Some("offline"));
    assert_eq!(
        Status::Connecting {
            attempt: 1,
            was_up: false
        }
        .short(),
        Some("connecting")
    );
}

#[tokio::test]
async fn notices_the_wall_clock_jump_ahead_as_after_sleeping() {
    let mut wake = Wake::new(Duration::from_millis(10));
    assert!(!wake.tick().await);
    // As though a minute passed on the wall clock while the monotonic one stood still.
    wake.last.1 -= Duration::from_secs(60);
    assert!(wake.tick().await);
    assert!(!wake.tick().await);
}

#[test]
fn backoff_takes_up_to_half_off_at_random() {
    let policy = Policy::default();
    let wait = policy.backoff(3, 0.0);
    let waits: Vec<_> = (0..200).map(|_| policy.backoff(3, random())).collect();
    assert!(
        waits.iter().all(|w| (wait / 2..=wait).contains(w)),
        "{waits:?}"
    );
    // Not all the same.
    assert!(waits.iter().any(|w| *w != waits[0]));
}
