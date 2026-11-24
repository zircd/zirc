//! The scrollback over a long conversation: how long a frame takes, that it lays out only
//! what it shows, and that what's read stays put while messages arrive below it and older
//! history comes in above it.

use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, TimeZone as _, Utc};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEventKind};
use ratatui::layout::Rect;
use zirc_core::chat::{Chat, Options};

use super::{render, scrollback_size};
use crate::app::{Action, App, HISTORY_LIMIT, Link};
use crate::rows::Zone;

fn later() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap()
}

/// A time `n` messages into the conversation: one every two minutes from 25 October, so
/// the days divide it.
fn at(n: usize) -> String {
    let start = Utc.with_ymd_and_hms(2026, 10, 25, 0, 0, 0).unwrap();
    let minutes = i64::try_from(n).unwrap() * 2;
    (start + chrono::Duration::minutes(minutes))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// What message `n` says: mostly a line, every so often a paragraph that wraps over
/// several rows, as a conversation goes.
fn said(n: usize) -> String {
    match n % 10 {
        3 => format!(
            "message {n}: a longer thought that goes on for a while, the way people write \
             when they explain something, wrapping over several rows on a narrow terminal \
             and a couple on a wide one, with a link https://example.com/{n} in it too"
        ),
        7 => format!("message {n}: a short one / and more after it, {n}"),
        _ => format!("message {n}"),
    }
}

/// Message `n` as the server sends it, from one of a few people, with its id `m<n>`, in
/// the batch `batch` if it's history.
fn privmsg(n: usize, batch: Option<&str>) -> String {
    let nick = ["alice", "bob", "carol"][n % 7 % 3];
    let batch = batch
        .map(|batch| format!("batch={batch};"))
        .unwrap_or_default();
    format!(
        "@{batch}msgid=m{n};time={} :{nick}!u@h PRIVMSG #api :{}",
        at(n),
        said(n)
    )
}

/// A connected app in #api, which keeps as many as `keep` messages.
fn joined(keep: usize) -> App {
    let mut app = App::new("me", "irc.zirc.test", Vec::new());
    app.chat = Chat::new(
        "me",
        "irc.zirc.test",
        Vec::new(),
        Options {
            history: HISTORY_LIMIT,
            max_entries: keep,
            ..Options::default()
        },
    );
    app.chat.clock = later;
    app.zone = Zone::Fixed(FixedOffset::east_opt(0).unwrap());
    app.connected(Link {
        nick: "me".into(),
        metadata: false,
        chathistory: true,
        echo: true,
        read_marker: false,
        trim: true,
        multiline: None,
    });
    for line in [
        ":me!u@h JOIN #api",
        ":s 353 me = #api :me @alice bob carol",
        ":s 366 me #api :End",
    ] {
        app.handle(line.parse().unwrap());
    }
    let _ = app.select(app.chat.position("#api").unwrap());
    app
}

/// Takes in the messages `from` up to `to`, live, some with a reaction.
fn arrive(app: &mut App, from: usize, to: usize) {
    for n in from..to {
        app.handle(privmsg(n, None).parse().unwrap());
        if n % 9 == 0 {
            let react = format!(
                "@+draft/react=👍;+draft/reply=m{n};time={} :bob!u@h TAGMSG #api",
                at(n)
            );
            app.handle(react.parse().unwrap());
        }
    }
}

/// Takes in the page of #api's history from `from` up to `to`, as the server answers
/// `CHATHISTORY`, saying what the app sends back.
fn history(app: &mut App, reference: &str, from: usize, to: usize) -> Vec<String> {
    let mut lines = vec![format!(":s BATCH +{reference} chathistory #api")];
    lines.extend((from..to).map(|n| privmsg(n, Some(reference))));
    lines.push(format!(":s BATCH -{reference}"));
    lines
        .iter()
        .flat_map(|line| app.handle(line.parse().unwrap()))
        .map(|line| line.to_string())
        .collect()
}

/// What `app` shows on a `width` by `height` screen, row by row.
fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    app.view = scrollback_size(app, Rect::new(0, 0, width, height));
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The scrollback's part of `rows`: what's between the sidebar and the member list.
fn scrollback(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.split('│')
                .nth(1)
                .unwrap_or_default()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// What `action` sends, as lines.
fn sent(action: &Action) -> Vec<String> {
    match action {
        Action::Send(lines) => lines.iter().map(ToString::to_string).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn holds_what_is_read_while_messages_arrive_below() {
    let mut app = joined(2_000);
    let _ = history(&mut app, "a", 0, 200);
    let shown = |app: &mut App| scrollback(&screen(app, 100, 24));
    // Reading at the end, what arrives follows.
    arrive(&mut app, 200, 201);
    assert!(
        shown(&mut app)
            .iter()
            .any(|row| row.contains("message 200"))
    );
    // Scrolled up, it doesn't move, however much arrives, until we're back at the end.
    for _ in 0..4 {
        app.key(KeyEvent::from(KeyCode::PageUp));
    }
    let before = shown(&mut app);
    assert!(app.scrolled_back());
    arrive(&mut app, 201, 230);
    assert_eq!(shown(&mut app), before);
    // The wheel moves it three rows, and back.
    let row_of = |rows: &[String], text: &str| rows.iter().position(|row| row.ends_with(text));
    let read = before
        .iter()
        .find(|row| row.contains("   message 1"))
        .unwrap()
        .clone();
    app.pointer(MouseEventKind::ScrollUp, None);
    assert_eq!(
        row_of(&shown(&mut app), &read),
        row_of(&before, &read).map(|row| row + 3)
    );
    app.pointer(MouseEventKind::ScrollDown, None);
    assert_eq!(shown(&mut app), before);
    for _ in 0..40 {
        app.key(KeyEvent::from(KeyCode::PageDown));
    }
    assert!(!app.scrolled_back());
    let end = shown(&mut app);
    assert!(
        end.iter().any(|row| row.contains("message 229")),
        "{end:#?}"
    );
}

#[test]
fn older_history_comes_in_above_without_moving_what_is_read() {
    let mut app = joined(2_000);
    // The newest page, as joining asks for it.
    assert!(history(&mut app, "a", 300, 500).is_empty());
    let (width, height) = (100, 24);
    let shown = |app: &mut App| scrollback(&screen(app, width, height));
    let _ = shown(&mut app);
    // Paging up asks for the page before once it's within two screens of the top, once.
    let mut asked = Vec::new();
    for _ in 0..200 {
        asked.extend(sent(&app.key(KeyEvent::from(KeyCode::PageUp))));
        if !asked.is_empty() {
            break;
        }
    }
    assert_eq!(asked, ["CHATHISTORY BEFORE #api msgid=m300 200"]);
    let before = shown(&mut app);
    assert!(
        before.iter().any(|row| row.contains("message 3")),
        "{before:#?}"
    );
    assert!(sent(&app.key(KeyEvent::from(KeyCode::PageUp))).is_empty());
    let before = shown(&mut app);
    // The page comes in above: what's read stays where it was, to the row.
    let _ = history(&mut app, "b", 100, 300);
    assert_eq!(shown(&mut app), before);
    // And what came in is there to scroll up to.
    let mut seen = false;
    for _ in 0..40 {
        app.key(KeyEvent::from(KeyCode::PageUp));
        seen |= shown(&mut app)
            .iter()
            .any(|row| row.contains("message 299"));
    }
    assert!(seen);
}

#[test]
fn lays_out_only_what_it_shows_and_each_item_once() {
    let mut app = joined(6_000);
    arrive(&mut app, 0, 5_000);
    let _ = screen(&mut app, 80, 40);
    let first = app.laid_afresh();
    // The rows shown, not the five thousand messages.
    assert!(first < 60, "{first}");
    // Drawn again unchanged, as while offline or while someone types, it lays out nothing.
    let _ = screen(&mut app, 80, 40);
    assert_eq!(app.laid_afresh(), first);
    // Typing doesn't either; paging up lays out only what comes into view.
    for c in "hello".chars() {
        app.key(KeyEvent::from(KeyCode::Char(c)));
        let _ = screen(&mut app, 80, 40);
    }
    assert_eq!(app.laid_afresh(), first);
    app.key(KeyEvent::from(KeyCode::PageUp));
    let _ = screen(&mut app, 80, 40);
    // What comes into view, and the two screens above it that say whether to ask for the
    // page of history before.
    assert!(app.laid_afresh() - first < 80, "{}", app.laid_afresh());
}

/// The median and worst of `frames` frames drawing `app` on a `width` by `height` screen.
fn frame_times(app: &mut App, width: u16, height: u16, frames: usize) -> (Duration, Duration) {
    app.view = scrollback_size(app, Rect::new(0, 0, width, height));
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut times: Vec<Duration> = (0..frames)
        .map(|n| {
            // Every other frame after something new arrived, as a busy channel's are.
            if n % 2 == 0 {
                arrive(app, 5_000 + n, 5_001 + n);
            }
            let start = Instant::now();
            terminal.draw(|frame| render(frame, app)).unwrap();
            start.elapsed()
        })
        .collect();
    times.sort();
    (times[times.len() / 2], times[times.len() - 1])
}

/// A frame of a channel of five thousand messages, at the end and scrolled up, is well
/// within one of a 60 Hz screen. Before each item's rows were kept and only those shown
/// laid out, one took 8.7 ms optimised, every frame wrapping every message.
///
/// The 60 Hz budget holds for an optimised build on an idle machine, as the Mac's hitch
/// budget does: `bazel test -c opt --test_env=ZIRC_PERF=1 //crates/zirc-tui:zirc-tui_test`.
/// Otherwise, as in CI's unoptimised run on a busy agent, no ceiling held (50 ms was missed
/// at 57 and 94 ms with nothing changed), and what a frame lays out is counted, whatever the
/// machine, by `lays_out_only_what_it_shows_and_each_item_once`. So unmeasured it only walks
/// the same path through a channel of two hundred for a few frames: the five thousand,
/// timed and unheld, were 28 of the 30 seconds these 120 tests take, and 168 on a CI agent
/// (build 10058).
#[test]
fn a_frame_of_five_thousand_messages_is_quick() {
    let measured = !cfg!(debug_assertions) && std::env::var("ZIRC_PERF").as_deref() == Ok("1");
    let budget = Duration::from_millis(16);
    let (messages, frames) = if measured { (5_000, 40) } else { (200, 4) };
    for (width, height) in [(80, 40), (200, 50)] {
        let mut app = joined(6_000);
        arrive(&mut app, 0, messages);
        let (median, worst) = frame_times(&mut app, width, height, frames);
        println!("at the end, {width}x{height}: median {median:?}, worst {worst:?}");
        assert!(!measured || median < budget, "{median:?}");
        for _ in 0..frames {
            app.key(KeyEvent::from(KeyCode::PageUp));
        }
        let (median, worst) = frame_times(&mut app, width, height, frames);
        println!("scrolled up, {width}x{height}: median {median:?}, worst {worst:?}");
        assert!(!measured || median < budget, "{median:?}");
    }
}
