use zirc_proto::leases::Want;
use zirc_proto::roles::{Address, Definition, Listing, SeatChange, Vacant, Wake};
use zirc_proto::state::Level;

use super::*;

/// When the brain starts, in unix milliseconds.
const START: u64 = 1_800_000_000_000;

/// Long enough after the start for seats to be said.
const LATER: u64 = START + LISTEN_FIRST + 1;

fn fixer() -> Definition {
    serde_json::from_value(serde_json::json!({
        "name": "fixer",
        "repository": "monorepo",
        "brief": "Owns a broken master and flaky tests.",
        "triggers": [{"state": "master", "level": "broken"}, {"landed": ["server/"]}],
    }))
    .unwrap()
}

fn brain() -> Brain {
    let mut brain = Brain::new(Kept::default(), START);
    brain
        .set_listing(Listing {
            roles: vec![fixer()],
            addresses: vec![Address {
                role: "fixer".into(),
                digest: Address::digest_of("k7Qp2vXe9LmR0aBcDeFgHiJk"),
                signing: std::collections::BTreeMap::new(),
            }],
            version: "1".into(),
        })
        .unwrap();
    brain
}

fn take(brain: &mut Brain, machine: &str, session: &str, at: u64) -> Vec<Out> {
    brain.heard(
        Input::Lease {
            from: machine.into(),
            owner: Some("henrik".into()),
            event: LeaseEvent::Want(Want {
                id: format!("{machine}-{at}-1"),
                name: roles::lease("fixer"),
                nick: machine.into(),
                session: session.into(),
                at,
                seconds: zirc_proto::leases::MAX_SECONDS,
            }),
        },
        at,
    )
}

fn write(brain: &mut Brain, text: &str, at: u64) -> Vec<Out> {
    brain.heard(
        Input::ToRole {
            role: "fixer".into(),
            from: "ada".into(),
            account: "ada".into(),
            sender: Sender::Person,
            id: Some("m1".into()),
            message: None,
            text: text.into(),
        },
        at,
    )
}

fn wake_of(out: &Out) -> (Wake, String) {
    let (wake, text) = Wake::of(&out.message.event).expect("a wake");
    (wake, text.to_owned())
}

#[test]
fn a_message_to_a_vacant_role_waits_and_only_its_writer_is_told_how_to_fill_it() {
    let mut brain = brain();
    let outs = write(&mut brain, "main is red", LATER);
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].to, "ada");
    let vacant = Vacant::of(&outs[0].message.event).expect("the vacant card");
    assert_eq!(vacant.seat.waiting, 1);
    assert_eq!(vacant.reply_to.as_deref(), Some("m1"));
    assert_eq!(brain.standing(LATER).seats[0].waiting, 1);
}

#[test]
fn the_next_holder_is_woken_by_what_waited_and_its_writer_told() {
    let mut brain = brain();
    write(&mut brain, "main is red", LATER);
    assert!(take(&mut brain, "henrik-mbp", "cc-1", LATER).is_empty());
    let outs = brain.tick(LATER + zirc_proto::leases::SETTLE);
    let to: Vec<&str> = outs.iter().map(|out| out.to.as_str()).collect();
    assert_eq!(
        to,
        ["ada", "henrik", zirc_proto::AGENTS_CHANNEL, "henrik-mbp"]
    );
    let change = SeatChange::of(&outs[2].message.event).unwrap();
    assert_eq!(change.words(), "cc-1 took the fixer role in monorepo");
    let (wake, text) = wake_of(&outs[3]);
    assert_eq!(
        (wake.session.as_str(), text.as_str()),
        ("cc-1", "main is red")
    );
    assert_eq!(wake.woke.role.as_deref(), Some("fixer"));
    assert_eq!(wake.woke.from.as_deref(), Some("ada"));
    assert_eq!(brain.standing(LATER).seats[0].waiting, 0);
}

#[test]
fn a_message_to_a_held_role_wakes_its_holder_alone() {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    let outs = write(&mut brain, "is it the same flake?", LATER + 5000);
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].to, "henrik-mbp");
    let (wake, _) = wake_of(&outs[0]);
    assert_eq!(wake.woke.what, What::Wrote);
    assert_eq!(wake.woke.line.as_deref(), Some("m1"));
    assert_eq!(brain.standing(LATER + 5000).seats[0].woke_today, 1);
}

#[test]
fn a_trigger_wakes_the_holder_and_its_words_say_the_role() {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    let state = zirc_proto::state::State {
        repo: "monorepo".into(),
        name: "master".into(),
        level: Level::Broken,
        why: None,
        owner: None,
        link: None,
        until: None,
    };
    let message = Message::new("ci", state.event());
    let outs = brain.heard(
        Input::Said {
            channel: "#ops".into(),
            from: "zirc-ci".into(),
            sender: Sender::Integration,
            id: Some("s1".into()),
            message: Some(message),
            text: String::new(),
        },
        LATER + 5000,
    );
    assert_eq!(outs.len(), 1);
    let (wake, text) = wake_of(&outs[0]);
    assert_eq!(text, "master is broken");
    assert_eq!(wake.woke.words(None), "master is broken");
    assert_eq!(wake.woke.heading("main-fix"), "main-fix woke as fixer:");
}

#[test]
fn only_the_holders_session_speaks_as_the_role() {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    let say = |session: &str, account: &str| Input::ToRole {
        role: "fixer".into(),
        from: account.into(),
        account: account.into(),
        sender: Sender::Machine,
        id: None,
        message: Some(Message::new(
            session,
            Event::Custom {
                kind: roles::SAY_KIND.into(),
                data: serde_json::json!({"role": "fixer", "to": "ada"}),
                text: "a fix is on its way".into(),
            },
        )),
        text: String::new(),
    };
    let outs = brain.heard(say("cc-1", "henrik-mbp"), LATER + 5000);
    assert_eq!(outs.len(), 1);
    assert_eq!(
        (outs[0].role.as_str(), outs[0].to.as_str()),
        ("fixer", "ada")
    );
    assert_eq!(outs[0].message.session, "cc-1");
    assert_eq!(outs[0].message.repo.as_deref(), Some("monorepo"));
    assert!(
        brain
            .heard(say("cc-2", "henrik-mbp"), LATER + 5000)
            .is_empty()
    );
    assert!(
        brain
            .heard(say("cc-1", "other-mac"), LATER + 5000)
            .is_empty()
    );
}

#[test]
fn the_holders_wake_for_a_persons_word_to_the_role_is_said_to_them_as_the_role() {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    let woke = |what, role: &str| Woke {
        from: Some("ada".into()),
        role: Some(role.into()),
        ..Woke::new(what, By::Person)
    };
    let said = |session: &str, account: &str, woke: &Woke| Input::ToRole {
        role: "fixer".into(),
        from: account.into(),
        account: account.into(),
        sender: Sender::Machine,
        id: None,
        message: Some(Message::new(session, woke.event())),
        text: String::new(),
    };
    let wrote = woke(What::Wrote, "fixer");
    let outs = brain.heard(said("cc-1", "henrik-mbp", &wrote), LATER + 5000);
    let [out] = &outs[..] else {
        panic!("one line: {outs:?}");
    };
    assert_eq!((out.role.as_str(), out.to.as_str()), ("fixer", "ada"));
    assert_eq!(Woke::of(&out.message.event), Some(wrote.clone()));
    assert_eq!(
        (out.message.session.as_str(), out.message.repo.as_deref()),
        ("cc-1", Some("monorepo"))
    );
    // Only the holder's, only for a person's word to this role, never a room's.
    for (session, account, woke) in [
        ("cc-2", "henrik-mbp", wrote.clone()),
        ("cc-1", "other-mac", wrote.clone()),
        ("cc-1", "henrik-mbp", woke(What::Wrote, "oncall")),
        ("cc-1", "henrik-mbp", woke(What::Asked, "fixer")),
        (
            "cc-1",
            "henrik-mbp",
            Woke {
                channel: Some("#general".into()),
                ..woke(What::Mentioned, "fixer")
            },
        ),
    ] {
        let outs = brain.heard(said(session, account, &woke), LATER + 5000);
        assert!(outs.is_empty(), "{session} {account} {woke:?}: {outs:?}");
    }
}

#[test]
fn a_seat_is_learnt_not_said_while_the_sidecar_first_listens() {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", START);
    assert!(brain.tick(START + 10_000).is_empty());
    // Once it's listened long enough, the holder it learnt is said once: in the room, and
    // to its owner.
    let outs = brain.tick(LATER);
    assert_eq!(outs.len(), 2);
    assert!(brain.tick(LATER + 1000).is_empty());
}

/// A sidecar holding cc-1 as the fixer, restarted at `restarted` with what it kept, its nick
/// joining and asking the machines what they hold at `asked`.
fn restarted(restarted: u64, asked: u64) -> Brain {
    let mut before = brain();
    take(&mut before, "henrik-mbp", "cc-1", LATER);
    assert_eq!(before.tick(LATER + zirc_proto::leases::SETTLE).len(), 2);
    let mut brain = Brain::new(before.kept.clone(), restarted);
    assert!(brain.heard(Input::Asked, asked).is_empty());
    brain
}

#[test]
fn a_restarted_sidecar_keeps_a_held_seat_the_machine_says_again_once_asked() {
    // Its nick joined late, as after a restart: the machine is asked 80 s on, and answers
    // 30 s after that, past the listening counted from the start.
    let restart = LATER + 3_600_000;
    let asked = restart + 80_000;
    let mut brain = restarted(restart, asked);
    assert!(brain.tick(restart + LISTEN_FIRST + 1).is_empty());
    let said_again = brain.heard(
        Input::Lease {
            from: "henrik-mbp".into(),
            owner: Some("henrik".into()),
            event: LeaseEvent::Want(Want {
                id: format!("henrik-mbp-{LATER}-1"),
                name: roles::lease("fixer"),
                nick: "henrik-mbp".into(),
                session: "cc-1".into(),
                at: LATER,
                seconds: zirc_proto::leases::MAX_SECONDS,
            }),
        },
        asked + 30_000,
    );
    // No line, then or once it's listened long enough: the seat never moved.
    assert!(said_again.is_empty());
    assert!(brain.tick(asked + LISTEN_FIRST + 1).is_empty());
    let seat = &brain.standing(asked + LISTEN_FIRST + 1).seats[0];
    assert_eq!(seat.holder.as_ref().unwrap().session.session, "cc-1");
}

#[test]
fn a_restarted_sidecar_says_a_seat_left_once_asked_machines_say_nothing_of_it() {
    let restart = LATER + 3_600_000;
    let asked = restart + 80_000;
    let mut brain = restarted(restart, asked);
    assert!(brain.tick(asked + LISTEN_FIRST - 1).is_empty());
    // One line, in the agents' room: whose the machine is was heard with its lease events,
    // which this sidecar hasn't heard.
    let outs = brain.tick(asked + LISTEN_FIRST + 1);
    let to: Vec<&str> = outs.iter().map(|out| out.to.as_str()).collect();
    assert_eq!(to, [zirc_proto::AGENTS_CHANNEL]);
    assert_eq!(
        SeatChange::of(&outs[0].message.event).unwrap().words(),
        "cc-1 left the fixer role"
    );
    assert!(brain.tick(asked + LISTEN_FIRST + 2000).is_empty());
}

#[test]
fn leaving_says_so_and_keeps_the_last_holder() {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    let outs = brain.heard(
        Input::Lease {
            from: "henrik-mbp".into(),
            owner: Some("henrik".into()),
            event: LeaseEvent::Release {
                id: format!("henrik-mbp-{LATER}-1"),
                at: LATER + 9000,
            },
        },
        LATER + 9000,
    );
    // To its owner, in their conversation with the role, and in the agents' room.
    let to: Vec<&str> = outs.iter().map(|out| out.to.as_str()).collect();
    assert_eq!(to, ["henrik", zirc_proto::AGENTS_CHANNEL]);
    assert_eq!(
        SeatChange::of(&outs[1].message.event).unwrap().words(),
        "cc-1 left the fixer role"
    );
    let seat = &brain.standing(LATER + 9000).seats[0];
    assert!(seat.holder.is_none());
    assert_eq!(seat.last.as_ref().unwrap().machine, "henrik-mbp");
}

#[test]
fn another_machine_cant_give_back_a_seat() {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    let outs = brain.heard(
        Input::Lease {
            from: "other-mac".into(),
            owner: None,
            event: LeaseEvent::Release {
                id: format!("henrik-mbp-{LATER}-1"),
                at: LATER + 9000,
            },
        },
        LATER + 9000,
    );
    assert!(outs.is_empty());
    assert!(brain.standing(LATER + 9000).seats[0].holder.is_some());
}

#[test]
fn how_roles_stand_never_says_their_addresses() {
    let brain = brain();
    assert!(
        brain
            .kept
            .listing
            .opens("fixer", "k7Qp2vXe9LmR0aBcDeFgHiJk")
    );
    assert!(brain.standing(LATER).listing.addresses.is_empty());
}

#[test]
fn a_closed_items_wake_no_longer_waits_for_the_next_holder() {
    let mut brain = brain();
    let item = |key: &str| {
        zirc_proto::role_report::OpenItem::opened(
            zirc_proto::role_report::RoleReport::new(
                "fixer",
                zirc_proto::role_report::Subject::Flaky,
                key,
            ),
            0,
        )
    };
    let (closed, open) = (item("//a:b"), item("//c:d"));
    for each in [&closed, &open] {
        let woke = zirc_proto::woke::Woke::reported(each, false);
        assert!(
            brain
                .wake("fixer", woke, &each.told(false), LATER)
                .is_empty()
        );
    }
    assert_eq!(brain.standing(LATER).seats[0].waiting, 2);
    brain.drop_waiting("fixer", &closed.id);
    assert_eq!(brain.standing(LATER).seats[0].waiting, 1);
    assert_eq!(
        brain.kept.waiting["fixer"][0]
            .woke
            .report
            .as_ref()
            .map(|reported| reported.id.as_str()),
        Some(open.id.as_str())
    );
}
