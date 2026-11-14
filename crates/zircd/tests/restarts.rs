//! zircd's server going away and coming back: a daemon whose Ergo restarts reconnects and
//! says again what its sessions hold.

use zirc_core::ipc::{Request, Response};
use zirc_core::leases::Lease;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

/// The lease `name` as `machine` sees it.
async fn seen(machine: &Machine, name: &str) -> Option<Lease> {
    let Ok(Response::Leases { leases }) = machine.try_request(Request::Leases).await else {
        return None;
    };
    leases.into_iter().find(|lease| lease.name == name)
}

fn held_by(lease: Option<&Lease>, nick: &str, session: &str) -> bool {
    lease
        .and_then(|lease| lease.holder.as_ref())
        .is_some_and(|holder| holder.nick == nick && holder.session == session)
}

/// Waits until `peer` lists cc-a's intent for `src`, the one zircd says for it as it changes
/// files there.
async fn sees_auto_intent(peer: &Peer, what: &str) {
    until_who(
        &peer.machine,
        |channels| {
            channels.iter().any(|c| {
                c.intents
                    .iter()
                    .any(|i| i.session == "cc-a" && i.paths == ["src"])
            })
        },
        &format!("cc-a's intent for src: {what}"),
    )
    .await;
}

/// Waits until `peer` sees alpha's cc-a holding the emulator.
async fn sees_lease(peer: &Peer, what: &str) {
    zirc_test_irc::eventually(&format!("alpha holding the emulator: {what}"), || async {
        held_by(
            seen(&peer.machine, "emulator").await.as_ref(),
            "alpha",
            "cc-a",
        )
    })
    .await;
}

/// Waits until `nicks` are all in `#api`, as a person who joins it sees.
async fn in_api_again(ergo: &zirc_test_ergo::Ergo, nicks: &[&str]) {
    let (ada, mut ada_in) = zirc_test_irc::person(ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#api").await;
    tokio::time::timeout(WAIT, async {
        loop {
            ada.send(zirc_irc::Line::new("NAMES", ["#api"])).unwrap();
            let names = zirc_test_irc::next_matching(&mut ada_in, |line| line.command == "353")
                .await
                .params
                .last()
                .cloned()
                .unwrap_or_default();
            let here = |nick: &str| {
                names
                    .split(' ')
                    .any(|n| n.trim_start_matches(['@', '+']) == nick)
            };
            if nicks.iter().all(|nick| here(nick)) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("everyone in #api again");
}

#[tokio::test]
async fn after_ergo_restarts_a_held_lease_and_an_intent_are_known_to_all_again() {
    let mut ergo = require_ergo!();
    let alpha = Peer::join(Machine::start(&ergo, "alpha").await, Some("cc-a")).await;
    let beta = Peer::join(Machine::start(&ergo, "beta").await, Some("cc-b")).await;
    let _alpha_watch = alpha.watch().await;
    alpha
        .machine
        .request(Request::Lease {
            name: "emulator".into(),
            session: "cc-a".into(),
            seconds: Some(3600),
            pid: None,
            wait: false,
        })
        .await;
    alpha.edits("src/login.rs", "fn login() {}\n");
    sees_auto_intent(&beta, "beta, before").await;
    sees_lease(&beta, "beta, before").await;

    // The server goes away and comes back with nobody in its channels, as for an upgrade.
    // A machine forgets what it heard when its connection drops, so beta knows both again
    // only once alpha, reconnected, has said them anew.
    ergo.restart();
    // Both are in #api on the new server, which only a new connection can be: so each has
    // dropped its old one, and what it heard over it, by now.
    in_api_again(&ergo, &["alpha", "beta"]).await;
    sees_auto_intent(&beta, "beta, after").await;
    sees_lease(&beta, "beta, after").await;
    sees_lease(&alpha, "alpha's own, after").await;
    // And a machine new to the server learns both.
    let gamma = Peer::join(Machine::start(&ergo, "gamma").await, Some("cc-g")).await;
    sees_auto_intent(&gamma, "gamma, new").await;
    sees_lease(&gamma, "gamma, new").await;
}
