//! The client against a real Ergo: a teammate talks in a channel before and after it
//! connects, and what it shows is read off a rendered screen.

use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zirc_core::chat::{BROWSE_CHANNELS, Kind, is_group, members_label};
use zirc_irc::{Config, Credentials, connect};
use zirc_proto::{Event, Message};
use zirc_test_ergo::{Ergo, require_ergo};
use zirc_test_irc::{config, next_matching, person, register};
use zirc_tui::{App, Session, Step, ui};

const WAIT: Duration = Duration::from_secs(10);

/// Runs the session until `done` holds of its app.
async fn until(session: &mut Session, what: &str, done: impl Fn(&App) -> bool) {
    tokio::time::timeout(WAIT, async {
        while !done(&session.app) {
            session.network().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let app = &session.app;
        let state: Vec<_> = app
            .conversations()
            .iter()
            .map(|c| (&c.name, c.joined, c.unread, c.read, c.entries.len()))
            .collect();
        panic!(
            "timed out waiting for {what}, showing {}: {state:?}\n{}",
            app.current().name,
            screen(app)
        )
    });
}

fn typed(session: &mut Session, text: &str) -> bool {
    for c in text.chars() {
        session.key(KeyEvent::from(KeyCode::Char(c)));
    }
    session.key(KeyEvent::from(KeyCode::Enter)) != Step::Quit
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 20)).unwrap();
    terminal.draw(|frame| ui::render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn buffer_at(app: &App, name: &str) -> usize {
    app.conversations()
        .iter()
        .position(|buffer| buffer.is(name))
        .unwrap_or_else(|| panic!("no {name}"))
}

#[tokio::test]
async fn chats_with_history_events_and_mentions() {
    let ergo = require_ergo!();

    // Before the client connects: a message and a commit in #api.
    let (alice, mut alice_in) = connect(&config(&ergo, "alice")).await.unwrap();
    alice.join("#api").unwrap();
    next_matching(&mut alice_in, |line| line.command == "JOIN").await;
    alice.privmsg("#api", "morning all").unwrap();
    let commit = Event::Commit {
        paths: Vec::new(),
        branch: "main".into(),
        sha: "abc123".into(),
        subject: "Fix login".into(),
        repo: None,
    };
    let body = zirc_proto::encode(&Message::new("cc-1", commit)).unwrap();
    alice.privmsg("#api", &body).unwrap();

    let app = App::new("tui", "127.0.0.1", vec!["#api".to_owned()]);
    let mut session = Session::connect(app, config_for_tui(&ergo)).await.unwrap();
    until(&mut session, "#api's history", |app| {
        app.conversations()
            .iter()
            .any(|b| b.is("#api") && b.entries.len() >= 2 && b.has_member("alice"))
    })
    .await;

    // A working set, as zircd sends it once the client is there: known as what alice is
    // changing, never shown as a row.
    let working = Message::new(
        "cc-1",
        Event::Working {
            paths: vec!["src/auth.rs".into()],
            continued: false,
            worktree: None,
            branch: None,
            part: Some([1, 1]),
            base: None,
            committed: Vec::new(),
        },
    );
    alice
        .send(zirc_core::presence::working_line("#api", &working).unwrap())
        .unwrap();
    until(&mut session, "alice's working set", |app| {
        app.chat
            .presence()
            .members("#api")
            .iter()
            .any(|m| m.nick == "alice" && m.paths == ["src/auth.rs"])
    })
    .await;

    let api = buffer_at(&session.app, "#api");
    let _ = session.app.select(api);
    let shown = screen(&session.app);
    // Under a header naming alice, as zirc's other apps show it.
    assert!(
        shown.contains("│ alice  ") && shown.contains("│   morning all"),
        "{shown}"
    );
    // The commit as one quiet line in the channel, in the core's words: who, what and what it was.
    assert!(
        shown.contains("│ ● alice committed: Fix login  (main abc123)"),
        "{shown}"
    );
    assert!(
        !shown.contains("ZIRC") && !shown.contains("uncommitted changes"),
        "{shown}"
    );
    assert!(
        shown.contains("@alice") || shown.contains(" alice"),
        "{shown}"
    );

    // What's typed reaches the channel, and comes back to show.
    assert!(typed(&mut session, "/me waves"));
    let line = next_matching(&mut alice_in, |line| line.command == "PRIVMSG").await;
    assert_eq!(line.params, ["#api", "\x01ACTION waves\x01"]);
    until(&mut session, "our own action", |app| {
        screen(app).contains("* tui waves")
    })
    .await;

    // A mention elsewhere is counted where it was made.
    let _ = session.app.select(0);
    alice.privmsg("#api", "tui: can you review?").unwrap();
    until(&mut session, "the mention", |app| {
        app.conversations()[api].mentions == 1
    })
    .await;
    assert!(screen(&session.app).contains(" 1 mention "));

    // A private message opens a conversation of its own.
    assert!(typed(&mut session, "/msg alice thanks"));
    let line = next_matching(&mut alice_in, |line| line.command == "PRIVMSG").await;
    assert_eq!(line.params, ["alice", "thanks"]);
    assert_eq!(session.app.current().kind, Kind::Query);

    // And a long one goes as one multiline message.
    let long = "word ".repeat(120);
    assert!(typed(&mut session, long.trim()));
    let line = next_matching(&mut alice_in, |line| line.command == "BATCH").await;
    assert!(line.params[0].starts_with('+'), "{line}");

    session.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    session.finish().await;
    next_matching(&mut alice_in, |line| {
        line.command == "QUIT"
            && line
                .source
                .as_deref()
                .is_some_and(|s| s.starts_with("tui!"))
    })
    .await;
}

#[tokio::test]
async fn reading_in_one_app_reads_it_in_the_others() {
    // Ergo keeps an account's read markers once its last connection is gone only for
    // always-on accounts.
    let always_on = ("ERGO__ACCOUNTS__MULTICLIENT__ALWAYS_ON", "opt-out");
    let Some(ergo) = Ergo::start_with(&[always_on]) else {
        return;
    };
    register(&ergo, "tui", "passphrase-for-tui").await;
    let (alice, mut alice_in) = person(&ergo, "alice").await;
    alice.join("#api").unwrap();
    next_matching(&mut alice_in, |line| line.command == "JOIN").await;

    // The same person on two devices, neither showing #api.
    let mut laptop = signed_in(&ergo).await;
    let mut phone = signed_in(&ergo).await;
    for session in [&mut laptop, &mut phone] {
        until(session, "#api joined", |app| {
            app.conversations().iter().any(|c| c.is("#api") && c.joined)
        })
        .await;
        let _ = session.app.select(0);
    }

    alice.privmsg("#api", "anyone there?").unwrap();
    for session in [&mut laptop, &mut phone] {
        until(session, "the unread message", |app| {
            unread(app, "#api") == 1
        })
        .await;
    }

    // Reading it on the laptop reads it on the phone.
    laptop.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT));
    assert!(laptop.app.current().is("#api"));
    until(&mut phone, "the phone to hear it was read", |app| {
        unread(app, "#api") == 0
    })
    .await;

    // Away from both, the server still knows how far it was read: as far as the message
    // the laptop showed.
    let read = laptop.app.current().read.expect("marked read");
    for mut session in [laptop, phone] {
        session.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        session.finish().await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (desk, mut desk_in) = connect(&Config {
        sasl: Some(credentials()),
        ..config(&ergo, "tui")
    })
    .await
    .unwrap();
    desk.join("#api").unwrap();
    let marker = next_matching(&mut desk_in, |line| line.command == "MARKREAD").await;
    assert_eq!(
        marker.params,
        [
            "#api".to_owned(),
            format!(
                "timestamp={}",
                read.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            )
        ]
    );
}

#[tokio::test]
async fn shows_who_is_away() {
    // People's accounts are always-on and auto-away, as zirc.dev sets them.
    let Some(ergo) = Ergo::start_with(&[
        ("ERGO__ACCOUNTS__MULTICLIENT__ALWAYS_ON", "opt-out"),
        ("ERGO__ACCOUNTS__MULTICLIENT__AUTO_AWAY", "opt-out"),
    ]) else {
        return;
    };
    register(&ergo, "tui", "passphrase-for-tui").await;
    let (alice, mut alice_in) = person(&ergo, "alice").await;
    alice.join("#api").unwrap();
    next_matching(&mut alice_in, |line| line.command == "JOIN").await;

    // The person signs in once, and closes the app: they stay, away.
    let mut laptop = signed_in(&ergo).await;
    until(&mut laptop, "#api joined", |app| {
        app.conversations().iter().any(|c| c.is("#api") && c.joined)
    })
    .await;
    laptop.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    laptop.finish().await;

    // Bob, joining, sees them in #api and away.
    let bob = App::new("bob", "127.0.0.1", vec!["#api".to_owned()]);
    let mut bob = Session::connect(
        bob,
        Config {
            nick: "bob".into(),
            ..config_for_tui(&ergo)
        },
    )
    .await
    .unwrap();
    until(&mut bob, "tui away in #api", |app| {
        app.chat
            .people("#api")
            .iter()
            .any(|person| person.nick == "tui" && person.away.is_some())
    })
    .await;

    // Back when an app connects, and away again, saying why, when they say so.
    let mut laptop = signed_in(&ergo).await;
    until(&mut bob, "tui back", |app| app.chat.away("tui").is_none()).await;
    assert!(typed(&mut laptop, "/away lunch"));
    until(&mut bob, "tui at lunch", |app| {
        app.chat.away("tui") == Some("lunch")
    })
    .await;
}

#[tokio::test]
async fn browses_joins_and_leaves_once_asked() {
    let ergo = require_ergo!();
    register(&ergo, "tui", "passphrase-for-tui").await;
    let (alice, mut alice_in) = person(&ergo, "alice").await;
    alice.join("#api/login").unwrap();
    next_matching(&mut alice_in, |line| line.command == "JOIN").await;
    alice
        .privmsg("#api/login", "who's on the login form?")
        .unwrap();

    let mut session = signed_in(&ergo).await;
    // A branch's channel is joined by choice: Browse channels lists it, with who's in it.
    until(&mut session, "the server's channels", |app| {
        app.chat
            .browse()
            .iter()
            .any(|(name, _)| name == "#api/login")
    })
    .await;
    assert!(typed(&mut session, "/browse"));
    let shown = screen(&session.app);
    assert!(shown.contains(BROWSE_CHANNELS), "{shown}");
    assert!(
        shown.contains(&format!("#api/login  {}", members_label(1))),
        "{shown}"
    );

    // /join completes it from that list with Tab.
    for c in "/join #api/l".chars() {
        session.key(KeyEvent::from(KeyCode::Char(c)));
    }
    session.key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(session.app.input.text(), "/join #api/login ");
    session.key(KeyEvent::from(KeyCode::Enter));
    until(&mut session, "#api/login joined", |app| {
        app.conversations()
            .iter()
            .any(|c| c.is("#api/login") && c.joined && c.has_member("alice"))
    })
    .await;

    // /part asks first, over the input, in zirc-chat's words; any other key keeps it.
    let _ = session.app.select(buffer_at(&session.app, "#api/login"));
    assert!(typed(&mut session, "/part"));
    let shown = screen(&session.app);
    assert!(
        shown.contains(&session.app.chat.leave_prompt("#api/login")),
        "{shown}"
    );
    assert!(
        shown.contains("y leaves · any other key keeps it"),
        "{shown}"
    );
    session.key(KeyEvent::from(KeyCode::Char('n')));
    assert!(session.app.leaving().is_none());
    assert!(session.app.current().joined);
    // Asked again, y leaves, and it's back in Browse channels.
    assert!(typed(&mut session, "/part"));
    session.key(KeyEvent::from(KeyCode::Char('y')));
    until(&mut session, "#api/login left", |app| {
        app.conversations()
            .iter()
            .any(|c| c.is("#api/login") && !c.joined)
            && app
                .chat
                .browse()
                .iter()
                .any(|(name, _)| name == "#api/login")
    })
    .await;
}

#[tokio::test]
async fn shows_who_someone_is() {
    let ergo = require_ergo!();
    register(&ergo, "tui", "passphrase-for-tui").await;
    let (alice, mut alice_in) = person(&ergo, "alice").await;
    alice.join("#api").unwrap();
    next_matching(&mut alice_in, |line| line.command == "JOIN").await;
    alice.privmsg("#api", "morning").unwrap();

    let mut session = signed_in(&ergo).await;
    until(&mut session, "alice in #api", |app| {
        app.conversations()
            .iter()
            .any(|c| c.is("#api") && c.has_member("alice"))
    })
    .await;
    assert!(typed(&mut session, "/whois tui"));
    until(&mut session, "Ergo's WHOIS answer", |app| {
        let me = app.chat.profile("tui");
        me.account.as_deref() == Some("tui") && me.signed_on.is_some() && me.idle_since.is_some()
    })
    .await;
    assert_eq!(session.app.profile(), Some("tui"));
    assert_eq!(session.app.chat.profile("alice").channels, ["#api"]);
}

#[tokio::test]
async fn keeps_a_group_to_its_members() {
    // People's accounts are always-on, as zirc.dev sets them.
    let Some(ergo) = Ergo::start_with(&[("ERGO__ACCOUNTS__MULTICLIENT__ALWAYS_ON", "opt-out")])
    else {
        return;
    };
    for name in ["tui", "bob", "carol", "dave"] {
        register(&ergo, name, &format!("passphrase-for-{name}")).await;
    }
    let as_person = |name: &str| Config {
        sasl: Some(Credentials {
            account: name.into(),
            password: format!("passphrase-for-{name}"),
        }),
        ..config(&ergo, name)
    };
    let (bob, mut bob_in) = connect(&as_person("bob")).await.unwrap();
    let (dave, mut dave_in) = connect(&as_person("dave")).await.unwrap();

    // Carol has no app connected: she's invited all the same.
    let mut session = signed_in(&ergo).await;
    until(&mut session, "registered", |app| app.chat.is_connected()).await;
    assert!(typed(&mut session, "/group bob carol"));
    until(&mut session, "the group made and shown", |app| {
        is_group(&app.current().name) && app.current().joined
    })
    .await;
    let group = session.app.current().name.clone();
    let invite = next_matching(&mut bob_in, |line| line.command == "INVITE").await;
    assert_eq!(invite.params, ["bob".to_owned(), group.clone()]);

    // Bob is let in; Dave, who isn't a member, can neither join it nor find it.
    bob.join(&group).unwrap();
    next_matching(&mut bob_in, |line| line.command == "JOIN").await;
    until(&mut session, "bob in the group", |app| {
        app.chat.title(&group) == "bob, carol"
            && app
                .conversations()
                .iter()
                .any(|c| c.is(&group) && c.has_member("bob"))
    })
    .await;
    dave.join(&group).unwrap();
    let refused = next_matching(&mut dave_in, |line| line.command == "473").await;
    assert_eq!(refused.params[1], group);
    dave.send(zirc_irc::Line::new("LIST", [""; 0])).unwrap();
    // The listing has the channels people can see, and the group isn't one of them.
    loop {
        let line = next_matching(&mut dave_in, |line| {
            line.command == "322" || line.command == "323"
        })
        .await;
        assert_ne!(line.params.get(1), Some(&group), "{line:?}");
        if line.command == "323" {
            break;
        }
    }

    // Carol's app, connecting later, joins it from the invitation the server kept.
    let carol = App::new("carol", "127.0.0.1", vec!["#api".to_owned()]);
    let mut carol = Session::connect(
        carol,
        Config {
            caps: config_for_tui(&ergo).caps,
            ..as_person("carol")
        },
    )
    .await
    .unwrap();
    until(&mut carol, "the group joined from history", |app| {
        app.conversations().iter().any(|c| c.is(&group) && c.joined)
    })
    .await;
    assert!(typed(&mut session, "hello both"));
    until(&mut carol, "the group's first message", |app| {
        app.conversations()
            .iter()
            .any(|c| c.is(&group) && c.entries.iter().any(|e| e.from == "tui"))
    })
    .await;
}

fn unread(app: &App, name: &str) -> usize {
    app.conversations()
        .iter()
        .find(|c| c.is(name))
        .map_or(0, |c| c.unread)
}

fn credentials() -> Credentials {
    Credentials {
        account: "tui".into(),
        password: "passphrase-for-tui".into(),
    }
}

/// A session for the account `tui`, as the person's apps sign in.
async fn signed_in(ergo: &Ergo) -> Session {
    let config = Config {
        sasl: Some(credentials()),
        ..config_for_tui(ergo)
    };
    let app = App::new("tui", "127.0.0.1", vec!["#api".to_owned()]);
    Session::connect(app, config).await.unwrap()
}

/// The client's connection, with the capabilities it asks for on top of zirc-irc's.
fn config_for_tui(ergo: &Ergo) -> Config {
    let mut config = config(ergo, "tui");
    config
        .caps
        .extend(zirc_tui::cli::EXTRA_CAPS.iter().map(|&cap| cap.to_owned()));
    config
}
