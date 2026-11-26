//! The client signed in to two organisations, each with its own Ergo: it hears both, says
//! what's unread in the one not shown, and switches between them.

use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use zirc_core::chat::mentions_label;
use zirc_core::orgs::Orgs;
use zirc_irc::connect;
use zirc_test_ergo::Ergo;
use zirc_test_irc::{config, next_matching};
use zirc_tui::{App, Session, network, switch, tell_orgs, ui};

const WAIT: Duration = Duration::from_secs(10);

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 20)).unwrap();
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

/// Takes in what any organisation's connection brings until `done` holds of `orgs`.
async fn until(orgs: &mut Orgs<Session>, what: &str, done: impl Fn(&Orgs<Session>) -> bool) {
    tokio::time::timeout(WAIT, async {
        while !done(orgs) {
            network(orgs).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn hears_each_organisation_and_switches_between_them() {
    let (Some(zirc), Some(globex)) = (Ergo::start(), Ergo::start()) else {
        return;
    };
    let mut orgs = Orgs::new();
    for (org, ergo) in [("zirc", &zirc), ("globex", &globex)] {
        let app = App::new("me", "127.0.0.1", vec!["#general".into()]);
        let session = Session::connect(app, config(ergo, "me")).await.unwrap();
        orgs.insert(org, session);
    }
    let joined = |orgs: &Orgs<Session>, org: &str| {
        orgs.get(org)
            .unwrap()
            .app
            .conversations()
            .iter()
            .any(|c| c.name == "#general" && c.joined)
    };
    until(&mut orgs, "joining both", |orgs| {
        joined(orgs, "zirc") && joined(orgs, "globex")
    })
    .await;
    // As the client does before each draw: globex, not shown, stops marking what's said read.
    tell_orgs(&mut orgs);

    // Someone in globex says something, while zirc is shown.
    let (bob, mut bob_in) = connect(&config(&globex, "bob")).await.unwrap();
    bob.join("#general").unwrap();
    next_matching(&mut bob_in, |line| line.command == "JOIN").await;
    bob.privmsg("#general", "me: hello from globex").unwrap();
    until(&mut orgs, "globex's message", |orgs| {
        orgs.get("globex")
            .unwrap()
            .app
            .conversations()
            .iter()
            .any(|c| c.unread > 0)
    })
    .await;
    tell_orgs(&mut orgs);
    let (shown, session) = orgs.selected().unwrap();
    assert_eq!(shown, "zirc");
    let screen = screen(&session.app);
    assert!(screen.contains(" zirc · #"), "{screen}");
    assert!(
        screen.contains(&format!("globex 1 ({}), Alt-O", mentions_label(1))),
        "{screen}"
    );

    // Alt-O's switch shows the next, as it was; one not signed in to is said so.
    switch(&mut orgs, None);
    tell_orgs(&mut orgs);
    assert_eq!(orgs.selected().unwrap().0, "globex");
    assert_eq!(orgs.selected().unwrap().1.app.current().name, "#general");
    switch(&mut orgs, Some("initech"));
    assert_eq!(orgs.selected().unwrap().0, "globex");
    let said: Vec<String> = orgs
        .selected()
        .unwrap()
        .1
        .app
        .current()
        .entries
        .iter()
        .map(zirc_core::chat::entry_text)
        .collect();
    assert!(
        said.iter()
            .any(|line| line.contains("not signed in to initech: there's zirc, globex")),
        "{said:?}"
    );
}
