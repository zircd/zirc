//! zircd's log, `zircd.log` in zirc's directory ([`paths::log`]), kept to a size: past
//! [`CAP`], its lines move to `zircd.log.1` ([`paths::old_log`]), replacing what was there,
//! and it starts again empty.
//!
//! zircd does it itself, at start and every [`EVERY`], since what opens the file for it
//! can't: launchd (`StandardOutPath`) and `zirc daemon start` give it the file as its output
//! and never look at it again. Its lines are copied, then the file emptied in place (as
//! logrotate's `copytruncate`), rather than renamed: the open file is the one launchd
//! gave, which zircd can't swap for another without `unsafe`. Both open it to append, so
//! what's written after it's emptied starts at its beginning. In a terminal, zircd
//! opens the file itself ([`open`]) and keeps it small the same way. Under systemd its
//! output isn't the file, and it's left alone: the journal keeps its own size.

use std::fs::{File, Metadata, OpenOptions};
use std::os::fd::AsFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zirc_core::paths;

/// Whether this zircd writes its log itself ([`open`]), as it does in a terminal: then it's
/// kept small as when it's the output.
static OPENED: AtomicBool = AtomicBool::new(false);

/// Opens the log in `home` for zircd to write its log to itself, to append, as it does when
/// run in a terminal, whose output is for what it does ([`crate::activity`]).
///
/// # Errors
///
/// When `home` can't be made, or the file opened.
pub fn open(home: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(home)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::log(home))?;
    OPENED.store(true, Ordering::Relaxed);
    Ok(log)
}

/// The most the log holds before its lines move to the old one: a few weeks of an ordinary
/// machine's, and the two together at most twice it.
pub const CAP: u64 = 4 * 1024 * 1024;

/// How often the log's size is checked.
pub const EVERY: Duration = Duration::from_mins(10);

/// Checks the log in `home` every [`EVERY`], starting now, while zircd's output is it.
pub async fn keep_small(home: PathBuf) {
    let mut every = tokio::time::interval(EVERY);
    loop {
        every.tick().await;
        let log = paths::log(&home);
        if !OPENED.load(Ordering::Relaxed) && !is_output(&log) {
            return;
        }
        match rotate(&log, &paths::old_log(&home), CAP) {
            Ok(true) => tracing::info!(
                "the log reached {} MiB: its older lines are in {}",
                CAP / 1024 / 1024,
                paths::old_log(&home).display()
            ),
            Ok(false) => {}
            Err(err) => tracing::warn!("keeping {} small failed: {err}", log.display()),
        }
    }
}

/// Whether this process's standard error is the file at `log`.
fn is_output(log: &Path) -> bool {
    let stderr = std::io::stderr();
    let Ok(output) = stderr.as_fd().try_clone_to_owned() else {
        return false;
    };
    let output = File::from(output).metadata();
    match (output, std::fs::metadata(log)) {
        (Ok(output), Ok(log)) => same_file(&output, &log),
        _ => false,
    }
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// When `log` is bigger than `cap`: copies it to `old`, replacing it, empties it, and answers
/// true.
fn rotate(log: &Path, old: &Path, cap: u64) -> std::io::Result<bool> {
    if std::fs::metadata(log)?.len() <= cap {
        return Ok(false);
    }
    std::fs::copy(log, old)?;
    OpenOptions::new().write(true).open(log)?.set_len(0)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn past_its_cap_the_log_moves_to_the_old_one_and_starts_again() {
        let dir = tempfile::tempdir().unwrap();
        let (log, old) = (dir.path().join("zircd.log"), dir.path().join("zircd.log.1"));
        // Opened as launchd and `zirc daemon start` open it: to append.
        let mut writer = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        writer.write_all(b"one\n").unwrap();
        assert!(!rotate(&log, &old, 10).unwrap(), "under the cap");
        assert!(!old.exists());

        writer.write_all(b"two\nthree\n").unwrap();
        assert!(rotate(&log, &old, 10).unwrap());
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "one\ntwo\nthree\n");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
        writer.write_all(b"four\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "four\n",
            "the writer goes on at the start"
        );

        writer.write_all(b"five six seven\n").unwrap();
        assert!(rotate(&log, &old, 10).unwrap());
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "four\nfive six seven\n",
            "one old file, replaced"
        );
    }

    #[test]
    fn leaves_a_log_that_isnt_its_output_alone() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("zircd.log");
        std::fs::write(&log, "someone else's").unwrap();
        assert!(!is_output(&log));
        assert!(!is_output(&dir.path().join("missing")));
        let file = File::open(&log).unwrap().metadata().unwrap();
        assert!(same_file(&file, &std::fs::metadata(&log).unwrap()));
    }
}
