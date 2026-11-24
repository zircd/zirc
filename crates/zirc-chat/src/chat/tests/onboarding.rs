//! Getting started's prompt in an app ([`Chat::prompt`]): the next step's alone, where the
//! person is, in the words every app draws.

use super::*;
use crate::chat::{Prompt, PromptAction, PromptContext, PromptPlace};
use zirc_proto::onboarding::progress;
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
    let acted = PromptContext {
        phone: true,
        acted: at,
    };
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
