//! A session taking on work another session took on already, here or on another machine,
//! hears so at once.

use zirc_core::ipc::{Heard, Watching};
use zirc_proto::Event;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

const CSV: &str = "taking: add a CSV export of orders";
const SPREADSHEET: &str = "taking: let staff download orders as a spreadsheet file (csv)";
const TOLD: &str = "may be on the same work already";

/// Waits for a post saying `text`, and returns what was heard up to it.
async fn until_said(watching: &mut Watching, text: &str) -> Vec<Heard> {
    until_marker(watching, |heard| says(heard, text)).await
}

/// The body of `heard`'s post.
fn body(heard: &Heard) -> &str {
    match heard {
        Heard::Message(delivery) => match &delivery.message.event {
            Event::Text { body } => body,
            _ => "",
        },
        _ => "",
    }
}

#[tokio::test]
async fn a_session_taking_on_another_machines_task_is_told_to_ask_it() {
    let fleet = Fleet::with_sessions(require_ergo!()).await;
    let mut beta_hears = fleet.beta.machine.watch(None).await;
    let mut cc_b = fleet.beta.watch().await;

    fleet.alpha.says(CSV).await;
    until_said(&mut beta_hears, CSV).await;
    // Other work isn't the same work.
    fleet
        .beta
        .says("taking: fix tax rounding on invoices")
        .await;
    fleet.alpha.says("first marker").await;
    let heard = until_said(&mut cc_b, "first marker").await;
    assert!(!heard.iter().any(|h| says(h, TOLD)), "{heard:?}");

    // The same, in other words: told at once, whose it is and to ask it.
    fleet.beta.says(SPREADSHEET).await;
    let heard = until_said(&mut cc_b, TOLD).await;
    let told = heard.last().unwrap();
    let Heard::Message(delivery) = told else {
        unreachable!()
    };
    assert!(delivery.why.is_some(), "it concerns cc-b: {delivery:?}");
    let line = body(told);
    assert!(
        line.starts_with("cc-b: alpha/cc-a may be on the same work already"),
        "{line}"
    );
    assert!(line.contains(CSV), "{line}");
    assert!(
        line.contains("zirc ask --session cc-b --to alpha/cc-a"),
        "{line}"
    );

    // Once alpha's is done, it's no one's to ask about.
    fleet.alpha.says("done: the CSV export").await;
    until_said(&mut beta_hears, "done: the CSV export").await;
    fleet.beta.says("taking: orders csv export, again").await;
    fleet.alpha.says("second marker").await;
    let heard = until_said(&mut cc_b, "second marker").await;
    assert!(!heard.iter().any(|h| says(h, TOLD)), "{heard:?}");
}

#[tokio::test]
async fn a_session_taking_on_a_task_of_another_session_here_is_told() {
    let ergo = require_ergo!();
    let alpha = Peer::join(Machine::start(&ergo, "alpha").await, Some("cc-a")).await;
    let mut cc_x = alpha.machine.watch(Some("cc-x")).await;

    alpha.says(CSV).await;
    post(&alpha.machine, alpha.clone.path(), "cc-x", SPREADSHEET).await;

    let heard = until_said(&mut cc_x, TOLD).await;
    let line = body(heard.last().unwrap());
    assert!(
        line.starts_with("cc-x: alpha/cc-a may be on the same work already"),
        "{line}"
    );
}
