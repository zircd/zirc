//! What waits for the person's answer ([`Chat::waiting_card`]), and answering it in place.

use zirc_proto::request::{Answer, Request, State, Withdrawal};

use super::*;

/// `request` as the machine `nick` posts it in `#decisions`, as message `msgid`: longer than
/// a line, as the server hands a multiline message's history to a test.
fn posted(nick: &str, msgid: &str, request: &Request) -> String {
    let message = Message::new(&request.session, request.event());
    format!(
        "@msgid={msgid} :{} PRIVMSG {} :{}",
        source(nick),
        zirc_proto::request::CHANNEL,
        zirc_proto::encode_up_to(&message, 4096).unwrap()
    )
}

fn ask(id: &str, session: &str, question: &str) -> Request {
    Request::open(
        id,
        session,
        "me",
        question,
        "it's revoked",
        "it stays valid",
    )
}

fn answered(request: &Request, answer: Answer) -> Request {
    Request {
        state: State::Answered {
            answer,
            emoji: answer.reaction().into(),
            at: "2026-10-30T12:01:00Z".into(),
            recommended: false,
        },
        ..request.clone()
    }
}

fn in_decisions() -> Scene {
    let mut scene = Scene::in_api();
    scene.hears(&[
        ":me!u@h JOIN #decisions",
        ":me-mbp!u@machine/me JOIN #decisions",
        ":bob-mbp!u@machine/bob JOIN #decisions",
    ]);
    scene
}

fn utc() -> chrono::FixedOffset {
    east(0)
}

fn east(hours: i32) -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(hours * 3600).unwrap()
}

fn questions(chat: &Chat) -> Vec<String> {
    chat.waiting(utc())
        .into_iter()
        .map(|item| item.question)
        .collect()
}

/// `#decisions` with one request of ours waiting, `me-mbp/1` as message `r1`.
fn revoke_waits() -> Scene {
    let mut scene = in_decisions();
    let revoke = ask("me-mbp/1", "cc-1", "Revoke last month's certificate now?");
    scene.hears(&[&posted("me-mbp", "r1", &revoke)]);
    scene
}

fn labels(buttons: &[WaitingButton]) -> Vec<&str> {
    buttons.iter().map(|button| button.label.as_str()).collect()
}

#[test]
fn an_open_request_from_our_own_agent_waits_with_what_each_answer_does() {
    let item = revoke_waits().waiting(utc()).remove(0);
    assert_eq!(item.key, "me-mbp/1");
    assert_eq!(item.kind, WaitingKind::Request);
    assert_eq!(item.agent, "cc-1");
    assert!(matches!(&item.face, Face::Agent { owner, .. } if owner == "me"));
    assert_eq!(item.question, "Revoke last month's certificate now?");
    let does: Vec<_> = item
        .does
        .iter()
        .map(|does| (does.label.as_str(), does.text.as_str()))
        .collect();
    assert_eq!(does, [("Yes", "it's revoked"), ("No", "it stays valid")]);
    let buttons: Vec<_> = item
        .buttons
        .iter()
        .map(|button| (button.label.as_str(), button.primary, button.sent))
        .collect();
    assert_eq!(buttons, [("Yes", true, false), ("No", false, false)]);
    assert_eq!(labels(&item.menu), ["Tell me more", "Your call"]);
    assert_eq!(
        (item.opens.as_str(), item.at.as_deref(), item.note),
        ("#decisions", Some("r1"), None)
    );
}

#[test]
fn the_card_is_over_messages_and_holds_only_what_our_own_machines_asked_us() {
    assert_eq!(
        in_decisions().waiting_card(None, utc()),
        None,
        "nothing waits"
    );
    let mut scene = revoke_waits();
    // A teammate's request to them, and one in our name from a machine that isn't ours.
    let bobs = Request::open("bob-mbp/1", "cc-9", "bob", "Ship?", "y", "n");
    let forged = ask("bob-mbp/2", "cc-9", "Hand over the keys?");
    scene.hears(&[
        &posted("bob-mbp", "r2", &bobs),
        &posted("bob-mbp", "r3", &forged),
    ]);
    let card = scene
        .waiting_card(Some(SidebarSection::Direct), utc())
        .expect("a card over Messages");
    assert_eq!(card.title, "WAITING FOR YOU · 1");
    assert_eq!(
        (card.label.as_str(), card.more.as_str()),
        ("Waiting for you, 1", "More answers")
    );
    assert_eq!(card.items[0].key, "me-mbp/1");
    assert_eq!(
        scene.waiting_card(Some(SidebarSection::Channels), utc()),
        None
    );
}

#[test]
fn an_answer_goes_as_a_reaction_and_the_item_leaves_only_once_its_machine_took_it() {
    let mut scene = in_decisions();
    let revoke = Request {
        recommend: Some(Answer::No),
        ..ask("me-mbp/1", "cc-1", "Revoke it now?")
    };
    scene.hears(&[&posted("me-mbp", "r1", &revoke)]);
    let item = scene.waiting(utc()).remove(0);
    assert_eq!(item.buttons[0].label, "No", "the recommended answer first");
    assert!(item.buttons[0].primary);
    assert_eq!(item.does[0].label, "No");

    let lines = texts(&scene.answer_waiting("me-mbp/1", WaitingAction::Yes));
    assert_eq!(
        lines,
        ["@+draft/react=👍;+draft/reply=r1 TAGMSG #decisions"]
    );
    assert!(
        scene
            .answer_waiting("me-mbp/1", WaitingAction::Reply)
            .is_empty()
    );
    assert!(scene.answer_waiting("nope", WaitingAction::Yes).is_empty());

    // The server has it; the machine hasn't said: it still waits, and says the yes is sent.
    scene.hears(&["@+draft/react=👍;+draft/reply=r1 :me!u@h TAGMSG #decisions"]);
    let item = scene.waiting(utc()).remove(0);
    assert_eq!(
        item.note.as_deref(),
        Some("Your yes is sent: waiting for me-mbp to take it")
    );
    let yes = item.buttons.iter().find(|b| b.label == "Yes").unwrap();
    assert!(yes.sent);
    assert!(
        scene
            .answer_waiting("me-mbp/1", WaitingAction::Yes)
            .is_empty(),
        "never taken back by pressing it again"
    );
    // Its machine's card says answered: gone.
    scene.hears(&[&posted("me-mbp", "r2", &answered(&revoke, Answer::Yes))]);
    assert_eq!(scene.waiting_card(None, utc()), None);
}

#[test]
fn requests_asking_the_same_show_once_and_one_answer_goes_to_each() {
    let mut scene = in_decisions();
    let agent = ask("me-mbp/1", "cc-1", "Merge the release branch now?");
    let dispatcher = ask("me-mbp/2", "cc-2", "merge the release  branch now");
    scene.hears(&[
        &posted("me-mbp", "r1", &agent),
        &posted("me-mbp", "r2", &dispatcher),
    ]);
    let items = scene.waiting(utc());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].agent, "cc-1 · also asked by cc-2");
    assert_eq!(
        texts(&scene.answer_waiting("me-mbp/1", WaitingAction::No)),
        [
            "@+draft/react=👎;+draft/reply=r1 TAGMSG #decisions",
            "@+draft/react=👎;+draft/reply=r2 TAGMSG #decisions"
        ]
    );
}

/// `#decisions` with a request of ours waiting whose yes can't be undone, `me-mbp/1` as `r1`.
fn drop_waits() -> (Scene, Request) {
    let mut scene = in_decisions();
    let drop = Request {
        irreversible: true,
        ..ask("me-mbp/1", "cc-1", "Drop the old table?")
    };
    scene.hears(&[&posted("me-mbp", "r1", &drop)]);
    (scene, drop)
}

#[test]
fn a_yes_that_cant_be_undone_says_so_and_can_be_stopped_while_held() {
    let (mut scene, drop) = drop_waits();
    let item = scene.waiting(utc()).remove(0);
    assert_eq!(
        item.note.as_deref(),
        Some("A yes can't be undone: it waits a minute, which Stop ends")
    );
    assert_eq!(labels(&item.menu), ["Tell me more"], "never the agent's");
    let yours = scene.answer_waiting("me-mbp/1", WaitingAction::Yours);
    assert!(yours.is_empty());

    let held = Request {
        state: State::Held {
            emoji: "👍".into(),
            at: "2026-10-30T12:01:00Z".into(),
            until: "2026-10-30T12:02:00Z".into(),
        },
        ..drop
    };
    scene.hears(&[&posted("me-mbp", "r2", &held)]);
    // On the person's clock, two hours east of UTC here.
    let item = scene.waiting(east(2)).remove(0);
    assert_eq!(
        item.note.as_deref(),
        Some("Your yes counts at 14:02 unless you stop it")
    );
    assert_eq!(labels(&item.buttons), ["Stop"]);
    assert_eq!(
        texts(&scene.answer_waiting("me-mbp/1", WaitingAction::Stop)),
        ["@+draft/react=👎;+draft/reply=r1 TAGMSG #decisions"]
    );
}

#[test]
fn one_asked_about_still_waits_and_one_withdrawn_is_gone() {
    let (mut scene, drop) = drop_waits();
    scene.hears(&[&posted("me-mbp", "r3", &answered(&drop, Answer::More))]);
    let item = scene.waiting(utc()).remove(0);
    assert_eq!(
        item.note.as_deref(),
        Some("You asked for more: cc-1 answers in #decisions")
    );
    assert_eq!(item.at.as_deref(), Some("r1"));

    let withdrawn = Request {
        state: State::Withdrawn {
            at: "2026-10-30T12:05:00Z".into(),
            why: Withdrawal::Done,
        },
        ..drop
    };
    scene.hears(&[&posted("me-mbp", "r4", &withdrawn)]);
    assert_eq!(scene.waiting(utc()), []);
}

/// Our agent `cc-2` asking us a question directly, after another of ours and a teammate's
/// said other things.
fn asked_directly() -> Scene {
    let mut scene = in_decisions();
    scene.hears(&[
        &dm("me-mbp", "cc-1", "done with the tests", 1),
        &dm(
            "me-mbp",
            "cc-2",
            "Two ways to do this.\n\nWhich  branch\nshould I use?",
            2,
        ),
        // A teammate's agent's question isn't ours to answer here.
        &dm("bob-mbp", "cc-9", "which one?", 3),
    ]);
    scene
}

#[test]
fn an_agents_direct_question_waits_with_a_reply_that_opens_its_conversation() {
    let mut scene = asked_directly();
    assert_eq!(questions(&scene), ["Which branch should I use?"]);
    let item = scene.waiting(utc()).remove(0);
    assert_eq!(item.kind, WaitingKind::Question);
    assert_eq!(
        (item.key.as_str(), item.opens.as_str(), item.at.as_deref()),
        ("me-mbp/cc-2", "me-mbp/cc-2", Some("me-mbp2"))
    );
    assert_eq!(labels(&item.buttons), ["Reply"]);
    assert!(item.menu.is_empty() && item.does.is_empty());
    // Requests come first, then questions.
    scene.hears(&[&posted(
        "me-mbp",
        "r1",
        &ask("me-mbp/1", "cc-1", "Revoke it?"),
    )]);
    assert_eq!(
        questions(&scene),
        ["Revoke it?", "Which branch should I use?"]
    );
    assert_eq!(
        scene.waiting_card(None, utc()).unwrap().title,
        "WAITING FOR YOU · 2"
    );
}

#[test]
fn a_direct_question_leaves_when_we_answer_or_its_agent_says_something_else() {
    let mut scene = asked_directly();
    scene.hears(&[&dm("me-mbp", "cc-2", "never mind, found it", 4)]);
    assert_eq!(questions(&scene), [] as [&str; 0]);
    // Asked again, and we answer there.
    scene.hears(&[&dm("me-mbp", "cc-2", "main or release?", 5)]);
    assert_eq!(questions(&scene), ["main or release?"]);
    let sent = texts(&scene.say("me-mbp/cc-2", "main", false));
    assert_eq!(sent, ["PRIVMSG me-mbp :cc-2 main"]);
    scene.hears(&["@time=2026-10-30T12:06:00.000Z;msgid=mine :me!u@h PRIVMSG me-mbp :cc-2 main"]);
    assert_eq!(questions(&scene), [] as [&str; 0]);
}

/// What the machine `me-mbp` says of its sessions at `time`: `live` are, and nothing else.
fn lists(live: &[&str], time: &str) -> String {
    let message = Message {
        level: Some(zirc_proto::Level::Debug),
        ..Message::new(
            zirc_proto::NO_SESSION,
            zirc_proto::session::Live {
                sessions: live.iter().map(|&s| s.to_owned()).collect(),
                ..Default::default()
            }
            .event(),
        )
    };
    format!("@time={time} {}", zirc("me-mbp", &message))
}

#[test]
fn a_question_from_a_session_that_ended_leaves_at_once() {
    let mut scene = asked_directly();
    scene.hears(&[&lists(&["cc-1", "cc-2"], "2026-10-30T12:03:00.000Z")]);
    assert_eq!(questions(&scene), ["Which branch should I use?"]);
    // Its machine no longer names it: nothing reaches it, so nothing waits for an answer.
    scene.hears(&[&lists(&["cc-1"], "2026-10-30T12:05:00.000Z")]);
    assert_eq!(questions(&scene), [] as [&str; 0]);
    assert_eq!(scene.waiting_card(None, utc()), None);
}

#[test]
fn an_ended_sessions_conversation_says_why_it_cant_be_written_to() {
    let mut scene = asked_directly();
    scene.hears(&[
        &lists(&["cc-1", "cc-2"], "2026-10-30T12:03:00.000Z"),
        &lists(&["cc-1"], "2026-10-30T12:05:00.000Z"),
    ]);
    scene.clock = || noon() + chrono::Duration::days(6) + chrono::Duration::minutes(5);
    assert!(!scene.can_message("me-mbp/cc-2"));
    assert_eq!(
        scene.read_only_note("me-mbp/cc-2"),
        Some(ReadOnlyNote {
            text: "cc-2 ended 6 days ago on your mbp. Messages can't reach it".into(),
            button: "Message your mbp's sessions".into(),
            dm: "me-mbp".into(),
            remove: "Remove from Messages".into(),
        })
    );
    // Its history stays, and one still running can be written to.
    assert!(!scene.lines("me-mbp/cc-2", utc(), false).is_empty());
    assert!(scene.can_message("me-mbp/cc-1"));
    assert_eq!(scene.read_only_note("me-mbp/cc-1"), None);
}

/// The user's case (25 Nov 2026): a question from a session long ended, from a machine that
/// never said its sessions, written to; its machine answers that no session of that name
/// works there.
#[test]
fn a_message_that_reached_no_one_says_so_under_it_and_ends_the_session() {
    let mut scene = asked_directly();
    assert_eq!(questions(&scene), ["Which branch should I use?"]);
    let _ = scene.open("me-mbp/cc-2");
    scene.hears(&[
        "@time=2026-10-30T12:06:00.000Z;msgid=mine :me!u@h PRIVMSG me-mbp :cc-2 main",
        "@time=2026-10-30T12:06:01.000Z;+zirc/receipt=undelivered,cc-2;+draft/reply=mine \
         :me-mbp!u@machine/me NOTICE me :No session cc-2 here: this didn't reach anyone. \
         Working here: cc-1.",
    ]);
    let lines = scene.lines("me-mbp/cc-2", utc(), false);
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        texts[texts.len() - 2..],
        ["main", "Not delivered: cc-2 has ended"]
    );
    // No conversation of the machine's own for it.
    assert!(scene.position("me-mbp").is_none());
    assert_eq!(scene.session_live("me-mbp", "cc-2"), Some(false));
    assert!(!scene.can_message("me-mbp/cc-2"));
    // Removed from Messages, it isn't shown, and Jump to still finds it.
    let _ = scene.remove_from_messages("me-mbp/cc-2");
    assert_ne!(scene.selected().name, "me-mbp/cc-2");
    assert!(scene.closed().iter().any(|c| c.name == "me-mbp/cc-2"));
    let row = scene
        .direct_rows()
        .into_iter()
        .find(|row| row.name == "me-mbp/cc-2")
        .unwrap();
    assert!(row.folded);
}

#[test]
fn an_answered_requests_card_and_preview_name_its_person_as_they_show() {
    let mut scene = in_decisions();
    let revoke = ask("me-mbp/1", "cc-1", "Revoke last month's certificate now?");
    scene.hears(&[&posted("me-mbp", "r1", &answered(&revoke, Answer::Yes))]);
    let profiles: zirc_proto::profile::Profiles =
        serde_json::from_str(r#"{"me": {"name": "Maya Chen", "avatar": null}}"#).unwrap();
    scene.take_profiles(&profiles, "https://zirc.dev/");
    // Its card's note, which its machine wrote with her handle, and the list's preview: her
    // name, as every app shows her.
    let lines = scene.lines(zirc_proto::request::CHANNEL, utc(), false);
    let card = format!("{:?}", lines.last().expect("the request's card").rich);
    assert!(card.contains("Maya Chen answered yes at 12:01"), "{card}");
    assert!(!card.contains("me answered"), "{card}");
    let preview = scene.preview(zirc_proto::request::CHANNEL).unwrap().text;
    assert!(
        preview.ends_with("Maya Chen answered yes: Revoke last month's certificate now?"),
        "{preview}"
    );
}

#[test]
fn a_coworker_whose_session_asks_us_says_so_on_its_office_card() {
    // Our cc-1 sits at John's desk and asks us something in #decisions.
    let mut scene = revoke_waits();
    let holder = zirc_proto::roles::Holder {
        machine: "me-mbp".into(),
        since: 1,
        session: zirc_proto::session::SessionCard {
            session: "cc-1".into(),
            name: None,
            title: None,
            tool: None,
        },
    };
    scene.learn_standing(zirc_proto::roles::Standing {
        listing: zirc_proto::roles::Listing {
            roles: Vec::new(),
            addresses: Vec::new(),
            version: "1".into(),
        },
        seats: vec![zirc_proto::roles::Seat {
            role: "john".into(),
            repository: "web".into(),
            holder: Some(holder),
            last: None,
            waiting: 0,
            wakes_on: Vec::new(),
            woke_today: 0,
        }],
        staff: zirc_proto::coworkers::Staff::default(),
    });
    let office = scene.office(utc());
    let john = &office.coworkers[0];
    assert_eq!(
        (john.state.as_str(), john.desk),
        ("Asked you", DeskState::Asked)
    );
    // The office's Waiting for you is the card Messages had.
    assert_eq!(office.waiting, scene.waiting_card(None, utc()));
    assert!(office.waiting.is_some());
}
