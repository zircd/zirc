//! A child process that ends with this one, however this one ends.
//!
//! Dropping an [`crate::Ergo`] stops its server, but a process that's killed runs no `Drop`:
//! Bazel's timeout, a cancelled landing, Ctrl-C, or a driver stopping `serve` with SIGTERM, as
//! the Android app's end-to-end run did after every run. Its Ergo then ran on for days, its
//! parent gone (16 of them on the Mac, 26 Nov 2026). So each child has a watcher beside it, a
//! shell reading a pipe whose other end only this process holds: when this process ends, the
//! system closes that end, and the watcher kills the child. That holds on macOS and Linux
//! alike, where Linux's `PR_SET_PDEATHSIG` would hold on Linux alone, and fire when the thread
//! that started the child ends rather than the process.

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// A running child, killed when this is dropped or when this process ends.
pub struct Tethered {
    child: Child,
    /// The shell that kills `child` once its standard input closes; none once the child is
    /// being stopped, so it never kills another process given the pid after.
    watcher: Option<Child>,
}

impl Tethered {
    /// Starts `command`, tied to this process.
    ///
    /// # Errors
    ///
    /// When the command or its watcher doesn't start; the command is stopped then.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let child = command.spawn()?;
        // Kills the child (`$1`) once nothing can write to its standard input: the pipe's end
        // here is close-on-exec, so no other child holds it. By the shell's own kill, as CI's
        // image has no kill command (build 3586).
        let watcher = Command::new("sh")
            .args([
                "-c",
                r#"read -r _; kill -KILL "$1" 2>/dev/null"#,
                "tether",
                &child.id().to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut tethered = Self {
            child,
            watcher: None,
        };
        tethered.watcher = Some(watcher?);
        Ok(tethered)
    }

    /// The child's process id.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// The child's exit status, if it has ended.
    ///
    /// # Errors
    ///
    /// As [`Child::try_wait`].
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Stops the child as a service manager would: SIGTERM, so it finishes what it's
    /// writing, then SIGKILL once `grace` has passed. It has ended once this returns.
    pub fn stop(&mut self, grace: Duration) {
        self.untether();
        // The shell's own kill, as for the watcher.
        let _ = Command::new("sh")
            .args(["-c", &format!("kill -TERM {}", self.child.id())])
            .status();
        let deadline = Instant::now() + grace;
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Once waited for, its pid could be another's: `kill` then signals nothing.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Stops the watcher, before the child is stopped.
    fn untether(&mut self) {
        if let Some(mut watcher) = self.watcher.take() {
            let _ = watcher.kill();
            let _ = watcher.wait();
        }
    }
}

impl Drop for Tethered {
    fn drop(&mut self) {
        self.untether();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
