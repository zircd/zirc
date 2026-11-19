//! The wake breaker through real wakes: a session woken six times in ten minutes, its
//! machine's `[wakes]` limit here, is woken no more, what would have woken it waits as notes, its owner hears it once by direct message, and wakes
//! resume when the window drains or when asked. The machine's clock is moved, not waited.

use std::time::Duration;

use zirc_core::ipc::{Delivery, Request, Response, Watching};
use zirc_proto::Event;

mod common;
use common::*;

/// What a session's machine (`alpha`, on a clock the test moves) and another (`beta`)
/// need: beta's posts name `cc-1`, whose watch is on alpha, and Ada, alpha's owner, reads her direct messages.
struct Storm {
    alpha: Machine,
    beta: Peer,
    clock: zircd::Clock,
    cc1: Watching,
    dms: zirc_irc::Incoming,
    _ada: zirc_irc::Client,
    _alpha_clone: tempfile::TempDir,
    _ergo: zirc_test_ergo::Ergo,
}

impl Storm {
    async fn start() -> Option<Self> {
        let ergo = zirc_test_ergo::Ergo::start()?;
        let clock = zircd::Clock::default();
        let hooks = zircd::Hooks {
            clock: clock.clone(),
            ..zircd::Hooks::default()
        };
        // Six, not the default, keeps each test's storm short, and tests the setting too.
        let settings = "owner = \"ada\"\n[wakes]\nper_session = 6\n";
        let alpha = Machine::hooked_with(&ergo, "alpha", settings, hooks).await;
        let alpha_clone = clone();
        alpha.add_repo(None, alpha_clone.path()).await.unwrap();
        alpha
            .eventually(|status| status.channels == JOINED, "alpha joining")
            .await;
        let beta = Peer::join(Machine::start(&ergo, "beta").await, Some("cc-2")).await;
        let (ada, dms) = zirc_test_irc::person(&ergo, "ada").await;
        let cc1 = alpha.watch(Some("cc-1")).await;
        Some(Self {
            alpha,
            beta,
            clock,
            cc1,
            dms,
            _ada: ada,
            _alpha_clone: alpha_clone,
            _ergo: ergo,
        })
    }

    /// Beta asks cc-1 `text`, and the delivery cc-1's watch hears of it.
    async fn asks(&mut self, text: &str) -> Delivery {
        self.beta.says(&format!("cc-1: {text}")).await;
        loop {
            let delivery = next_post(&mut self.cc1).await;
            if matches!(&delivery.message.event, Event::Text { body } if body.contains(text)) {
                return delivery;
            }
        }
    }

    async fn breaker(&self) -> zirc_core::ipc::BreakerState {
        self.alpha.status().await.unwrap().breaker.unwrap()
    }
}

/// The first line of the breaker's notice to Ada: a NOTICE, which no phone pushes.
async fn notice_to_ada(dms: &mut zirc_irc::Incoming) -> String {
    let mut batches = zirc_irc::multiline::Batches::default();
    let line = zirc_test_irc::next_whole_matching(dms, &mut batches, |line| {
        line.command == "NOTICE"
            && line.params[0] == "ada"
            && line
                .params
                .get(1)
                .is_some_and(|body| body.contains("wakes paused:"))
    })
    .await;
    line.params[1].clone()
}

#[tokio::test]
async fn a_session_woken_six_times_in_ten_minutes_waits_until_the_window_drains() {
    let Some(mut storm) = Storm::start().await else {
        return;
    };
    for n in 1..=6 {
        let woke = storm.asks(&format!("can you look at {n}?")).await;
        assert!(woke.why.is_some(), "wake {n} goes through");
    }
    assert_eq!(storm.breaker().await.wakes, 6);

    // The seventh is heard, but wakes nobody, and Ada is told once.
    let held = storm.asks("and at 7?").await;
    assert_eq!(held.why, None);
    let notice = notice_to_ada(&mut storm.dms).await;
    assert!(notice.contains("6 wakes of cc-1 in 10 min"), "{notice}");
    let held = storm.asks("and at 8?").await;
    assert_eq!(held.why, None);
    let state = storm.breaker().await;
    assert_eq!(state.paused_sessions, ["cc-1"]);
    assert_eq!(state.held, 2);
    // Another session isn't paused for cc-1's sake.
    let mut cc3 = storm.alpha.watch(Some("cc-3")).await;
    storm.beta.says("cc-3: can you look at this?").await;
    assert!(next_post(&mut cc3).await.why.is_some());

    // Ten minutes on, the window has drained, and the next wakes cc-1, the notes held
    // meanwhile first.
    storm.clock.advance(Duration::from_mins(10));
    assert!(storm.breaker().await.paused_sessions.is_empty());
    storm.beta.says("cc-1: anyone there?").await;
    let mut heard = Vec::new();
    loop {
        let delivery = next_post(&mut storm.cc1).await;
        let Event::Text { body } = &delivery.message.event else {
            unreachable!()
        };
        // cc-3's line above is heard too, concerning cc-3 alone.
        if body.starts_with("cc-1: ") {
            heard.push((body.clone(), delivery.why.is_some()));
        }
        if body.contains("anyone there?") {
            break;
        }
    }
    let heard: Vec<(&str, bool)> = heard.iter().map(|(b, w)| (b.as_str(), *w)).collect();
    assert_eq!(
        heard,
        [
            ("cc-1: meanwhile, for you and needing nothing yet:", true),
            ("cc-1: and at 7?", true),
            ("cc-1: and at 8?", true),
            ("cc-1: anyone there?", true),
        ]
    );
}

#[tokio::test]
async fn a_paused_session_wakes_again_at_once_when_asked_to() {
    let Some(mut storm) = Storm::start().await else {
        return;
    };
    for n in 1..=6 {
        storm.asks(&format!("can you look at {n}?")).await;
    }
    assert_eq!(storm.asks("and at 7?").await.why, None);
    notice_to_ada(&mut storm.dms).await;

    let Response::Wakes {
        breaker: Some(breaker),
        ..
    } = storm.alpha.request(Request::WakesResume).await
    else {
        panic!("not the breaker's state")
    };
    assert!(breaker.paused_sessions.is_empty());
    assert_eq!(breaker.wakes, 0, "counted afresh");
    assert!(storm.asks("now?").await.why.is_some());
}
