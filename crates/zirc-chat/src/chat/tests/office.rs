//! The office ([`Chat::office`]): the coworkers at their desks, everyone's sessions at hot
//! desks, a coworker's page and the sheet of an empty desk.

use zirc_proto::coworkers::{Coworker, Job, Staff};
use zirc_proto::desk::{Came, Handover};
use zirc_proto::request::Request;
use zirc_proto::roles::{Holder, Listing, Seat, Standing};
use zirc_proto::session::{Away, Live, SessionCard};
use zirc_proto::woke::Woke;

use super::*;

fn utc() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(0).unwrap()
}

/// Seconds since the epoch, `minutes` past noon.
fn past_noon(minutes: i64) -> u64 {
    u64::try_from((noon() + chrono::Duration::minutes(minutes)).timestamp()).unwrap()
}

/// A wake, as its JSON says it.
fn woke(json: serde_json::Value) -> Woke {
    serde_json::from_value(json).unwrap()
}

fn coworker(name: &str, shown: &str, job: Option<&str>, picture: &str) -> Coworker {
    Coworker {
        name: name.into(),
        shown_name: Some(shown.into()),
        jobs: job.into_iter().map(str::to_owned).collect(),
        repository: Some("web".into()),
        picture: Some(picture.into()),
        hue: Some(60),
        added_by: Some("henrik".into()),
        hired: None,
    }
}

/// Unix seconds at 09:00 UTC on `day` `month` `year`.
fn day(year: i32, month: u32, day: u32) -> u64 {
    u64::try_from(
        Utc.with_ymd_and_hms(year, month, day, 9, 0, 0)
            .unwrap()
            .timestamp(),
    )
    .unwrap()
}

fn job(name: &str) -> Job {
    Job {
        name: name.into(),
        brief: "Does it.".into(),
        triggers: Vec::new(),
        reports: Vec::new(),
    }
}

/// John the fixer, at his desk on henrik's mbp as cc-1 since 08:02; Noor at triage, her desk
/// empty with one waiting, its last session gone at 11:30.
fn standing() -> Standing {
    let john = Holder {
        machine: "henrik-mbp".into(),
        since: past_noon(-238),
        session: SessionCard {
            session: "cc-1".into(),
            name: Some("john-desk".into()),
            title: None,
            tool: Some(zirc_proto::CLAUDE_CODE.into()),
        },
    };
    let noor_last = Holder {
        since: past_noon(-30),
        ..john.clone()
    };
    Standing {
        listing: Listing {
            roles: Vec::new(),
            addresses: Vec::new(),
            version: "1".into(),
        },
        seats: vec![
            Seat {
                role: "john".into(),
                repository: "web".into(),
                holder: Some(john),
                last: None,
                waiting: 0,
                wakes_on: Vec::new(),
                woke_today: 0,
            },
            Seat {
                role: "noor".into(),
                repository: "web".into(),
                holder: None,
                last: Some(noor_last),
                waiting: 1,
                wakes_on: Vec::new(),
                woke_today: 0,
            },
        ],
        staff: Staff {
            coworkers: vec![
                Coworker {
                    hired: Some(day(2026, 10, 27)),
                    ..coworker("john", "John", Some("fixer"), "char-03")
                },
                Coworker {
                    hired: Some(day(2025, 12, 3)),
                    ..coworker("noor", "Noor", Some("triage"), "char-07")
                },
            ],
            jobs: vec![job("fixer"), job("triage")],
        },
    }
}

/// What the machine `nick` says of its sessions: `live` watching, `away` not.
fn live(nick: &str, live: &[&str], away: &[&str]) -> String {
    let away: Vec<(&str, i64)> = away.iter().map(|&s| (s, -2)).collect();
    live_away(nick, live, &away)
}

/// What machine `nick` says of its sessions: `live` online, and each of `away` away since its
/// minutes past noon.
fn live_away(nick: &str, live: &[&str], away: &[(&str, i64)]) -> String {
    let message = Message {
        level: Some(zirc_proto::Level::Debug),
        ..Message::new(
            zirc_proto::NO_SESSION,
            Live {
                sessions: live.iter().map(|&s| s.to_owned()).collect(),
                away: away
                    .iter()
                    .map(|&(s, minutes)| {
                        let since = past_noon(minutes);
                        (s.to_owned(), Away { since, waiting: 0 })
                    })
                    .collect(),
            }
            .event(),
        )
    };
    zirc(nick, &message)
}

/// `#api` with henrik's mbp and ours in it, and the server's standing learnt.
fn office() -> Scene {
    let mut scene = Scene::in_api();
    scene.hears(&[
        ":henrik-mbp!u@machine/henrik JOIN #api",
        ":me-mbp!u@machine/me JOIN #api",
    ]);
    scene.learn_standing(standing());
    scene
}

fn states(office: &Office) -> Vec<(&str, Option<&str>, &str, DeskState)> {
    office
        .coworkers
        .iter()
        .map(|c| (c.name.as_str(), c.job.as_deref(), c.state.as_str(), c.desk))
        .collect()
}

#[test]
fn each_coworker_has_a_card_saying_how_its_desk_stands_and_how_many_sit() {
    let mut scene = office();
    let office = scene.office(utc());
    assert_eq!(
        states(&office),
        [
            ("John", Some("Fixer"), "At desk, idle", DeskState::Idle),
            (
                "Noor",
                Some("Triage"),
                "Desk empty · 1 waiting",
                DeskState::Empty
            ),
        ]
    );
    assert_eq!(office.coworkers_heading, "Coworkers");
    assert_eq!(office.count.as_deref(), Some("1 of 2 at a desk"));
    assert_eq!(office.coworkers[0].label, "John, Fixer, At desk, idle");
    assert!(office.no_coworkers.is_none());
    // Drawn as the character each was given.
    let Face::Agent { character, .. } = &office.coworkers[1].face else {
        panic!("a coworker is drawn as its character");
    };
    assert_eq!(character.as_deref(), Some("char-07"));
    assert!(DeskState::Empty.wants_you() && !DeskState::Idle.wants_you());

    // The session at John's desk takes something on: his card says it.
    let taking = Message::new(
        "cc-1",
        Event::Text {
            body: "taking: Fixing build 431".into(),
        },
    );
    scene.hears(&[
        &live("henrik-mbp", &["cc-1"], &[]),
        &zirc("henrik-mbp", &taking),
    ]);
    assert_eq!(
        states(&scene.office(utc()))[0],
        (
            "John",
            Some("Fixer"),
            "Fixing build 431",
            DeskState::Working
        )
    );
}

#[test]
fn everyones_sessions_at_no_coworkers_desk_are_hot_desks_online_first() {
    let mut scene = office();
    scene.hears(&[
        &live("henrik-mbp", &["cc-1", "cc-2"], &[]),
        &live("me-mbp", &[], &["cc-9"]),
    ]);
    let desks: Vec<(String, String, Option<String>, bool)> = scene
        .office(utc())
        .hot_desks
        .into_iter()
        .map(|desk| (desk.conversation, desk.whose, desk.doing, desk.online))
        .collect();
    // cc-1 sits at John's desk, so it's John's card, not a hot desk.
    assert_eq!(
        desks,
        [
            ("henrik-mbp/cc-2".into(), "henrik's".into(), None, true),
            ("me-mbp/cc-9".into(), "Yours".into(), None, false),
        ]
    );
    let office = scene.office(utc());
    assert_eq!(office.hot_desks_heading, "Hot desks");
    // Each is dealt a character, drawn on no colour.
    let Face::Agent {
        character, ground, ..
    } = &office.hot_desks[0].face
    else {
        panic!("a hot desk's session is an agent");
    };
    assert!(character.is_some() && ground.is_none());
}

#[test]
fn hot_desks_show_who_is_in_the_office_and_fold_the_rest_under_one_line() {
    let mut scene = office();
    // Online, away a minute, away ten minutes, away three hours; cc-7 was named once and
    // then left out of its machine's list: ended.
    scene.hears(&[
        &live_away("me-mbp", &["cc-7"], &[]),
        &live_away(
            "henrik-mbp",
            &["cc-1", "cc-2"],
            &[("cc-3", -1), ("cc-4", -10), ("cc-5", -180)],
        ),
        &live_away("me-mbp", &[], &[]),
    ]);
    let office = scene.office(utc());
    let conversations = |desks: &[HotDesk]| {
        desks
            .iter()
            .map(|d| d.conversation.clone())
            .collect::<Vec<_>>()
    };
    // cc-1 sits at John's desk; the rest are hot desks.
    assert_eq!(
        conversations(&office.hot_desks),
        ["henrik-mbp/cc-2", "henrik-mbp/cc-3"]
    );
    assert_eq!(office.hot_desks_away.as_deref(), Some("2 away"));
    assert_eq!(
        conversations(&office.hot_desks_folded),
        ["henrik-mbp/cc-4", "henrik-mbp/cc-5"]
    );
    assert!(office.hot_desks_folded.iter().all(|d| !d.online));
    // Back online, it's in the office again, and nothing's folded.
    scene.hears(&[&live_away("henrik-mbp", &["cc-2", "cc-4"], &[("cc-3", -1)])]);
    let office = scene.office(utc());
    assert_eq!(
        conversations(&office.hot_desks),
        ["henrik-mbp/cc-2", "henrik-mbp/cc-4", "henrik-mbp/cc-3"]
    );
    assert_eq!(office.hot_desks_away, None);
    assert!(office.hot_desks_folded.is_empty());
}

#[test]
fn an_office_with_no_coworkers_offers_to_hire_one() {
    let scene = Scene::in_api();
    let office = scene.office(utc());
    assert!(office.coworkers.is_empty() && office.count.is_none());
    let none = office.no_coworkers.unwrap();
    assert_eq!(
        (none.line.as_str(), none.hire.as_str()),
        ("No coworkers yet.", "Hire a coworker")
    );
    assert_eq!(
        hire_page("https://zirc.dev/", "acme"),
        "https://zirc.dev/orgs/acme/hire"
    );
}

/// The office, John's desk handing over what he was on and that build 431 woke him at 09:12.
fn john_handed_over() -> Scene {
    let mut scene = office();
    scene.learn_handover(Handover {
        coworker: "john".into(),
        doing: Some("the flaky checkout wait".into()),
        recent: vec![Came {
            woke: woke(serde_json::json!({"what": "build", "by": "ci", "build": "431"})),
            text: String::new(),
            at: past_noon(-168),
        }],
        waiting: Vec::new(),
    });
    scene
}

#[test]
fn a_coworkers_page_says_who_it_is_and_what_its_on() {
    let mut scene = john_handed_over();
    scene.set_org_name(Some("Home"));
    let page = scene.coworker_page("John", utc()).unwrap();
    assert_eq!(
        (
            page.name.as_str(),
            page.handle.as_str(),
            page.job.as_deref()
        ),
        ("John", "@john · hired 27 Oct", Some("Fixer"))
    );
    // Its page is headed as its badge is.
    assert_eq!(
        (page.office.as_str(), page.staff.as_str()),
        ("Home", "Staff")
    );
    assert_eq!(page.pill, "At desk, idle");
    assert_eq!(page.now.as_deref(), Some("the flaky checkout wait"));
    assert!(page.now_under.unwrap().starts_with("Woke 09:12 · "));
    assert_eq!(page.message, "Message John");
    assert_eq!(
        page.recent
            .iter()
            .map(|came| came.at.as_str())
            .collect::<Vec<_>>(),
        ["09:12"]
    );
    assert!(scene.coworker_page("nobody", utc()).is_none());
}

#[test]
fn a_coworkers_page_says_its_job_desk_and_who_hired_it() {
    let page = john_handed_over().coworker_page("john", utc()).unwrap();
    let about: Vec<(&str, &str)> = page
        .about
        .iter()
        .map(|row| (row.label.as_str(), row.text.as_str()))
        .collect();
    assert_eq!(about[0].0, "Job");
    assert!(about[0].1.starts_with("Owns a red master"));
    assert!(about[0].1.contains(" Wakes on master breaking"));
    assert_eq!(
        about[1],
        ("Desk", "Claude Code on henrik's mbp, in web · since 08:02")
    );
    assert_eq!(about[2], ("Hired by", "henrik"));
}

/// The office, Raj's message waiting at Noor's empty desk since 07:55.
fn noor_waits() -> Scene {
    let mut scene = office();
    scene.learn_handover(Handover {
        coworker: "noor".into(),
        doing: None,
        recent: Vec::new(),
        waiting: vec![Came {
            woke: woke(serde_json::json!({"what": "wrote", "by": "person", "from": "raj"})),
            text: "Checkout button overlaps the total".into(),
            at: past_noon(-245),
        }],
    });
    scene
}

#[test]
fn an_empty_desk_says_what_waits_for_it() {
    let scene = noor_waits();
    assert!(scene.empty_desk("john", utc()).is_none());
    let sheet = scene.empty_desk("noor", utc()).unwrap();
    assert_eq!(sheet.title, "Noor's desk is empty");
    assert_eq!(sheet.under, "Triage · last session ended 11:30");
    assert_eq!(sheet.waiting_heading.as_deref(), Some("1 waiting for Noor"));
    assert_eq!(
        (sheet.waiting[0].at.as_str(), sheet.waiting[0].text.as_str()),
        (
            "07:55",
            "raj wrote to it: Checkout button overlaps the total"
        )
    );
    // Its page says so too.
    let page = scene.coworker_page("noor", utc()).unwrap();
    assert_eq!(page.pill, "Desk empty · 1 waiting");
    assert_eq!(page.about[1].text, "Desk empty · 1 waiting · in web");
}

#[test]
fn an_empty_desk_says_the_sentence_that_seats_a_session() {
    let sheet = noor_waits().empty_desk("noor", utc()).unwrap();
    let how: Vec<(&str, bool)> = sheet
        .how
        .iter()
        .map(|span| (span.text.as_str(), span.mono))
        .collect();
    assert_eq!(
        how,
        [
            (
                "On your computer, start a Claude Code or Codex session in ",
                false
            ),
            ("web", true),
            (" and say:", false),
        ]
    );
    assert_eq!(sheet.sentence, "You're Noor. Take your desk.");
    assert_eq!((sheet.copy.as_str(), sheet.done.as_str()), ("Copy", "Done"));
    assert_eq!(sheet.message, "Message Noor");
    assert_eq!(
        sheet.foot,
        "A message waits too, and reaches Noor at the desk."
    );
}

#[test]
fn a_server_that_says_no_staff_shows_the_roles_its_lines_name() {
    let mut scene = Scene::in_api();
    let host = zirc_proto::names::role_host("fixer");
    scene.hears(&[&format!(":fixer!u@{host} JOIN #api")]);
    let office = scene.office(utc());
    assert_eq!(
        states(&office),
        [("fixer", None, "Desk empty", DeskState::Empty)]
    );
}

#[test]
fn a_coworkers_card_is_its_office_badge() {
    let mut scene = office();
    scene.set_org_name(Some("Home"));
    let office = scene.office(utc());
    assert_eq!(
        (
            office.name.as_str(),
            office.staff.as_str(),
            office.hot_desk.as_str()
        ),
        ("Home", "Staff", "Hot desk")
    );
    let handles: Vec<&str> = office.coworkers.iter().map(|c| c.handle.as_str()).collect();
    // Hired this year says the day; another year says it too.
    assert_eq!(
        handles,
        ["@john · hired 27 Oct", "@noor · hired 3 Dec 2025"]
    );
    // Noor's empty desk says what waits, in the warning tone.
    let noor = &office.coworkers[1];
    assert_eq!(noor.state, "Desk empty · 1 waiting");
    assert!(noor.desk.wants_you());
}

#[test]
fn an_unnamed_coworkers_badge_says_its_job_once_and_offers_to_name_it() {
    // John came over from the roles: no shown name, his nick his job's.
    let mut standing = standing();
    standing.staff.coworkers[0] = Coworker {
        name: "fixer".into(),
        shown_name: None,
        ..standing.staff.coworkers[0].clone()
    };
    standing.seats[0].role = "fixer".into();
    let mut scene = office();
    scene.learn_standing(standing);
    let office = scene.office(utc());
    let fixer = &office.coworkers[0];
    assert_eq!((fixer.name.as_str(), fixer.job.as_deref()), ("Fixer", None));
    assert_eq!(fixer.give_a_name.as_deref(), Some("Give them a name"));
    assert!(fixer.label.starts_with("Fixer, "), "{}", fixer.label);
    // Named, the badge says the name and the job, and offers nothing.
    let noor = &office.coworkers[1];
    assert_eq!(
        (noor.name.as_str(), noor.job.as_deref()),
        ("Noor", Some("Triage"))
    );
    assert_eq!(noor.give_a_name, None);
    let page = scene.coworker_page("fixer", utc()).unwrap();
    assert_eq!((page.name.as_str(), page.job.as_deref()), ("Fixer", None));
    assert_eq!(page.give_a_name.as_deref(), Some("Give them a name"));
    assert_eq!(
        coworker_web_page("https://zirc.dev/", "acme", "Fixer"),
        "https://zirc.dev/orgs/acme/coworkers/fixer#name"
    );
}

#[test]
fn a_hot_desks_badge_says_whose_it_is_its_tool_and_what_its_doing() {
    let mut scene = office();
    let taking = Message {
        name: Some("checkout-form".into()),
        tool: Some(zirc_proto::CLAUDE_CODE.into()),
        ..Message::new(
            "cc-2",
            Event::Text {
                body: "taking: Changing the payment form".into(),
            },
        )
    };
    scene.hears(&[
        &live("henrik-mbp", &["cc-1", "cc-2"], &[]),
        &zirc("henrik-mbp", &taking),
    ]);
    let office = scene.office(utc());
    let desk = &office.hot_desks[0];
    assert_eq!(
        (desk.name.as_str(), desk.whose.as_str()),
        ("checkout-form", "henrik's · Claude Code")
    );
    assert_eq!(desk.doing.as_deref(), Some("Changing the payment form"));
    assert_eq!(
        desk.label,
        "checkout-form, a session at a hot desk, henrik's · Claude Code, Changing the payment form"
    );
    // Whose it is is that line, not the owner's small face on its picture (the user, 28 Nov
    // 2026: "the owner picture in the bottom right corner doesn't look good").
    assert!(
        matches!(&desk.face, Face::Agent { owner, owner_photo: None, .. } if owner.is_empty()),
        "{:?}",
        desk.face
    );
}

#[test]
fn a_line_said_at_a_coworkers_desk_is_headed_by_its_name_and_job_with_no_tag() {
    let mut scene = office();
    let seated = Message {
        desk: Some("john".into()),
        tool: Some(zirc_proto::CLAUDE_CODE.into()),
        ..Message::new(
            "cc-1",
            Event::Text {
                body: "on it".into(),
            },
        )
    };
    scene.hears(&[&zirc("henrik-mbp", &seated)]);
    let line = scene.lines("#api", utc(), false).pop().unwrap();
    let speaker = line.speaker.unwrap();
    // The user's choice, 27 Nov 2026: the job, muted, where a ROLE tag was.
    assert_eq!(
        (
            speaker.name.as_str(),
            speaker.detail.as_deref(),
            speaker.tag
        ),
        ("John", Some("Fixer"), None)
    );
    assert_eq!(speaker.label, "John, Fixer");
    // A session at a hot desk says whose it is by its tool, never "agent".
    let hot = Message {
        tool: Some(zirc_proto::CODEX.into()),
        ..Message::new(
            "cc-2",
            Event::Text {
                body: "looking".into(),
            },
        )
    };
    scene.hears(&[&zirc("henrik-mbp", &hot)]);
    let line = scene.lines("#api", utc(), false).pop().unwrap();
    assert_eq!(
        line.speaker.unwrap().detail.as_deref(),
        Some("henrik's Codex")
    );
}

/// `message` said by machine `nick` at `hh:mm` on the test's day, to `#api`, or to
/// `#decisions` for a request, which is longer than a line.
fn said_at(time: &str, nick: &str, message: &Message) -> String {
    let request = matches!(
        &message.event,
        Event::Custom { kind, .. } if kind == zirc_proto::request::KIND
    );
    let line = if request {
        format!(
            ":{} PRIVMSG {} :{}",
            source(nick),
            zirc_proto::request::CHANNEL,
            zirc_proto::encode_up_to(message, 4096).unwrap()
        )
    } else {
        zirc(nick, message)
    };
    format!("@time=2026-10-30T{time}:00.000Z;msgid=at-{time} {line}")
}

/// cc-9's push of `head` to its branch.
fn pushed(head: &str) -> Message {
    Message::new(
        "cc-9",
        Event::Push {
            branch: "tls".into(),
            head: head.into(),
            forced: false,
            subjects: Vec::new(),
            paths: Vec::new(),
            base: None,
            lines: std::collections::BTreeMap::new(),
            repo: None,
            on: None,
        },
    )
}

fn intent(summary: &str) -> Message {
    Message::new(
        "cc-9",
        Event::Intent {
            summary: summary.into(),
            paths: Vec::new(),
            expires: None,
            lasts: None,
        },
    )
}

/// Our cc-9 at a hot desk, rotating the TLS certificates since 11:30, which asked us at 11:35
/// whether to revoke last month's.
fn tls_asks() -> Scene {
    let mut scene = office();
    scene.hears(&[
        ":me!u@h JOIN #decisions",
        ":me-mbp!u@machine/me JOIN #decisions",
        &live("me-mbp", &["cc-9"], &[]),
    ]);
    let mut revoke = Request::open(
        "me-mbp/1",
        "cc-9",
        "me",
        "Revoke last month's TLS certificate now?",
        "It's revoked",
        "It stays valid",
    );
    revoke.irreversible = true;
    scene.hears(&[
        &said_at("11:30", "me-mbp", &intent("Rotating the TLS certificates")),
        &said_at("11:35", "me-mbp", &Message::new("cc-9", revoke.event())),
    ]);
    scene
}

#[test]
fn a_question_shows_on_its_askers_row_and_opens_there_not_atop_the_office() {
    let scene = tls_asks();
    let office = scene.office(utc());
    let desk = office
        .hot_desks
        .iter()
        .find(|d| d.conversation == "me-mbp/cc-9")
        .unwrap();
    assert_eq!(
        desk.doing.as_deref(),
        Some("Asked you · Revoke last month's TLS certificate now?")
    );
    let question = desk.question.as_ref().unwrap();
    assert_eq!(question.asked, "Asked you 25 min ago");
    assert_eq!((question.name.as_str(), question.mono), ("cc-9", true));
    assert_eq!(question.under, "Yours");
    assert_eq!(
        (
            question.opens.as_str(),
            question.open.as_str(),
            question.more.as_str()
        ),
        ("me-mbp/cc-9", "Open", "More answers")
    );
    assert_eq!(
        question
            .item
            .buttons
            .iter()
            .map(|b| b.label.as_str())
            .collect::<Vec<_>>(),
        ["Yes", "No"]
    );
    // It's waited on its answer since: nothing to say.
    assert_eq!(question.kept_working, None);
    // Other rows ask nothing.
    assert!(
        office
            .hot_desks
            .iter()
            .filter(|d| d.conversation != "me-mbp/cc-9")
            .all(|d| d.question.is_none())
    );
    assert!(office.coworkers.iter().all(|c| c.question.is_none()));
}

#[test]
fn an_asking_session_away_long_stays_on_its_row() {
    let mut scene = tls_asks();
    scene.hears(&[&live_away("me-mbp", &[], &[("cc-9", -60)])]);
    let office = scene.office(utc());
    assert!(
        office
            .hot_desks
            .iter()
            .any(|d| d.conversation == "me-mbp/cc-9")
    );
    assert!(office.hot_desks_folded.is_empty());
}

#[test]
fn an_opened_question_says_when_its_session_has_kept_working_since() {
    let mut scene = tls_asks();
    // Its intent said again, unchanged: still waiting on us.
    scene.hears(&[&said_at(
        "11:40",
        "me-mbp",
        &intent("Rotating the TLS certificates"),
    )]);
    let kept = |scene: &Scene| {
        scene
            .office(utc())
            .hot_desks
            .into_iter()
            .find(|d| d.conversation == "me-mbp/cc-9")
            .and_then(|d| d.question)
            .and_then(|q| q.kept_working)
    };
    assert_eq!(kept(&scene), None);
    // A new task: it's moved on, maybe answered in its own tool.
    scene.hears(&[&said_at(
        "11:45",
        "me-mbp",
        &intent("Rotate the staging certificates"),
    )]);
    assert_eq!(
        kept(&scene).as_deref(),
        Some(
            "It has kept working since asking: now on Rotate the staging certificates. If you \
             answered in the session, it'll take this back."
        )
    );
    // And two pushes, the second said in two channels.
    scene.hears(&[
        &said_at("11:50", "me-mbp", &pushed("a1")),
        &said_at("11:52", "me-mbp", &pushed("b2")),
        &said_at("11:52", "me-mbp", &pushed("b2")).replace("msgid=at-11:52", "msgid=again"),
    ]);
    assert_eq!(
        kept(&scene).as_deref(),
        Some(
            "It has kept working since asking: 2 pushes, now on Rotate the staging \
             certificates. If you answered in the session, it'll take this back."
        )
    );
}

#[test]
fn a_session_its_machine_never_named_gets_a_row_for_its_question() {
    let mut scene = office();
    scene.hears(&[
        ":me!u@h JOIN #decisions",
        ":me-mbp!u@machine/me JOIN #decisions",
    ]);
    let revoke = Request::open("me-mbp/1", "cc-4", "me", "Revoke it?", "y", "n");
    scene.hears(&[&said_at(
        "11:35",
        "me-mbp",
        &Message::new("cc-4", revoke.event()),
    )]);
    let office = scene.office(utc());
    let desk = office
        .hot_desks
        .iter()
        .find(|d| d.conversation == "me-mbp/cc-4")
        .expect("a row for the question");
    assert_eq!(desk.doing.as_deref(), Some("Asked you · Revoke it?"));
}
