//! Reaping every child zircd started, before it starts again from its binary.
//!
//! zircd restarts by `exec` (main.rs), keeping its pid. What it was running as it wound down,
//! a `git` given up on when its task was dropped, is killed then (`kill_on_drop`), but tokio
//! reaps such a child only later, on a SIGCHLD. The `exec` came first, and the new image
//! doesn't know those pids, so nothing ever waited for them: the user's zircd had seven
//! zombies, each born a few seconds after it logged "restarting, as asked" (26 Nov 2026).

use std::time::{Duration, Instant};

/// Waits for this process's children to end, and reaps them, for at most `within`: none
/// waits at all, reaping only those already ended. Gives how many it reaped.
///
/// Only once nothing else waits for a child of this process: before tokio's runtime starts,
/// or after it has shut down. A child reaped here is one whose waiter would find it gone.
#[must_use]
pub fn reap(within: Duration) -> usize {
    let deadline = Instant::now() + within;
    let mut reaped = 0;
    loop {
        match rustix::process::waitpid(None, rustix::process::WaitOptions::NOHANG) {
            // One ended, perhaps more.
            Ok(Some(_)) => reaped += 1,
            // Some still run.
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            // None left (ECHILD), or out of time.
            Ok(None) | Err(_) => return reaped,
        }
    }
}
