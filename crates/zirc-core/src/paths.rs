//! Where zirc keeps its files: `$ZIRC_HOME`, or `~/.zirc`.

use std::env;
use std::path::{Path, PathBuf};

/// zirc's directory: `$ZIRC_HOME` when set, otherwise `.zirc` in the home directory.
///
/// # Panics
///
/// When neither `$ZIRC_HOME` nor `$HOME` is set.
#[must_use]
pub fn home() -> PathBuf {
    env::var_os("ZIRC_HOME").map_or_else(
        || PathBuf::from(env::var_os("HOME").expect("HOME or ZIRC_HOME must be set")).join(".zirc"),
        PathBuf::from,
    )
}

/// Writes `text` to `path`, readable by this user only, for files that hold passwords. The
/// file is replaced whole, through a temporary file beside it, so a crash midway leaves the
/// old one rather than a truncated one.
///
/// # Errors
///
/// When the file or its directory can't be written.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".tmp");
    let temporary = path.with_file_name(name);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// The Unix socket zircd listens on.
#[must_use]
pub fn socket(home: &Path) -> PathBuf {
    home.join("zircd.sock")
}

/// Where zircd's output goes when `zirc daemon start` or the login service runs it: kept to
/// a size by zircd, the older lines in [`old_log`].
#[must_use]
pub fn log(home: &Path) -> PathBuf {
    home.join("zircd.log")
}

/// [`log`]'s older lines, from before it last reached its size.
#[must_use]
pub fn old_log(home: &Path) -> PathBuf {
    home.join("zircd.log.1")
}

/// The file the running zircd holds locked, its process id and a token of its own in it
/// (`<pid> <token>`): one zircd per home.
#[must_use]
pub fn lock(home: &Path) -> PathBuf {
    home.join("zircd.lock")
}

/// The token the zircd with `socket` wrote in its [`lock`] file when it started, if there is
/// one: no two zircds write the same, not even in one process, which a process id and a
/// socket file's inode and times can't promise (a new socket may get the old one's inode
/// and, on a file system whose times tick every few milliseconds, its time too).
#[must_use]
pub fn lock_token(socket: &Path) -> Option<String> {
    let text = std::fs::read_to_string(lock(socket.parent()?)).ok()?;
    text.split_whitespace().nth(1).map(str::to_owned)
}

/// The longest path a Unix socket can have: `sun_path`'s size, less the NUL after it.
const SOCKET_PATH_MAX: usize = if cfg!(target_os = "linux") { 107 } else { 103 };

/// Checks that `socket` is short enough for a Unix socket's path, which a deep `$HOME` or
/// `$ZIRC_HOME` may not be.
///
/// # Errors
///
/// When it's too long, saying what to do about it.
pub fn check_socket(socket: &Path) -> std::io::Result<()> {
    let len = socket.as_os_str().len();
    if len <= SOCKET_PATH_MAX {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "{} is {len} bytes, longer than a socket's path can be ({SOCKET_PATH_MAX}): set \
             ZIRC_HOME to a shorter directory",
            socket.display()
        ),
    ))
}

/// The daemon's settings: which server, which nick, which account.
#[must_use]
pub fn config(home: &Path) -> PathBuf {
    home.join("config.toml")
}

/// Who may query this machine's agents ([`crate::agents`]): a file of its own rather than a
/// table in `config.toml`, which zirc's older releases would refuse.
#[must_use]
pub fn agents(home: &Path) -> PathBuf {
    home.join("agents.toml")
}

/// The machine's own key, which zircd signs in with ([`crate::device_key::Software`]).
#[must_use]
pub fn device_key(home: &Path) -> PathBuf {
    home.join("device-key")
}

/// Passphrases, where the system has no secret store of its own. See
/// [`crate::secrets`].
#[must_use]
pub fn secrets(home: &Path) -> PathBuf {
    home.join("secrets.toml")
}

/// What sessions on this machine are called ([`crate::ipc::Request::Name`]), kept across
/// restarts.
#[must_use]
pub fn session_names(home: &Path) -> PathBuf {
    home.join("session-names.json")
}

/// The task each session on this machine is on ([`crate::ipc::Request::Task`]), kept across
/// restarts.
#[must_use]
pub fn session_tasks(home: &Path) -> PathBuf {
    home.join("session-tasks.json")
}

/// The task each session on this machine finished last and when, for its landing to claim
/// ([`crate::ipc::Request::Task`]), kept across restarts.
#[must_use]
pub fn session_tasks_finished(home: &Path) -> PathBuf {
    home.join("session-tasks-finished.json")
}

/// What each session on this machine's landing finishes, and where its `taking:` went, for
/// zircd to say `done:` when that landing lands, kept across restarts.
#[must_use]
pub fn session_tasks_landing(home: &Path) -> PathBuf {
    home.join("session-tasks-landing.json")
}

/// The full title each session was given ([`crate::ipc::Request::Name`]), by session.
#[must_use]
pub fn session_titles(home: &Path) -> PathBuf {
    home.join("session-titles.json")
}

/// Where its owner opens each session ([`crate::ipc::Request::Name`]), by session: kept on
/// this machine, never sent to the chat server.
#[must_use]
pub fn session_links(home: &Path) -> PathBuf {
    home.join("session-links.json")
}

/// Which agent tool sessions on this machine run in ([`crate::ipc::Request::Name`]), kept
/// across restarts.
#[must_use]
pub fn session_tools(home: &Path) -> PathBuf {
    home.join("session-tools.json")
}

/// Which picture sessions on this machine go by ([`crate::ipc::Request::Name`]), kept
/// across restarts.
#[must_use]
pub fn session_avatars(home: &Path) -> PathBuf {
    home.join("session-avatars.json")
}

/// When each session on this machine was last seen, by session, in seconds since the Unix
/// epoch: one not seen for a week is forgotten.
#[must_use]
pub fn session_seen(home: &Path) -> PathBuf {
    home.join("session-seen.json")
}

/// The topic channels the daemon has joined, kept across restarts.
#[must_use]
pub fn topics(home: &Path) -> PathBuf {
    home.join("topics.json")
}

/// The repositories the daemon watches, kept across restarts.
#[must_use]
pub fn repos(home: &Path) -> PathBuf {
    home.join("repos.json")
}

/// Where the daemon keeps what's an organisation's own, such as its topics, for each
/// organisation but the first, whose things are kept in `home` as a machine signed in to
/// one keeps them.
#[must_use]
pub fn org(home: &Path, org: &str) -> PathBuf {
    home.join("orgs").join(org)
}

/// The requests this machine's sessions asked its owner that are still open
/// ([`zirc_proto::request`]), kept across restarts.
#[must_use]
pub fn requests(home: &Path) -> PathBuf {
    home.join("requests.json")
}

/// How this machine's sessions' watches woke, counted by day and reason
/// ([`crate::ipc::WakeCount`]).
#[must_use]
pub fn wakes(home: &Path) -> PathBuf {
    home.join("wakes.json")
}

/// What concerned each session while none of its watches was open, told to its next watch
/// and kept across restarts for as long as zircd keeps it at all.
#[must_use]
pub fn missed(home: &Path) -> PathBuf {
    home.join("missed.json")
}

/// What sessions wait on (`zirc wait-for`, and the branches zircd follows for them), kept
/// across restarts.
#[must_use]
pub fn awaits(home: &Path) -> PathBuf {
    home.join("awaits.json")
}

/// The gotchas zircd heard in its repositories' gotchas rooms ([`crate::gotchas`]).
#[must_use]
pub fn gotchas(home: &Path) -> PathBuf {
    home.join("gotchas.json")
}

/// Which gotchas each session on this machine was told, kept across restarts: told once.
#[must_use]
pub fn gotchas_told(home: &Path) -> PathBuf {
    home.join("gotchas-told.json")
}

/// Where zircd asks what was said from when it next connects (`replay.json`): the newest
/// message it heard in each channel, and when it was last connected. Kept across restarts.
#[must_use]
pub fn replay(home: &Path) -> PathBuf {
    home.join("replay.json")
}

/// When each of this machine's agent sessions last had a `zirc watch` open
/// (`live-sessions.json`), so a restarting zircd keeps saying they're live
/// ([`zirc_proto::session::Live`]). Kept across restarts.
#[must_use]
pub fn live_sessions(home: &Path) -> PathBuf {
    home.join("live-sessions.json")
}

/// The leases the daemon has heard of ([`crate::leases`]), kept across restarts.
#[must_use]
pub fn leases(home: &Path) -> PathBuf {
    home.join("leases.json")
}

/// Where an older zircd logged the commands sessions ran, as zirc's tool hook told
/// it: removed at start.
#[must_use]
pub fn ran(home: &Path) -> PathBuf {
    home.join("ran.jsonl")
}

/// Where an older zircd kept whose each uncommitted hunk was, as zirc's tool hook told it:
/// removed at start.
#[must_use]
pub fn owners(home: &Path) -> PathBuf {
    home.join("owners.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_when_a_socket_path_is_too_long() {
        assert!(check_socket(Path::new("/Users/ada/.zirc/zircd.sock")).is_ok());
        let deep = PathBuf::from(format!("/tmp/{}/.zirc/zircd.sock", "deep/".repeat(30)));
        let err = check_socket(&deep).unwrap_err().to_string();
        assert!(err.contains("ZIRC_HOME"), "{err}");
    }

    #[test]
    fn a_zircd_is_told_from_the_last_by_its_lock_token() {
        let home = tempfile::tempdir().unwrap();
        let socket = socket(home.path());
        assert_eq!(lock_token(&socket), None);
        // The lock a zircd from before tokens left holds only its process id.
        std::fs::write(lock(home.path()), "42\n").unwrap();
        assert_eq!(lock_token(&socket), None);
        std::fs::write(lock(home.path()), "42 7-1\n").unwrap();
        assert_eq!(lock_token(&socket).as_deref(), Some("7-1"));
        std::fs::write(lock(home.path()), "42 7-2\n").unwrap();
        assert_eq!(lock_token(&socket).as_deref(), Some("7-2"));
    }

    #[test]
    fn writes_private_files_whole() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zirc/config.toml");
        write_private(&path, "old").unwrap();
        // An older file, made readable by others, is replaced by a private one.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.toml"], "no temporary file is left behind");
    }
}
