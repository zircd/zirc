use zirc_proto::coworkers::Given;
use zirc_proto::leases::Want;
use zirc_proto::roles::{Address, Definition, SeatChange, Vacant, Wake};
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

/// zirc.dev's word: one coworker, the fixer, holding the job of its name, which [`fixer`] is.
fn given() -> Given {
    let fixer = fixer();
    Given {
        staff: serde_json::from_value(serde_json::json!({
            "coworkers": [{"name": "fixer", "jobs": ["fixer"], "repository": fixer.repository}],
            "jobs": [{"name": "fixer", "brief": fixer.brief, "triggers": fixer.triggers}],
        }))
        .unwrap(),
        addresses: vec![Address {
            role: "fixer".into(),
            digest: Address::digest_of("k7Qp2vXe9LmR0aBcDeFgHiJk"),
            signing: std::collections::BTreeMap::new(),
        }],
        version: "1".into(),
    }
}

fn brain() -> Brain {
    let mut brain = Brain::new(Kept::default(), START);
    brain.set_staff(given(), START).unwrap();
    brain
}

fn take(brain: &mut Brain, machine: &str, session: &str, at: u64) -> Vec<Out> {
    sit_at(brain, "fixer", machine, session, at)
}

/// `session` on `machine` asks for coworker `name`'s desk at `at`.
fn sit_at(brain: &mut Brain, name: &str, machine: &str, session: &str, at: u64) -> Vec<Out> {
    brain.heard(
        Input::Lease {
            from: machine.into(),
            owner: Some("henrik".into()),
            event: LeaseEvent::Want(Want {
                // A machine's lease ids differ: its count of them, the name here.
                id: format!(
                    "{machine}-{at}-{}",
                    if name == "fixer" { "1" } else { name }
                ),
                name: roles::lease(name),
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
    assert_eq!(change.words(), "cc-1 sat at fixer's desk in monorepo");
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
        "cc-1 left fixer's desk"
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
        "cc-1 left fixer's desk"
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
    assert!(brain.opened("fixer", "k7Qp2vXe9LmR0aBcDeFgHiJk").is_some());
    assert!(brain.opened("fixer", "k7Qp2vXe9LmR0aBcDeFgHiJ0").is_none());
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
                .wake_job("fixer", None, woke, &each.told(false), LATER)
                .is_empty()
        );
    }
    assert_eq!(brain.standing(LATER).seats[0].waiting, 2);
    brain.drop_waiting(&closed.id);
    assert_eq!(brain.standing(LATER).seats[0].waiting, 1);
    assert_eq!(
        brain.kept.pending[0]
            .waiting
            .woke
            .report
            .as_ref()
            .map(|reported| reported.id.as_str()),
        Some(open.id.as_str())
    );
}

/// A staff of the fixer job, held by `ada` in web, `grace` in api and `hedy`, whom no session
/// has sat at yet, and `linus`, who has no job.
fn staffed() -> Brain {
    let given: Given = serde_json::from_value(serde_json::json!({
        "staff": {
            "coworkers": [
                {"name": "ada", "jobs": ["fixer"], "repository": "web", "added_by": "henrik"},
                {"name": "grace", "jobs": ["fixer"], "repository": "api"},
                {"name": "hedy", "jobs": ["fixer"]},
                {"name": "linus"},
            ],
            "jobs": [{
                "name": "fixer",
                "brief": "Owns a broken master and flaky tests.",
                "triggers": [{"state": "master", "level": "broken"}],
                "reports": ["flaky"],
            }],
        },
        "addresses": [{"role": "fixer", "digest": Address::digest_of("k7Qp2vXe9LmR0aBcDeFgHiJk")}],
        "version": "1",
    }))
    .unwrap();
    let mut brain = Brain::new(Kept::default(), START);
    brain.set_staff(given, START).unwrap();
    brain
}

/// A broken master said in `#ops` about `repository`, heard at `at`.
fn master_broke(brain: &mut Brain, repository: Option<&str>, at: u64) -> Vec<Out> {
    let state = zirc_proto::state::State {
        repo: String::new(),
        name: "master".into(),
        level: Level::Broken,
        why: None,
        owner: None,
        link: None,
        until: None,
    };
    let message = Message {
        repo: repository.map(str::to_owned),
        ..Message::new("ci", state.event())
    };
    brain.heard(
        Input::Said {
            channel: zirc_proto::state::CHANNEL.into(),
            from: "ci".into(),
            sender: Sender::Integration,
            id: None,
            message: Some(message),
            text: String::new(),
        },
        at,
    )
}

#[test]
fn the_first_session_to_sit_sets_a_coworkers_repository_and_zirc_devs_next_word_keeps_it() {
    let mut brain = staffed();
    let standing = brain.standing(LATER);
    assert_eq!(standing.seats[2].role, "hedy");
    assert_eq!(standing.seats[2].repository, "");
    assert_eq!(standing.seats[2].status(""), "Desk empty");
    assert_eq!(standing.staff.coworkers[2].repository, None);
    let (role, _) = brain.sit("hedy", "docs", LATER).unwrap();
    assert_eq!(role.repository, "docs");
    // Sitting again there is fine; from another repository it's refused, in words.
    assert!(brain.sit("hedy", "docs", LATER).is_ok());
    assert_eq!(
        brain.sit("hedy", "web", LATER).unwrap_err(),
        "hedy works in docs: run this in a checkout of it"
    );
    assert_eq!(
        brain.sit("nobody", "web", LATER).unwrap_err(),
        "there's no coworker named nobody"
    );
    // zirc.dev, which gave it none, says the staff again: it stays, and is said back.
    let mut given = Given {
        staff: brain.staff().clone(),
        addresses: Vec::new(),
        version: "2".into(),
    };
    given.staff.coworkers[2].repository = None;
    brain.set_staff(given.clone(), LATER).unwrap();
    assert_eq!(
        brain.standing(LATER).staff.coworkers[2]
            .repository
            .as_deref(),
        Some("docs")
    );
    // Its own word for one replaces it.
    given.staff.coworkers[2].repository = Some("server".into());
    brain.set_staff(given, LATER).unwrap();
    assert_eq!(brain.standing(LATER).seats[2].repository, "server");
}

/// [`staffed`] with ada's and grace's desks taken, by `cc-1` and `cc-2`, and when it's so.
fn seated() -> (Brain, u64) {
    let mut brain = staffed();
    sit_at(&mut brain, "ada", "henrik-mbp", "cc-1", LATER);
    sit_at(&mut brain, "grace", "henrik-mbp", "cc-2", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    (brain, LATER + zirc_proto::leases::SETTLE + 1000)
}

#[test]
fn what_comes_for_a_job_wakes_its_holder_in_that_repository_and_no_other() {
    let (mut brain, at) = seated();
    let outs = master_broke(&mut brain, Some("api"), at);
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].role, "grace");
    let (wake, _) = wake_of(&outs[0]);
    assert_eq!(wake.session, "cc-2");
    assert_eq!(wake.woke.role.as_deref(), Some("grace"));
    // ada, who holds it in web, and linus, who has no job, aren't woken.
    let counts: Vec<u32> = brain
        .standing(at)
        .seats
        .iter()
        .map(|seat| seat.woke_today)
        .collect();
    assert_eq!(counts, [0, 1, 0, 0]);
}

#[test]
fn what_is_about_a_repository_no_holder_works_in_waits_for_one_to_sit_there() {
    let (mut brain, at) = seated();
    assert!(master_broke(&mut brain, Some("docs"), at).is_empty());
    assert_eq!(brain.kept.pending.len(), 1);
    assert!(
        brain
            .standing(at)
            .seats
            .iter()
            .all(|seat| seat.waiting == 0)
    );
    // hedy's first session sets her repository: it's hers to take once one sits.
    let (_, outs) = brain.sit("hedy", "docs", at).unwrap();
    assert!(outs.is_empty());
    assert_eq!(brain.standing(at).seats[2].waiting, 1);
    sit_at(&mut brain, "hedy", "raj-mbp", "cc-3", at);
    let outs = brain.tick(at + zirc_proto::leases::SETTLE);
    let wakes: Vec<(&str, String)> = outs
        .iter()
        .filter_map(|out| Some((out.role.as_str(), Wake::of(&out.message.event)?.0.session)))
        .collect();
    assert_eq!(wakes, [("hedy", "cc-3".to_owned())]);
    assert!(brain.kept.pending.is_empty());
}

#[test]
fn of_several_holders_the_one_woken_longest_ago_takes_what_names_no_repository() {
    let (mut brain, at) = seated();
    // Neither woken yet: the first added. Then the other, then the first again.
    let taker = |brain: &mut Brain, at| master_broke(brain, None, at)[0].role.clone();
    assert_eq!(taker(&mut brain, at), "ada");
    assert_eq!(taker(&mut brain, at + 1), "grace");
    assert_eq!(taker(&mut brain, at + 2), "ada");
    // A direct message is the coworker's alone, whoever is idle.
    let outs = brain.heard(
        Input::ToRole {
            role: "ada".into(),
            from: "henrik".into(),
            account: "henrik".into(),
            sender: Sender::Person,
            id: Some("m1".into()),
            message: None,
            text: "hello".into(),
        },
        at + 3,
    );
    assert_eq!(outs[0].role, "ada");
}

#[test]
fn a_report_is_taken_by_its_jobs_holder_and_said_by_someone() {
    let mut brain = staffed();
    sit_at(&mut brain, "grace", "henrik-mbp", "cc-2", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    let at = LATER + zirc_proto::leases::SETTLE + 1000;
    let item = |repository: Option<&str>| {
        zirc_proto::role_report::OpenItem::opened(
            zirc_proto::role_report::RoleReport {
                repository: repository.map(str::to_owned),
                ..zirc_proto::role_report::RoleReport::new(
                    "fixer",
                    zirc_proto::role_report::Subject::Flaky,
                    "//a:b",
                )
            },
            0,
        )
    };
    let wake = |item| crate::items::Wake {
        role: "fixer".into(),
        item,
        reminder: false,
    };
    let outs = brain.reported(&wake(item(Some("api"))), at);
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].role, "grace");
    // ada holds it in web, with no session at her desk: it waits for her.
    assert!(brain.reported(&wake(item(Some("web"))), at).is_empty());
    assert_eq!(brain.standing(at).seats[0].waiting, 1);
    // Who says a job's report: its holder there, else a holder, else anyone.
    assert_eq!(
        brain.speaker("fixer", Some("api")).as_deref(),
        Some("grace")
    );
    assert_eq!(brain.speaker("fixer", Some("docs")).as_deref(), Some("ada"));
    assert_eq!(brain.speaker("oncall", None).as_deref(), Some("ada"));
}

/// What `session` on `machine` says in `#agents` at `at`, sitting at `desk` or at none.
fn says(brain: &mut Brain, machine: &str, session: &str, desk: Option<&str>, text: &str, at: u64) {
    let message = Message {
        desk: desk.map(str::to_owned),
        ..Message::new(session, Event::Text { body: text.into() })
    };
    brain.heard(
        Input::Said {
            channel: zirc_proto::AGENTS_CHANNEL.into(),
            from: machine.into(),
            sender: Sender::Machine,
            id: None,
            message: Some(message),
            text: text.into(),
        },
        at,
    );
}

/// The fixer's desk with henrik-mbp's `cc-1` at it, and a moment after it sat.
fn fixer_seated() -> (Brain, u64) {
    let mut brain = brain();
    take(&mut brain, "henrik-mbp", "cc-1", LATER);
    brain.tick(LATER + zirc_proto::leases::SETTLE);
    (brain, LATER + zirc_proto::leases::SETTLE + 1000)
}

#[test]
fn a_desks_handover_says_what_its_session_was_on_and_what_woke_it() {
    let (mut brain, at) = fixer_seated();
    write(&mut brain, "main is red", at);
    says(
        &mut brain,
        "henrik-mbp",
        "cc-1",
        Some("fixer"),
        "taking: fix main",
        at,
    );
    // Said by a session at no desk, or of other work: not the coworker's.
    says(
        &mut brain,
        "henrik-mbp",
        "cc-2",
        None,
        "taking: my own thing",
        at,
    );
    says(
        &mut brain,
        "henrik-mbp",
        "cc-1",
        Some("fixer"),
        "looking at the logs",
        at,
    );
    let handover = brain.handover("fixer").unwrap();
    assert_eq!(handover.doing.as_deref(), Some("taking: fix main"));
    let recent: Vec<&str> = handover
        .recent
        .iter()
        .map(|came| came.text.as_str())
        .collect();
    assert_eq!(recent, ["main is red"]);
    assert!(handover.waiting.is_empty());
    assert!(brain.handover("nobody").is_none());
}

#[test]
fn a_desks_handover_says_what_waits_once_its_session_has_gone() {
    let (mut brain, at) = fixer_seated();
    brain.heard(
        Input::Lease {
            from: "henrik-mbp".into(),
            owner: Some("henrik".into()),
            event: LeaseEvent::Release {
                id: format!("henrik-mbp-{LATER}-1"),
                at,
            },
        },
        at,
    );
    let later = at + zirc_proto::leases::SETTLE;
    brain.tick(later);
    write(&mut brain, "still red", later);
    let handover = brain.handover("fixer").unwrap();
    let waiting: Vec<&str> = handover
        .waiting
        .iter()
        .map(|came| came.text.as_str())
        .collect();
    assert_eq!(waiting, ["still red"]);
}

#[test]
fn a_desk_keeps_only_its_latest_wakes() {
    let (mut brain, at) = fixer_seated();
    for n in 0..=desk::RECENT {
        write(&mut brain, &format!("wake {n}"), at + n as u64);
    }
    let handover = brain.handover("fixer").unwrap();
    assert_eq!(handover.recent.len(), desk::RECENT);
    assert_eq!(handover.recent[0].text, "wake 1");
}

/// What ada's nick sends whoever hired her about her empty desk, among `outs`.
fn emptied(outs: &[Out]) -> Vec<(String, Emptied)> {
    outs.iter()
        .filter_map(|out| Some((out.to.clone(), Emptied::of(&out.message.event)?)))
        .collect()
}

#[test]
fn whoever_hired_a_coworker_is_told_once_when_work_comes_to_its_empty_desk() {
    let (mut brain, at) = seated();
    brain.heard(
        Input::Lease {
            from: "henrik-mbp".into(),
            owner: Some("henrik".into()),
            event: LeaseEvent::Release {
                id: format!("henrik-mbp-{LATER}-ada"),
                at,
            },
        },
        at,
    );
    let later = at + zirc_proto::leases::SETTLE;
    // Left with nothing waiting: nobody's told yet.
    assert!(emptied(&brain.tick(later)).is_empty());
    let told = emptied(&master_broke(&mut brain, Some("web"), later));
    let expected = Emptied {
        coworker: "ada".into(),
        waiting: 1,
    };
    assert_eq!(told, [("henrik".to_owned(), expected.clone())]);
    assert_eq!(
        expected.text(),
        "Desk empty · 1 waiting. To seat a session, tell it: \"You're ada. Take your desk.\""
    );
    // Once per emptying: more work tells nobody again.
    assert!(emptied(&master_broke(&mut brain, Some("web"), later + 1)).is_empty());
    assert_eq!(
        brain.standing(later).seats[0].member_line(""),
        "Desk empty · 2 waiting"
    );
}

#[test]
fn a_coworker_nobody_has_left_tells_nobody_of_work_waiting() {
    let mut brain = staffed();
    assert!(emptied(&master_broke(&mut brain, Some("web"), LATER)).is_empty());
}
