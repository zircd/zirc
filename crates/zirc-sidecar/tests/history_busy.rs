//! Ergo's history while something else holds its database, as the sidecar's scrub does
//! (`zirc_sidecar::trim`, VACUUM): a message said meanwhile is kept once it's let go, rather
//! than lost, as each said during a scrub was until 16 Nov 2026 (Ergo's `SQLITE_BUSY`).

use std::time::Duration;

use zirc_test_ergo::Ergo;
use zirc_test_irc::{joins, person};

/// Longer than Ergo's own wait for its database, 5s, which a scrub of zirc's history outlasted.
const HELD: Duration = Duration::from_secs(7);

#[tokio::test]
async fn a_message_said_while_the_history_is_held_is_kept_once_its_let_go() {
    let Some(ergo) = Ergo::start_with(zirc_test_ergo::HISTORY) else {
        eprintln!("skipped: set ZIRC_ERGO and ZIRC_ERGO_CONFIG, as Bazel does");
        return;
    };
    let (alice, mut heard) = person(&ergo, "alice").await;
    joins(&alice, &mut heard, "#api").await;
    // Held as VACUUM holds it: nothing else writes until it's done.
    let held = rusqlite::Connection::open(ergo.history()).unwrap();
    held.execute_batch("BEGIN EXCLUSIVE").unwrap();
    alice.privmsg("#api", "said while it's held").unwrap();
    tokio::time::sleep(HELD).await;
    held.execute_batch("COMMIT").unwrap();

    let reader = rusqlite::Connection::open(ergo.history()).unwrap();
    reader.busy_timeout(Duration::from_secs(5)).unwrap();
    let kept = || -> i64 {
        reader
            .query_row(
                "SELECT count(*) FROM history WHERE data LIKE '%said while it''s held%'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while kept() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(kept(), 1, "{}", ergo.log());
}
