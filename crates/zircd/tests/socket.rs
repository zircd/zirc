//! zircd's socket against clients gone wrong, through a real daemon: a line that isn't a
//! request, or one past the cap, is answered with an error, and a client that connects
//! and says nothing, or never finishes its line, holds up nobody else meanwhile.

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use zirc_core::ipc::{MAX_REQUEST_BYTES, Request, Response};
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

/// Sends `bytes` raw on a new connection to `machine`, and the line it answers with.
async fn answer_to(machine: &Machine, bytes: &[u8]) -> String {
    let stream = UnixStream::connect(machine.socket()).await.unwrap();
    let (read, mut write) = stream.into_split();
    write.write_all(bytes).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(WAIT, BufReader::new(read).read_line(&mut line))
        .await
        .expect("no answer")
        .unwrap();
    line
}

/// Whether `machine` answers a status request as it should.
async fn still_answers(machine: &Machine) {
    let answer = tokio::time::timeout(WAIT, machine.try_request(Request::Status))
        .await
        .expect("the daemon is held up");
    assert!(matches!(answer, Ok(Response::Status(_))), "{answer:?}");
}

#[tokio::test]
async fn a_line_that_isnt_a_request_is_answered_with_an_error() {
    let ergo = require_ergo!();
    let alpha = Machine::start(&ergo, "alpha").await;
    let answer = answer_to(&alpha, b"{\"not\": \"a request\"}\n").await;
    assert!(answer.contains("not a request"), "{answer}");
    let answer = answer_to(&alpha, b"\xff\xfe garbage\n").await;
    assert!(answer.contains("not a request"), "{answer}");
    still_answers(&alpha).await;
}

#[tokio::test]
async fn a_request_past_the_cap_is_refused_and_the_daemon_carries_on() {
    let ergo = require_ergo!();
    let alpha = Machine::start(&ergo, "alpha").await;
    // A line that never ends: refused once it passes the cap, without being read whole.
    let long = vec![b' '; usize::try_from(MAX_REQUEST_BYTES).unwrap() + 1024];
    let stream = UnixStream::connect(alpha.socket()).await.unwrap();
    let (read, mut write) = stream.into_split();
    // Written as it's read: the daemon may answer and hang up before taking it all.
    let writing = tokio::spawn(async move {
        let _ = write.write_all(&long).await;
    });
    let mut line = String::new();
    tokio::time::timeout(WAIT, BufReader::new(read).read_line(&mut line))
        .await
        .expect("no answer")
        .unwrap();
    assert!(line.contains("longer than"), "{line}");
    writing.abort();
    still_answers(&alpha).await;
}

#[tokio::test]
async fn a_client_that_says_nothing_holds_up_nobody() {
    let ergo = require_ergo!();
    let alpha = Machine::start(&ergo, "alpha").await;
    // One connected and silent, one halfway through its line: neither has sent a request,
    // and the daemon waits for them on their own, answering everyone else meanwhile.
    let _silent = UnixStream::connect(alpha.socket()).await.unwrap();
    let mut half = UnixStream::connect(alpha.socket()).await.unwrap();
    half.write_all(b"{\"v\": 1, \"request\"").await.unwrap();
    for _ in 0..5 {
        still_answers(&alpha).await;
    }
    let _watching = alpha.watch(Some("cc-1")).await;
    still_answers(&alpha).await;
}
