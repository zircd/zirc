//! Whether this machine has room for a lease, as its `config.toml` says
//! ([`zirc_core::settings::LeaseRoom`]): its load average and the space free on a disk, read
//! the way the machine's own tools say them (`/proc/loadavg` or `sysctl`, and `df`), so
//! zircd needs nothing more to read them.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use zirc_core::settings::LeaseRoom;

/// How long `sysctl` or `df` may take before it's given up on: `df` waits on a disk that has
/// stopped answering, such as a network mount.
const WITHIN: Duration = Duration::from_secs(5);

/// What `command` printed, run without blocking the daemon, as [`crate::procs`] runs `ps`.
/// `None` when it fails or takes longer than [`WITHIN`], when it's killed.
async fn said(mut command: Command) -> Option<String> {
    let running = command.kill_on_drop(true).output();
    let out = tokio::time::timeout(WITHIN, running).await.ok()?.ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The load average over five minutes: from `/proc/loadavg` on Linux, `sysctl vm.loadavg`
/// on macOS. `None` when neither says.
pub async fn load() -> Option<f64> {
    // Read from memory, not a disk: it doesn't wait.
    let text = if let Ok(text) = std::fs::read_to_string("/proc/loadavg") {
        text
    } else {
        let mut sysctl = Command::new("sysctl");
        sysctl.args(["-n", "vm.loadavg"]);
        said(sysctl).await?
    };
    five_minutes(&text)
}

/// The five-minute figure of a load average as `/proc/loadavg` (`4.6 33.9 51.1 …`) or
/// `sysctl` (`{ 4.6 33.9 51.1 }`) says it.
fn five_minutes(text: &str) -> Option<f64> {
    text.split_whitespace()
        .filter(|word| *word != "{" && *word != "}")
        .nth(1)?
        .parse()
        .ok()
}

/// The gigabytes free on the disk holding `path`, as `df -Pk` says. `None` when it can't.
pub async fn free_gb(path: &Path) -> Option<u64> {
    let mut df = Command::new("df");
    df.arg("-Pk").arg(path);
    available_kb(&said(df).await?).map(|kb| kb / (1024 * 1024))
}

/// The available kilobytes in `df -Pk`'s output: the fourth field of its second line.
fn available_kb(text: &str) -> Option<u64> {
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

/// Why this machine hasn't room as `room` asks, now ([`short_of`]), reading only what `room`
/// sets a limit on.
pub async fn short_now(room: &LeaseRoom) -> Option<String> {
    let load = if room.max_load.is_some() {
        load().await
    } else {
        None
    };
    let free = if room.min_free_gb.is_some() {
        free_gb(room.disk.as_deref().unwrap_or_else(|| Path::new("/"))).await
    } else {
        None
    };
    short_of(room, || load, |_| free)
}

/// Why this machine hasn't room as `room` asks, with its load and space as `load` and
/// `free` say: `load 52 is over 40`, `9 GB free on /, under 15`. `None` when it has room,
/// or when what a limit is about can't be read, which never holds a lease back.
pub fn short_of(
    room: &LeaseRoom,
    load: impl FnOnce() -> Option<f64>,
    free: impl FnOnce(&Path) -> Option<u64>,
) -> Option<String> {
    let mut why = Vec::new();
    if let Some(max) = room.max_load
        && let Some(load) = load()
        && load > f64::from(max)
    {
        why.push(format!("load {load:.0} is over {max}"));
    }
    if let Some(min) = room.min_free_gb {
        let disk = room.disk.as_deref().unwrap_or_else(|| Path::new("/"));
        if let Some(free) = free(disk)
            && free < min
        {
            why.push(format!("{free} GB free on {}, under {min}", disk.display()));
        }
    }
    (!why.is_empty()).then(|| why.join(" and "))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn reads_the_machines_own_words() {
        assert_eq!(five_minutes("4.65 33.93 51.12 3/812 4113\n"), Some(33.93));
        assert_eq!(five_minutes("{ 4.65 33.93 51.12 }\n"), Some(33.93));
        assert_eq!(five_minutes(""), None);
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                  /dev/disk3s1s1 482797652 12582912 37748736 25% /\n";
        assert_eq!(available_kb(df), Some(37_748_736));
        assert_eq!(available_kb("Filesystem\n"), None);
    }

    #[tokio::test]
    async fn reads_this_machines_load_and_space_without_blocking() {
        // A tool that isn't there or fails says nothing, as a test's sandbox has no sysctl.
        assert_eq!(said(Command::new("zirc-no-such-tool")).await, None);
        assert_eq!(said(Command::new("false")).await, None);
        let mut echo = Command::new("echo");
        echo.arg("{ 4.65 33.93 51.12 }");
        assert_eq!(
            five_minutes(&said(echo).await.unwrap_or_default()),
            Some(33.93)
        );
        // No limit set, nothing read, nothing short.
        assert_eq!(short_now(&LeaseRoom::default()).await, None);
        // More load than any machine has, and no space asked for: room.
        let room = LeaseRoom {
            max_load: Some(u32::MAX),
            min_free_gb: Some(0),
            disk: None,
        };
        assert_eq!(short_now(&room).await, None);
    }

    #[test]
    fn says_what_the_machine_is_short_of() {
        let room = LeaseRoom {
            max_load: Some(40),
            min_free_gb: Some(15),
            disk: Some(PathBuf::from("/Users")),
        };
        assert_eq!(short_of(&room, || Some(12.0), |_| Some(40)), None);
        assert_eq!(
            short_of(&room, || Some(52.4), |_| Some(40)).as_deref(),
            Some("load 52 is over 40")
        );
        assert_eq!(
            short_of(
                &room,
                || Some(52.4),
                |disk| {
                    assert_eq!(disk, Path::new("/Users"));
                    Some(9)
                }
            )
            .as_deref(),
            Some("load 52 is over 40 and 9 GB free on /Users, under 15")
        );
        // What can't be read holds nothing back, nor does a limit not set.
        assert_eq!(short_of(&room, || None, |_| None), None);
        assert_eq!(
            short_of(&LeaseRoom::default(), || Some(99.0), |_| Some(0)),
            None
        );
    }
}
