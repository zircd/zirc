//! What was said while a machine's zircd wasn't there to hear it, told once it's back.

use zirc_core::ipc::Why;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

/// Waits for a post saying `text` among those `watching` hears, with why it woke the
/// session.
async fn post_saying(watching: &mut zirc_core::ipc::Watching, text: &str) -> Option<Why> {
    loop {
        let (body, why) = next_post_why(watching).await;
        if body.contains(text) {
            return why;
        }
    }
}

#[tokio::test]
async fn a_mention_said_while_zircd_was_stopped_wakes_the_session_once_it_is_back() {
    let ergo = require_ergo!();
    let alpha = Peer::join(Machine::start(&ergo, "alpha").await, Some("cc-a")).await;
    let beta = Peer::join(Machine::start(&ergo, "beta").await, Some("cc-b")).await;
    // A plain IRC client in #api, which tells when something's been said there.
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#api").await;
    {
        let mut watching = alpha.watch().await;
        beta.says("cc-a: heard live").await;
        assert_eq!(
            post_saying(&mut watching, "heard live").await,
            Some(Why::Mention)
        );
    }

    let Peer {
        machine,
        clone,
        session,
    } = alpha;
    // Said while alpha's zircd is stopped, as for an update: on the server before it's
    // back.
    let machine = machine
        .restart_after(async {
            beta.says("cc-a: can you rebase onto mine?").await;
            tokio::time::timeout(
                WAIT,
                zirc_test_irc::next_matching(&mut ada_in, |line| {
                    line.command == "PRIVMSG"
                        && line
                            .params
                            .get(1)
                            .is_some_and(|body| body.contains("rebase"))
                }),
            )
            .await
            .expect("beta's post on the server");
        })
        .await;
    let alpha = Peer {
        machine,
        clone,
        session,
    };

    let mut watching = alpha.watch().await;
    assert_eq!(
        post_saying(&mut watching, "can you rebase onto mine?").await,
        Some(Why::Mention),
        "what was said meanwhile wakes the session it names"
    );
}

#[tokio::test]
async fn after_a_long_gap_a_waiting_session_is_told_zircd_was_away_rather_than_replayed_to() {
    let ergo = require_ergo!();
    let alpha = Peer::join(Machine::start(&ergo, "alpha").await, Some("cc-a")).await;
    alpha
        .machine
        .request(zirc_core::ipc::Request::WaitFor {
            session: "cc-a".into(),
            on: vec!["1299".into()],
            states: Vec::new(),
            reactions: Vec::new(),
            drop: false,
        })
        .await;
    let home = alpha.machine.home.path().to_owned();
    let Peer {
        machine,
        clone,
        session,
    } = alpha;
    // Stopped for an hour, as a laptop asleep: it was last connected then.
    let machine = machine
        .restart_after(async {
            let hour_ago = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - 3600;
            std::fs::write(
                zirc_core::paths::replay(&home),
                format!(r#"{{"alive":{hour_ago},"newest":{{}}}}"#),
            )
            .unwrap();
        })
        .await;
    let alpha = Peer {
        machine,
        clone,
        session,
    };

    // Told as it connects, it's kept for cc-a's next watch: after "while no watch of yours
    // ran", or live if the watch was first.
    let mut watching = alpha.watch().await;
    let (body, why) = loop {
        let (body, why) = next_post_why(&mut watching).await;
        if body.contains("zircd was away") {
            break (body, why);
        }
    };
    assert_eq!(why, Some(Why::Mention));
    assert!(
        body.starts_with("cc-a: zircd was away from the server for 1h, so 1299 may have"),
        "{body}"
    );
    assert!(body.contains("`zirc catchup --since 1h`"), "{body}");
}
