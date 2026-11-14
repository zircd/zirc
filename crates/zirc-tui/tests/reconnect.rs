//! The client losing its connection the ways real networks lose them, through a proxy that
//! breaks it: how long it takes to notice, and to be back in its channels with what was
//! said meanwhile. Each test prints what it measured.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use zirc_core::connection::{Connector, Policy, Supervisor};
use zirc_irc::{Client, Config, Credentials, Incoming, Line, connect};
use zirc_test_ergo::{Ergo, require_ergo};
use zirc_test_irc::faults::Proxy;
use zirc_test_irc::{config, joins, next_matching, person, register};
use zirc_tui::{App, Session};

/// Runs the session until `done` holds of its app, saying how long that took.
///
/// # Panics
///
/// When it takes longer than `limit`.
async fn within(
    session: &mut Session,
    limit: Duration,
    what: &str,
    done: impl Fn(&App) -> bool,
) -> Duration {
    let started = Instant::now();
    tokio::time::timeout(limit, async {
        while !done(&session.app) {
            session.network().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} took longer than {limit:?}"));
    started.elapsed()
}

fn connected(app: &App) -> bool {
    app.chat.is_connected()
}

fn in_api(app: &App) -> bool {
    app.chat.is_connected() && app.chat.conversation("#api").is_some_and(|c| c.joined)
}

fn in_channel(app: &App, name: &str) -> bool {
    app.chat.conversation(name).is_some_and(|c| c.joined)
}

/// Whether the server's last complete list of its channels has `name`.
fn listed(app: &App, name: &str) -> bool {
    app.chat.listed().iter().any(|(listed, _)| listed == name)
}

/// The messages said in #api, in order.
fn said(app: &App) -> Vec<String> {
    app.chat.conversation("#api").map_or_else(Vec::new, |c| {
        c.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                zirc_core::chat::EntryKind::Message(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Alice in #api, on the server itself; and the client, through `proxy`, in #api too.
async fn setup(ergo: &Ergo, proxy: &Proxy) -> ((Client, Incoming), Session) {
    let alice = alice_in_api(ergo).await;
    let app = App::new("tui", "127.0.0.1", vec!["#api".to_owned()]);
    let session = Session::connect(app, through(ergo, proxy)).await.unwrap();
    (alice, joined(session).await)
}

/// [`Policy::default`], but for pinging a server silent for 2 seconds, not 30, and giving
/// it 2 to answer, not 15: a silent loss is noticed the same way, without the test waiting
/// out the 45 seconds an app takes. As zirc-app's reconnect test has it.
fn quick_to_ping() -> Policy {
    Policy {
        keepalive: Duration::from_secs(2),
        answer_within: Duration::from_secs(2),
        ..Policy::default()
    }
}

/// [`setup`], with the client keeping its connection up as `policy` says.
async fn setup_with(ergo: &Ergo, proxy: &Proxy, policy: Policy) -> ((Client, Incoming), Session) {
    let alice = alice_in_api(ergo).await;
    let app = App::new("tui", "127.0.0.1", vec!["#api".to_owned()]);
    let supervisor = Supervisor::with(
        through(ergo, proxy),
        policy,
        Box::new(|config| Box::pin(async move { connect(&config).await })),
    );
    let session = Session::with(app, supervisor).await.unwrap();
    (alice, joined(session).await)
}

/// `session`, once it's in #api.
async fn joined(mut session: Session) -> Session {
    within(
        &mut session,
        Duration::from_secs(10),
        "joining #api",
        in_api,
    )
    .await;
    session
}

/// Alice in #api, on the server itself.
async fn alice_in_api(ergo: &Ergo) -> (Client, Incoming) {
    let (alice, mut alice_in) = person(ergo, "alice").await;
    alice.join("#api").unwrap();
    next_matching(&mut alice_in, |line| line.command == "JOIN").await;
    (alice, alice_in)
}

/// The client's config, through `proxy`.
fn through(ergo: &Ergo, proxy: &Proxy) -> Config {
    let mut config = config(ergo, "tui");
    config.port = proxy.port();
    config
        .caps
        .extend(zirc_tui::cli::EXTRA_CAPS.iter().map(|&cap| cap.to_owned()));
    config
}

/// `config`, signed in to the account `tui` ([`always_on`]), as a person's apps sign in.
fn signed_in(config: Config) -> Config {
    Config {
        sasl: Some(Credentials {
            account: "tui".into(),
            password: "passphrase-for-tui".into(),
        }),
        ..config
    }
}

/// A server where people's accounts are always-on, as zirc.dev sets them, so that it keeps
/// their channels; with the account `tui`.
async fn always_on() -> Option<Ergo> {
    let ergo = Ergo::start_with(&[("ERGO__ACCOUNTS__MULTICLIENT__ALWAYS_ON", "opt-out")])?;
    register(&ergo, "tui", "passphrase-for-tui").await;
    Some(ergo)
}

/// Cuts the client off, and waits for it to notice: until [`Proxy::heal`].
async fn cut_off(session: &mut Session, proxy: &Proxy) {
    proxy.refuse();
    proxy.reset();
    within(session, Duration::from_secs(5), "noticing", |app| {
        !connected(app)
    })
    .await;
}

/// Says while the client's away, then waits for it to come back with that.
async fn back_with_what_was_said(
    session: &mut Session,
    alice: &Client,
    limit: Duration,
) -> Duration {
    alice.privmsg("#api", "said while you were gone").unwrap();
    let back = within(session, limit, "coming back to #api", in_api).await;
    within(session, Duration::from_secs(5), "catching up", |app| {
        said(app)
            .iter()
            .any(|text| text == "said while you were gone")
    })
    .await;
    let said = said(&session.app);
    let once = said
        .iter()
        .filter(|text| *text == "said while you were gone")
        .count();
    assert_eq!(once, 1, "{said:?}");
    back
}

#[tokio::test]
async fn comes_back_after_the_connection_is_reset() {
    let ergo = require_ergo!();
    let proxy = Proxy::start(ergo.port()).await;
    let ((alice, _alice_in), mut session) = Box::pin(setup(&ergo, &proxy)).await;

    proxy.reset();
    let noticed = within(&mut session, Duration::from_secs(5), "noticing", |app| {
        !connected(app)
    })
    .await;
    let back = back_with_what_was_said(&mut session, &alice, Duration::from_secs(5)).await;
    println!("reset: noticed in {noticed:?}, back in {back:?}");
}

#[tokio::test]
async fn comes_back_after_the_server_restarts() {
    let Some(mut ergo) = Ergo::start() else {
        return;
    };
    let proxy = Proxy::start(ergo.port()).await;
    let (_, mut session) = Box::pin(setup(&ergo, &proxy)).await;

    let started = Instant::now();
    ergo.restart();
    let restarted = started.elapsed();
    within(&mut session, Duration::from_secs(5), "noticing", |app| {
        !connected(app)
    })
    .await;
    let noticed = started.elapsed();
    // History is in the server's memory, and goes with it: only say after it's back.
    let back = within(
        &mut session,
        Duration::from_secs(10),
        "coming back to #api",
        in_api,
    )
    .await;
    println!(
        "restart: the server took {restarted:?}, noticed after {noticed:?}, back {back:?} later"
    );
}

#[tokio::test]
async fn notices_a_connection_lost_silently_and_comes_back() {
    let ergo = require_ergo!();
    let proxy = Proxy::start(ergo.port()).await;
    let ((alice, _alice_in), mut session) =
        Box::pin(setup_with(&ergo, &proxy, quick_to_ping())).await;

    // As after a laptop sleeps: what's sent is lost, and nothing says so.
    proxy.blackhole();
    let noticed = within(&mut session, Duration::from_secs(20), "noticing", |app| {
        !connected(app)
    })
    .await;
    proxy.heal();
    let back = back_with_what_was_said(&mut session, &alice, Duration::from_secs(5)).await;
    println!("silent loss: noticed in {noticed:?}, back in {back:?}");
}

#[tokio::test]
async fn notices_a_silent_loss_at_once_when_told_the_network_changed() {
    let ergo = require_ergo!();
    let proxy = Proxy::start(ergo.port()).await;
    let ((alice, _alice_in), mut session) = Box::pin(setup(&ergo, &proxy)).await;

    proxy.blackhole();
    // As the OS says when the machine wakes or the Wi-Fi changes.
    session.nudge().now();
    let noticed = within(&mut session, Duration::from_secs(12), "noticing", |app| {
        !connected(app)
    })
    .await;
    proxy.heal();
    let back = back_with_what_was_said(&mut session, &alice, Duration::from_secs(5)).await;
    println!("silent loss, nudged: noticed in {noticed:?}, back in {back:?}");
}

#[tokio::test]
async fn comes_back_once_names_resolve_again() {
    let ergo = require_ergo!();
    let proxy = Proxy::start(ergo.port()).await;
    let (alice, mut alice_in) = person(&ergo, "alice").await;
    alice.join("#api").unwrap();
    next_matching(&mut alice_in, |line| line.command == "JOIN").await;

    // While `unresolved`, every attempt asks for a name no resolver knows.
    let unresolved = Arc::new(AtomicBool::new(false));
    let connector: Connector = {
        let unresolved = Arc::clone(&unresolved);
        Box::new(move |mut config: Config| {
            if unresolved.load(Ordering::SeqCst) {
                config.host = "zirc-test.invalid".to_owned();
            }
            Box::pin(async move { zirc_irc::connect(&config).await })
        })
    };
    let supervisor = Supervisor::with(through(&ergo, &proxy), Policy::default(), connector);
    let app = App::new("tui", "127.0.0.1", vec!["#api".to_owned()]);
    let mut session = Session::with(app, supervisor).await.unwrap();
    within(
        &mut session,
        Duration::from_secs(10),
        "joining #api",
        in_api,
    )
    .await;

    unresolved.store(true, Ordering::SeqCst);
    proxy.reset();
    within(&mut session, Duration::from_secs(5), "noticing", |app| {
        !connected(app)
    })
    .await;
    // A few failed attempts, each saying why.
    let outage = Duration::from_secs(6);
    let _ = tokio::time::timeout(outage, async {
        loop {
            session.network().await;
        }
    })
    .await;
    // Between attempts, it says why the last failed.
    within(&mut session, Duration::from_secs(20), "saying why", |app| {
        app.connection
            .describe(chrono::Utc::now())
            .contains("couldn't connect")
    })
    .await;

    unresolved.store(false, Ordering::SeqCst);
    let back = back_with_what_was_said(&mut session, &alice, Duration::from_secs(10)).await;
    println!("names unresolved for {outage:?}: back {back:?} after they resolved again");
}

#[tokio::test]
async fn joins_only_by_choice_and_what_was_left_stays_left_across_reconnects() {
    let ergo = require_ergo!();
    let proxy = Proxy::start(ergo.port()).await;
    let (alice, mut alice_in) = person(&ergo, "alice").await;
    for channel in ["#api/login", "#ops"] {
        alice.join(channel).unwrap();
        next_matching(&mut alice_in, |line| line.command == "JOIN").await;
    }
    let app = App::new(
        "tui",
        "127.0.0.1",
        vec!["#api".to_owned(), "#ops".to_owned()],
    );
    let mut session = Session::connect(app, through(&ergo, &proxy)).await.unwrap();
    within(
        &mut session,
        Duration::from_secs(10),
        "joining #api",
        in_api,
    )
    .await;
    assert!(listed(&session.app, "#api/login"), "listed, to pick");
    assert!(!in_channel(&session.app, "#api/login"), "but not joined");

    // Asked for, then left: a reconnect brings back neither the listing's nor the leaver's.
    within(
        &mut session,
        Duration::from_secs(5),
        "joining #ops",
        |app| in_channel(app, "#ops"),
    )
    .await;
    session.leave("#ops");
    within(
        &mut session,
        Duration::from_secs(5),
        "leaving #ops",
        |app| !in_channel(app, "#ops"),
    )
    .await;
    proxy.reset();
    within(&mut session, Duration::from_secs(5), "noticing", |app| {
        !connected(app)
    })
    .await;
    within(
        &mut session,
        Duration::from_secs(10),
        "coming back to #api",
        |app| in_api(app) && listed(app, "#ops"),
    )
    .await;
    assert!(!in_channel(&session.app, "#api/login"), "never chosen");
    assert!(!in_channel(&session.app, "#ops"), "left stays left");
}

#[tokio::test]
async fn a_channel_left_on_another_device_while_away_stays_left() {
    let Some(ergo) = always_on().await else {
        return;
    };
    let proxy = Proxy::start(ergo.port()).await;
    // The person's desk, on the server itself, and their laptop, through the proxy, which
    // asks for no channel: the server puts it in the desk's.
    let (desk, mut desk_in) = connect(&signed_in(config(&ergo, "tui"))).await.unwrap();
    for channel in ["#api", "#ops"] {
        joins(&desk, &mut desk_in, channel).await;
    }
    let app = App::new("tui", "127.0.0.1", Vec::new());
    let mut laptop = Session::connect(app, signed_in(through(&ergo, &proxy)))
        .await
        .unwrap();
    let both = |app: &App| in_api(app) && in_channel(app, "#ops");
    within(&mut laptop, Duration::from_secs(10), "its channels", both).await;

    // While the laptop's away, the desk leaves #ops for #lunch.
    cut_off(&mut laptop, &proxy).await;
    desk.send(Line::new("PART", ["#ops"])).unwrap();
    joins(&desk, &mut desk_in, "#lunch").await;
    proxy.heal();

    // Once the server has listed #lunch, the laptop has asked to join what it will. What
    // it asks next is answered after that: had it asked for #ops, it would be in it by now.
    let back = |app: &App| in_api(app) && listed(app, "#lunch");
    within(&mut laptop, Duration::from_secs(20), "coming back", back).await;
    laptop.leave("#lunch");
    let left = |app: &App| !in_channel(app, "#lunch");
    within(&mut laptop, Duration::from_secs(5), "leaving #lunch", left).await;
    assert!(!in_channel(&laptop.app, "#ops"), "left stays left");
}
