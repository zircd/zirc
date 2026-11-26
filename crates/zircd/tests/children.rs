//! zircd reaps every child it started before it starts again from its binary
//! (`zircd::children`), where the user's zircd had kept seven zombies (26 Nov 2026). A test
//! binary of its own, and one test, as reaping takes any child of the process, another
//! test's too.

use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, test_kill_process};
use zircd::children::reap;

/// Whether `pid` is a process still, running or a zombie: one reaped is none.
fn exists(pid: u32) -> bool {
    let pid = Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    match test_kill_process(pid) {
        Ok(()) => true,
        Err(Errno::SRCH) => false,
        Err(err) => panic!("asking whether {pid:?} runs: {err}"),
    }
}

#[test]
fn reaps_what_it_started() {
    // As zircd winds down to restart: a task's `git`, given up on, is dropped and killed,
    // and the runtime stops before tokio reaps it.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let given_up: Vec<u32> = runtime.block_on(async {
        (0..3)
            .map(|_| {
                let child = tokio::process::Command::new("sleep")
                    .arg("60")
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap();
                child.id().unwrap()
            })
            .collect()
    });
    runtime.shutdown_timeout(Duration::from_secs(5));
    // And one never waited for, which ends by itself.
    let ended = std::process::Command::new("true").spawn().unwrap().id();
    assert!(exists(ended), "a child never waited for stays until reaped");

    let started = Instant::now();
    // Four, unless tokio caught one ending as it was dropped.
    assert!(reap(Duration::from_secs(10)) >= 1);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "reaping waited past its children's end"
    );
    for pid in given_up.into_iter().chain([ended]) {
        assert!(!exists(pid), "{pid} wasn't reaped");
    }

    // None waits when asked not to, though a child still runs; that one is reaped once
    // it has ended.
    #[allow(
        clippy::zombie_processes,
        reason = "reaped by reap(), as zircd reaps its own"
    )]
    let mut running = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let started = Instant::now();
    assert_eq!(reap(Duration::ZERO), 0);
    assert!(started.elapsed() < Duration::from_secs(1));
    running.kill().unwrap();
    assert_eq!(reap(Duration::from_secs(10)), 1);
    assert!(!exists(running.id()));
}
