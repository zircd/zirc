//! zircd restarting for an update: told a release was installed, a daemon answers what
//! was waiting, tells its watches it's restarting and ends to restart, and the next on the
//! same home carries on with its state. A file of its own, so its process forks nothing
//! else meanwhile: a child forked by another test holds the home's lock until it execs.

use zirc_core::ipc::{self, Request, Response};
use zirc_core::update::Version;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

type Asked = tokio::task::JoinHandle<Result<Response, ipc::Error>>;

/// `n` requests for `machine`'s status, sent at once.
fn statuses(machine: &Machine, n: usize) -> Vec<Asked> {
    (0..n)
        .map(|_| {
            let socket = machine.socket();
            tokio::spawn(async move { ipc::request(&socket, &Request::Status).await })
        })
        .collect()
}

/// Checks `asked` had an answer: the status, the daemon saying it's stopping, or no daemon
/// listening any more: refused, or reset unaccepted in the socket's backlog as the
/// listener closed (`Closed`), which never reached zircd. Never left waiting.
async fn answered(asked: Asked) {
    let answer = tokio::time::timeout(WAIT, asked)
        .await
        .expect("a request left waiting")
        .unwrap();
    match &answer {
        Ok(Response::Status(_)) | Err(ipc::Error::Io(_) | ipc::Error::Closed) => {}
        Err(ipc::Error::Daemon(message)) => assert_eq!(message, "zircd is stopping"),
        _ => panic!("{answer:?}"),
    }
}

/// Starts the next daemon on `machine`'s home, as zircd's main does after a restart, and
/// waits until it's connected, failing the test with why if it ended instead.
async fn next_daemon_connects(machine: &mut Machine) {
    machine.again();
    tokio::time::timeout(WAIT, async {
        while !machine
            .status()
            .await
            .is_some_and(|status| status.connected)
        {
            if machine.daemon.is_finished() {
                let ended = (&mut machine.daemon).await;
                panic!("the next daemon ended: {ended:?}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the next daemon connected");
}

#[tokio::test]
async fn an_updated_daemon_answers_what_waits_and_ends_to_restart() {
    let ergo = require_ergo!();
    let (updated, update) = tokio::sync::oneshot::channel();
    let hooks = zircd::Hooks {
        updated: Some(update),
        ..zircd::Hooks::default()
    };
    let mut alpha = Machine::hooked(&ergo, "alpha", hooks).await;
    let clone = clone();
    alpha.add_repo(None, clone.path()).await.unwrap();
    alpha
        .eventually(|status| status.channels == JOINED, "joining")
        .await;
    let mut watch = alpha.watch(Some("cc-1")).await;

    // Requests in flight as the update comes: each is answered, by this daemon or, once it
    // has stopped listening, refused at the socket, and none is left hanging or cut off.
    let asking = statuses(&alpha, 20);
    updated.send(Version(99, 0, 0)).unwrap();
    for asked in asking {
        answered(asked).await;
    }
    // The watch is told it's restarting, rather than seeing its connection drop.
    let told = tokio::time::timeout(WAIT, async {
        loop {
            match watch.next().await {
                Ok(Some(_)) => {}
                other => return other,
            }
        }
    })
    .await
    .expect("the watch was told nothing");
    assert!(
        matches!(&told, Err(ipc::Error::Daemon(message)) if message == ipc::RESTARTING),
        "{told:?}"
    );
    let ended = tokio::time::timeout(WAIT, &mut alpha.daemon)
        .await
        .expect("the daemon didn't end")
        .unwrap()
        .unwrap();
    assert_eq!(ended, zircd::Ended::Restart);

    // The next daemon on the home, as zircd's main starts it, has the repository still.
    next_daemon_connects(&mut alpha).await;
    alpha
        .eventually(|status| status.channels == JOINED, "joining again")
        .await;
    let Response::Repos { repos } = alpha.request(Request::RepoList).await else {
        panic!("not a repo list")
    };
    assert_eq!(repos.len(), 1);
    // And a watch watches it.
    let _watch = alpha.watch(Some("cc-1")).await;
}
