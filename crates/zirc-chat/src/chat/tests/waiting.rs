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
