//! Presence as zircd keeps it: a machine's working set goes to people as a TAGMSG, which no
//! history keeps, and is said again when an app asks; a session is watching only while its
//! watch is open, and one whose watch has been gone three minutes gives back its leases, so
//! it stops holding up the next session waiting.

use std::time::Duration;

use tempfile::TempDir;
use zirc_core::ipc::{Request, Response};
use zirc_irc::{Incoming, Line};
use zirc_proto::Event;
use zirc_test_ergo::{Ergo, require_ergo};

mod common;
use common::*;

/// The paths of the next working set `nick` says in `#api`, as a person's client hears it:
/// failing the test if it comes as a PRIVMSG, which history would keep.
async fn next_working_tagmsg(incoming: &mut Incoming, nick: &str) -> Vec<String> {
    let working = |line: &Line| {
        line.source
            .as_deref()
            .is_some_and(|source| source.starts_with(&format!("{nick}!")))
            && line
                .tag(zirc_proto::EVENT_TAG)
                .and_then(|payload| zirc_proto::from_payload(payload).ok())
                .or_else(|| {
                    line.params
                        .get(1)
                        .and_then(|body| zirc_proto::decode(body).ok().flatten())
                })
                .is_some_and(|message| matches!(message.event, Event::Working { .. }))
    };
    let line = zirc_test_irc::next_matching(incoming, working).await;
    assert_eq!(
        line.command, "TAGMSG",
        "a working set kept out of history: {line:?}"
    );
    let message = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap();
    let Event::Working { paths, .. } = message.event else {
        unreachable!()
    };
    paths
}

#[tokio::test]
async fn a_working_set_reaches_people_as_a_tagmsg_and_is_said_again_when_an_app_asks() {
    let ergo = require_ergo!();
    // An hour between working sets said unasked: what's said below is said because someone
    // joined or asked.
    let alpha = Machine::start_with(&ergo, "alpha", "announce_every = 3600\n").await;
    let clone = clone();
    std::fs::write(clone.path().join("README.md"), "alpha's take\n").unwrap();
    alpha.add_repo(None, clone.path()).await.unwrap();
    alpha
        .eventually(
            |status| status.channels.iter().any(|c| c == "#agents"),
            "joining #api",
        )
        .await;
    // Its own scan done, and said: whatever's said from here on is said for Ada.
    until_who(
        &alpha,
        |channels| {
            channels
                .iter()
                .any(|c| c.channel == "#api" && c.members.iter().any(|m| m.nick == "alpha"))
        },
        "alpha's own working set",
    )
    .await;

    // Ada joining hears it.
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#agents").await;
    assert_eq!(
        next_working_tagmsg(&mut ada_in, "alpha").await,
        ["README.md"]
    );

    // An app asking, as one attaching to an account that's always in the channel does, hears
    // it again, though nothing changed and the hour isn't up.
    ada.send(zirc_core::presence::ask("#agents")).unwrap();
    assert_eq!(
        next_working_tagmsg(&mut ada_in, "alpha").await,
        ["README.md"]
    );
}

/// A machine on `clock`, watching a clone of zirc/app, in `#general`.
async fn member(ergo: &Ergo, nick: &str, clock: &zircd::Clock) -> (Machine, TempDir) {
    let hooks = zircd::Hooks {
        clock: clock.clone(),
        ..zircd::Hooks::default()
    };
    let machine = Machine::hooked(ergo, nick, hooks).await;
    let clone = zirc_test_ergo::git::clone("git@github.com:zirc/app.git");
    machine.add_repo(None, clone.path()).await.unwrap();
    machine
        .eventually(
            |status| status.channels.iter().any(|c| c == "#general"),
            "joining #general",
        )
        .await;
    (machine, clone)
}

async fn lease(machine: &Machine, name: &str, session: &str) {
    machine
        .request(Request::Lease {
            name: name.into(),
            session: session.into(),
            seconds: Some(3600),
            pid: None,
            wait: false,
        })
        .await;
}

/// Who holds lease `name` and who waits, by session, as `machine` sees it.
async fn holders(machine: &Machine, name: &str) -> (Vec<String>, Vec<String>) {
    let Ok(Response::Leases { leases }) = machine.try_request(Request::Leases).await else {
        return (Vec::new(), Vec::new());
    };
    let sessions = |list: &[zirc_core::leases::Holder]| -> Vec<String> {
        list.iter().map(|it| it.session.clone()).collect()
    };
    leases
        .iter()
        .find(|lease| lease.name == name)
        .map(|lease| (sessions(&lease.holders), sessions(&lease.waiting)))
        .unwrap_or_default()
}

async fn until_holders(machine: &Machine, name: &str, holding: &[&str], waiting: &[&str]) {
    zirc_test_irc::eventually(
        &format!("{name} held by {holding:?}, {waiting:?} waiting"),
        || async { holders(machine, name).await == (to_strings(holding), to_strings(waiting)) },
    )
    .await;
}

fn to_strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|&it| it.to_owned()).collect()
}

#[tokio::test]
async fn a_session_whose_watch_has_gone_three_minutes_gives_back_its_leases() {
    let ergo = require_ergo!();
    let clock = zircd::Clock::default();
    let (alpha, _alpha_clone) = member(&ergo, "alpha", &clock).await;
    let (beta, _beta_clone) = member(&ergo, "beta", &zircd::Clock::default()).await;

    // cc-gone and cc-here each hold a device, which no idleness gives back, and cc-next on
    // beta waits behind cc-gone.
    let gone = alpha.watch(Some("cc-gone")).await;
    let _here = alpha.watch(Some("cc-here")).await;
    let mut waiting = beta.watch(Some("cc-next")).await;
    lease(&alpha, "emulator", "cc-gone").await;
    lease(&alpha, "simulator", "cc-here").await;
    until_holders(&beta, "emulator", &["cc-gone"], &[]).await;
    lease(&beta, "emulator", "cc-next").await;
    until_holders(&alpha, "emulator", &["cc-gone"], &["cc-next"]).await;
    until_holders(&alpha, "simulator", &["cc-here"], &[]).await;

    // cc-gone's watch ends: it's no longer watching, at once.
    drop(gone);
    alpha
        .eventually(
            |status| {
                !status.watching.contains(&Some("cc-gone".into()))
                    && status.watching.contains(&Some("cc-here".into()))
            },
            "cc-gone no longer watching",
        )
        .await;

    // Three minutes on, by alpha's clock, its lease is given back and cc-next has it. The
    // clock moves a minute at a time, as zircd notices a watch gone at its next look.
    zirc_test_irc::eventually("cc-gone's lease given back", || async {
        clock.advance(Duration::from_mins(1));
        holders(&alpha, "emulator").await.0 == ["cc-next"]
    })
    .await;
    let line = loop {
        let delivery = next(&mut waiting).await;
        if delivery.why.is_some()
            && let Event::Custom { kind, text, .. } = &delivery.message.event
            && kind == zirc_core::leases::LEASE_KIND
        {
            break text.clone();
        }
    };
    assert!(
        line.starts_with("cc-next: lease emulator is yours"),
        "{line}"
    );
    // cc-here, still watching, keeps its own through the same minutes: the look that gave
    // cc-gone's back looked at it too.
    assert_eq!(holders(&alpha, "simulator").await.0, ["cc-here"]);
}

#[tokio::test]
async fn an_integration_joining_hears_no_working_set_said_again() {
    let ergo = require_ergo!();
    let alpha = Machine::start_with(&ergo, "alpha", "announce_every = 3600\n").await;
    let clone = clone();
    std::fs::write(clone.path().join("README.md"), "alpha's take\n").unwrap();
    alpha.add_repo(None, clone.path()).await.unwrap();
    alpha
        .eventually(
            |status| status.channels.iter().any(|c| c == "#agents"),
            "joining #agents",
        )
        .await;
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#agents").await;
    assert_eq!(
        next_working_tagmsg(&mut ada_in, "alpha").await,
        ["README.md"]
    );

    // CI joins for each line it says: nothing is said again for it.
    let (ci, mut ci_in) = zirc_test_irc::person(&ergo, zirc_proto::ci::NICK).await;
    zirc_test_irc::joins(&ci, &mut ci_in, "#agents").await;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            next_working_tagmsg(&mut ada_in, "alpha")
        )
        .await
        .is_err(),
        "said again for CI"
    );
    // A person's app asking still hears it.
    ada.send(zirc_core::presence::ask("#agents")).unwrap();
    assert_eq!(
        next_working_tagmsg(&mut ada_in, "alpha").await,
        ["README.md"]
    );
}
