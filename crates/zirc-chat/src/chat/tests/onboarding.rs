//! Getting started's prompt in an app ([`Chat::prompt`]): the next step's alone, where the
//! person is, in the words every app draws.

use super::*;
use crate::chat::{
    Prompt, PromptAction, PromptContext, PromptLater, PromptPlace, PutAway, wants_today,
};
use zirc_proto::onboarding::{Today, progress};
use zirc_proto::session::Live;

/// What `me-mbp` says of its live sessions, in #api.
fn live(sessions: &[&str]) -> String {
    let message = Message {
        level: Some(zirc_proto::Level::Debug),
        ..Message::new(
            zirc_proto::NO_SESSION,
            Live {
                sessions: sessions.iter().map(|&s| s.to_owned()).collect(),
                ..Default::default()
            }
            .event(),
        )
    };
    zirc("me-mbp", &message)
}

/// `#api` with our machine in it and `cc-tls` listening.
fn listening() -> Scene {
    let mut chat = Scene::in_api();
    feed(
        &mut chat,
        &[":me-mbp!u@machine/me JOIN #api", &live(&["cc-tls"])],
    );
    chat
}

/// The same once we said hi to it and it answered.
fn answered() -> Scene {
    let mut chat = listening();
    feed(
        &mut chat,
        &[
            "@time=2026-10-30T11:58:00.000Z;msgid=hi :me!u@h PRIVMSG me-mbp :cc-tls Hi! How's it going?",
            &dm("me-mbp", "cc-tls", "Hi. The new certificates load.", 1),
        ],
    );
    chat
}

const PHONE: PromptContext = PromptContext {
    phone: true,
    acted: None,
    today: None,
};

fn shown(chat: &Chat, reached: &[u8], context: PromptContext) -> Option<Prompt> {
    chat.prompt(&progress(reached), context)
}

#[test]
fn the_phones_step_says_hi_to_the_agent_listening_on_a_phone_alone() {
    let chat = listening();
    let prompt = shown(&chat, &[1, 2, 3], PHONE).expect("the phone's step is next");
    assert_eq!(prompt.step, "phone");
    assert_eq!(prompt.place, PromptPlace::Messages);
    assert_eq!(prompt.title.as_deref(), Some("cc-tls is listening."));
    let button = prompt.button.expect("its one action");
    assert_eq!(button.label, "Message cc-tls");
    assert_eq!(
        button.action,
        PromptAction::Open {
            name: "me-mbp/cc-tls".to_owned()
        }
    );
    // A message from a computer doesn't prove the phone's step: no card there.
    assert_eq!(shown(&chat, &[1, 2, 3], PromptContext::default()), None);
    // Nor before an agent listens, nor for a step prompted elsewhere.
    assert_eq!(shown(&Scene::in_api(), &[1, 2, 3], PHONE), None);
    assert_eq!(shown(&chat, &[1], PHONE), None);
}

#[test]
fn ci_is_asked_of_the_agent_that_answered_under_its_first_answer() {
    let chat = answered();
    let prompt = shown(&chat, &[1, 2, 3, 4], PHONE).expect("CI is next");
    assert_eq!(prompt.step, "ci");
    assert_eq!(
        prompt.place,
        PromptPlace::Conversation {
            name: "me-mbp/cc-tls".to_owned(),
            after: "me-mbp1".to_owned()
        }
    );
    assert_eq!(prompt.label.as_deref(), Some("Zirc · next step"));
    assert_eq!(prompt.title.as_deref(), Some("Your phone reaches cc-tls."));
    let button = prompt.button.expect("its one action");
    assert_eq!(button.label, "Ask cc-tls to do it");
    assert_eq!(
        button.action,
        PromptAction::Say {
            name: "me-mbp/cc-tls".to_owned(),
            text: zirc_proto::ci::CONNECT_ASK.to_owned()
        }
    );
    // Before any agent has answered, there's no conversation to put it in.
    assert_eq!(shown(&listening(), &[1, 2, 3, 4], PHONE), None);
}

#[test]
fn a_prompt_acted_on_says_so_in_its_buttons_place() {
    let chat = answered();
    let at =
        chrono::NaiveDate::from_ymd_opt(2026, 10, 30).and_then(|day| day.and_hms_opt(9, 10, 0));
    let acted = PromptContext { acted: at, ..PHONE };
    let ci = shown(&chat, &[1, 2, 3, 4], acted).expect("CI is next");
    assert_eq!(ci.button, None);
    assert_eq!(
        ci.done.as_deref(),
        Some("✓ You asked cc-tls to do it · 09:10")
    );
    // The first wake's, once CI is heard: a test failure, then that it was sent.
    let wake = shown(&chat, &[1, 2, 3, 4, 5], PHONE).expect("the first wake is next");
    assert_eq!(wake.step, "first_wake");
    assert_eq!(
        wake.button.map(|b| b.action),
        Some(PromptAction::Say {
            name: "me-mbp/cc-tls".to_owned(),
            text: zirc_proto::ci::test_failure_ask()
        })
    );
    let sent = shown(&chat, &[1, 2, 3, 4, 5], acted).expect("the first wake is next");
    assert_eq!(
        sent.done.as_deref(),
        Some("✓ Sent · 09:10. CI is running it.")
    );
    // Every step of getting started done: nothing to prompt.
    assert_eq!(shown(&chat, &[1, 2, 3, 4, 5, 6, 7, 8], PHONE), None);
}

/// The lines of the conversation with `cc-tls`, as an app draws them.
fn tls_lines(chat: &Chat) -> Vec<crate::chat::lines::Line> {
    chat.lines(
        "me-mbp/cc-tls",
        chrono::FixedOffset::east_opt(0).unwrap(),
        false,
    )
}

#[test]
fn the_next_steps_card_is_in_the_agents_conversation_under_its_first_answer() {
    let mut chat = answered();
    chat.set_prompting(Some(progress(&[1, 2, 3, 4])), PHONE);
    let shown = tls_lines(&chat);
    let at = shown
        .iter()
        .position(|line| line.key == "me-mbp1")
        .expect("the answer");
    let card = &shown[at + 1];
    assert_eq!(card.key, "me-mbp1-next");
    assert_eq!(card.shape, crate::chat::RowShape::Prompt);
    let callout = card.callout.as_ref().expect("a callout");
    assert_eq!(callout.label, "Zirc · next step");
    assert_eq!(callout.tone, "ok");
    assert_eq!(callout.button.as_deref(), Some("Ask cc-tls to do it"));
    assert_eq!(callout.action.as_deref(), Some("prompt:ci"));
    assert!(
        card.text.contains("Your phone reaches cc-tls."),
        "{}",
        card.text
    );
}

#[test]
fn the_next_steps_card_is_nowhere_else_and_goes_once_its_step_is_done() {
    let mut chat = answered();
    let card = |lines: Vec<crate::chat::lines::Line>| {
        lines.into_iter().any(|line| line.key.ends_with("-next"))
    };
    // Before an app says the steps, no card.
    assert!(!card(tls_lines(&chat)));
    chat.set_prompting(Some(progress(&[1, 2, 3, 4])), PHONE);
    assert!(card(tls_lines(&chat)));
    // Not in another conversation.
    assert!(!card(chat.lines(
        "#api",
        chrono::FixedOffset::east_opt(0).unwrap(),
        false
    )));
    chat.set_prompting(Some(progress(&[1, 2, 3, 4, 5, 6, 7, 8])), PHONE);
    assert!(!card(tls_lines(&chat)));
}

/// On a phone or a computer, with their agents' day as zirc.dev counts it.
fn today(wakes: u32, fixed: u32) -> PromptContext {
    PromptContext {
        today: Some(Today { wakes, fixed }),
        ..PromptContext::default()
    }
}

#[test]
fn once_their_agents_woke_today_a_teammate_is_invited_atop_messages() {
    let chat = Scene::in_api();
    let done = [1, 2, 3, 4, 5, 6];
    assert!(wants_today(&progress(&done)));
    let prompt = shown(&chat, &done, today(4, 2)).expect("the first overlap waits");
    assert_eq!(prompt.step, "teammate");
    assert_eq!(prompt.place, PromptPlace::Messages);
    assert_eq!(
        prompt.title.as_deref(),
        Some("Your agents woke 4 times today and fixed 2 builds.")
    );
    assert_eq!(
        prompt.button.map(|b| b.action),
        Some(PromptAction::Page {
            path: zirc_proto::onboarding::INVITE_PATH.to_owned()
        })
    );
    let later = prompt.later.expect("Not now");
    assert_eq!(
        (later.label.as_str(), later.back_after_days),
        ("Not now", Some(7))
    );
}

#[test]
fn the_invite_card_counts_a_lone_wake_and_waits_for_a_busy_day() {
    let chat = Scene::in_api();
    let done = [1, 2, 3, 4, 5, 6];
    let once = shown(&chat, &[1, 2, 3, 4, 5, 6, 7], today(1, 0)).expect("a teammate is next");
    assert_eq!(once.title.as_deref(), Some("Your agents woke once today."));
    assert_eq!(
        once.body,
        zirc_proto::onboarding::STEPS[7].about,
        "the step's own words"
    );
    // Not on a quiet day, nor before zirc.dev says, nor once a teammate joined.
    assert_eq!(shown(&chat, &done, today(0, 0)), None);
    assert_eq!(shown(&chat, &done, PromptContext::default()), None);
    assert_eq!(shown(&chat, &[1, 2, 3, 4, 5, 6, 8], today(4, 2)), None);
    assert!(!wants_today(&progress(&[1, 2, 3, 4, 5])));
}

#[test]
fn not_now_puts_a_prompt_away_for_a_week_once_and_dismiss_for_good() {
    let day = 24 * 60 * 60;
    let not_now = PromptLater {
        label: "Not now".to_owned(),
        back_after_days: Some(7),
    };
    let once = PutAway::default().again(&not_now, 0);
    assert!(!PutAway::default().hides(0));
    assert!(once.hides(6 * day));
    assert!(!once.hides(7 * day), "back a week later");
    let twice = once.again(&not_now, 7 * day);
    assert!(twice.hides(1000 * day), "then gone for good");
    let dismiss = PromptLater {
        back_after_days: None,
        ..not_now
    };
    assert!(PutAway::default().again(&dismiss, 0).hides(1000 * day));
    // The phone's card is dismissed for good.
    let hi = shown(&listening(), &[1, 2, 3], PHONE).expect("the phone's step");
    assert_eq!(hi.later.map(|l| l.back_after_days), Some(None));
}
