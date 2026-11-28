//! `zirc daemon`: starting, stopping and restarting zircd.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::Subcommand;
use zirc_core::ipc::{self, Request, Response};
use zirc_core::paths;
use zirc_proto::PROGRAM;

use crate::client::{not_running, shown, tail};
use crate::service;

/// How long `daemon start` and `daemon stop` wait for zircd.
pub(crate) const DAEMON_WAIT: Duration = Duration::from_secs(10);

/// How long a restart is waited for: longer than a start or a stop, as zircd reads every
/// repository it watches before it listens again. On a machine with some thirty worktrees
/// it was back after 15 to 30 s, and `zirc update` had said it "didn't come back within
/// 10s" each time, of a zircd that was fine (26 Nov 2026).
pub(crate) const RESTART_WAIT: Duration = Duration::from_secs(90);

#[derive(Debug, Subcommand)]
pub(crate) enum DaemonCommand {
    /// Start zircd in the background: by its login service once installed.
    ///
    /// Once `zirc daemon install` (or `zirc login --background`) has set zircd up to run at
    /// login, launchd or systemd starts it, apart from this shell. Otherwise it runs in a
    /// process group of its own, so stopping this shell doesn't stop it, but with nothing to
    /// restart it. To watch what it does, run `zircd` in a terminal tab instead.
    Start {
        /// The zircd to run. Next to this zirc, or on the PATH, unless set.
        #[arg(long, env = "ZIRC_ZIRCD", hide = true)]
        zircd: Option<PathBuf>,
    },
    /// Stop zircd.
    Stop,
    /// Restart zircd from its binary, as after an update: watches carry on.
    Restart,
    /// Run zircd at login, and keep it running: launchd on macOS, systemd on Linux.
    Install {
        /// The zircd to run. Next to this zirc, or on the PATH, unless set.
        #[arg(long, env = "ZIRC_ZIRCD", hide = true)]
        zircd: Option<PathBuf>,
    },
    /// Stop running zircd at login.
    Uninstall,
}

/// Restarts zircd from its binary, as after an update, and says what it runs once it's
/// back. Whether zircd took the request: one that isn't running runs its binary when it
/// starts anyway, and an older one may not know it.
pub(crate) async fn restart(home: &Path, out: &mut impl Write) -> Result<bool> {
    let socket = paths::socket(home);
    // The restarted zircd listens on a socket of its own: a new file where the old one was.
    let before = socket_id(&socket);
    match ipc::request(&socket, &Request::Restart).await {
        // zircd may restart before its answer is out.
        Ok(Response::Done) | Err(ipc::Error::Closed) => {}
        Err(ipc::Error::Io(_)) => {
            writeln!(out, "{}", not_running_said(home))?;
            return Ok(true);
        }
        Ok(_) | Err(_) => return Ok(false),
    }
    let started = tokio::time::Instant::now();
    let mut said = false;
    loop {
        if socket_id(&socket) != before
            && let Ok(Response::Status(status)) = ipc::request(&socket, &Request::Status).await
        {
            let version = status
                .version
                .unwrap_or_else(|| "an older zircd".to_owned());
            writeln!(out, "zircd restarted, running {version}")?;
            return Ok(true);
        }
        let waited = started.elapsed();
        if waited > RESTART_WAIT {
            // Still not a failure it can name: it may be reading a large machine's
            // repositories yet.
            bail!(
                "zircd isn't back after {RESTART_WAIT:?}, and may still be starting: {PROGRAM} \
                 status says when it is, and its log is {}",
                shown(&paths::log(home))
            );
        }
        if waited > DAEMON_WAIT && !said {
            writeln!(
                out,
                "zircd is restarting: it reads each repository it watches first"
            )?;
            said = true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Which zircd's socket this is, if it's there: the file, and the token the zircd that made it
/// wrote in its lock file. A file system may give a new socket the old one's inode, and one
/// whose times tick every few milliseconds its time too (a zircd that stops and starts again
/// at once, on Linux), but not the token.
pub(crate) type SocketId = (u64, i64, i64, Option<String>);

pub(crate) fn socket_id(socket: &Path) -> Option<SocketId> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(socket).ok().map(|metadata| {
        (
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
            zirc_core::paths::lock_token(socket),
        )
    })
}

pub(crate) async fn start(home: &Path, zircd: &Path, out: &mut impl Write) -> Result<()> {
    let socket = paths::socket(home);
    if ipc::request(&socket, &Request::Status).await.is_ok() {
        writeln!(out, "zircd is already running")?;
        return Ok(());
    }
    std::fs::create_dir_all(home)?;
    let log_path = paths::log(home);
    // Run at login: the service manager starts it, apart from this shell and its
    // environment, and restarts it if it fails.
    let mut child = if service::installed_for(home) {
        service::start()?;
        None
    } else {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        Some(
            detached(zircd)
                .env("ZIRC_HOME", home)
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()
                .with_context(|| format!("starting {}", zircd.display()))?,
        )
    };

    let deadline = tokio::time::Instant::now() + DAEMON_WAIT;
    loop {
        if ipc::request(&socket, &Request::Status).await.is_ok() {
            writeln!(out, "zircd is running, logging to {}", shown(&log_path))?;
            return Ok(());
        }
        let exited = match child.as_mut() {
            Some(child) => child.try_wait()?,
            None => None,
        };
        if let Some(status) = exited {
            bail!(
                "zircd exited ({status}):\n{}",
                tail(&std::fs::read_to_string(&log_path).unwrap_or_default())
            );
        }
        if tokio::time::Instant::now() > deadline {
            bail!(
                "zircd didn't start within {DAEMON_WAIT:?}: see {}",
                shown(&log_path)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// zircd as a process of its own: in a new process group, so stopping the shell or the
/// agent that ran `zirc daemon start` doesn't stop it too. It keeps that shell's environment
/// and session; only `zirc daemon install`'s service runs it apart from both.
pub(crate) fn detached(zircd: &Path) -> Process {
    use std::os::unix::process::CommandExt as _;
    let mut command = Process::new(zircd);
    command.process_group(0);
    command
}

pub(crate) async fn stop(socket: &Path, out: &mut impl Write) -> Result<()> {
    match ipc::request(socket, &Request::Stop).await {
        // zircd may exit before its answer is out.
        Ok(_) | Err(ipc::Error::Closed) => {}
        Err(err) => return Err(not_running(socket, err)),
    }
    let deadline = tokio::time::Instant::now() + DAEMON_WAIT;
    while socket.exists() {
        if tokio::time::Instant::now() > deadline {
            bail!("zircd didn't stop within {DAEMON_WAIT:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    writeln!(out, "zircd stopped")?;
    Ok(())
}

/// What to do on a machine zirc isn't set up on yet.
pub(crate) fn not_set_up() -> String {
    format!(
        "this machine isn't set up for {PROGRAM} yet: run `zirc login`, or `zirc init --server \
         <host>` for a server of your own"
    )
}

/// Fails, saying what to do, unless `zirc login` or `zirc init` has said which server zircd
/// connects to.
pub(crate) fn configured(home: &Path) -> Result<()> {
    if !paths::config(home).exists() {
        bail!(not_set_up());
    }
    Ok(())
}

/// The command that runs zircd for `home` in a terminal, as people run it until they put it
/// in the background: `zircd`, by its whole path when the PATH's isn't `zircd`'s, and with
/// `--home` when `home` isn't zirc's usual directory.
pub(crate) fn foreground(home: &Path, zircd: &Path) -> String {
    let on_path = std::env::var_os("PATH").is_some_and(|path| {
        let found = zircd.canonicalize().ok();
        std::env::split_paths(&path).any(|dir| {
            dir.join("zircd")
                .canonicalize()
                .is_ok_and(|it| found.as_ref() == Some(&it))
        })
    });
    let command = if zircd == Path::new("zircd") || on_path {
        "zircd".to_owned()
    } else {
        shown(zircd)
    };
    if home == paths::home() {
        command
    } else {
        format!("{command} --home {}", shown(home))
    }
}

/// The command that runs zircd for `home` in a terminal ([`foreground`]), with the zircd
/// next to this zirc or the PATH's.
fn foreground_here(home: &Path) -> String {
    zircd_binary(None).map_or_else(|_| "zircd".to_owned(), |zircd| foreground(home, &zircd))
}

/// What a command that needs zircd says when it isn't running for `home`: how to start it.
/// One whose login service was installed starts again that way; otherwise it runs in a
/// terminal tab of its own.
pub(crate) fn not_running_said(home: &Path) -> String {
    if service::installed_for(home) {
        "zircd isn't running: `zirc daemon start` starts its login service again".to_owned()
    } else {
        format!(
            "zircd isn't running: start it in a terminal tab of its own with `{}`",
            foreground_here(home)
        )
    }
}

/// What `zirc watch` says when zircd isn't running for `home`, to the agent that runs it,
/// which never starts zircd: what to tell its person, once, and to carry on without zirc.
pub(crate) fn watch_not_running_said(home: &Path) -> String {
    let how = if service::installed_for(home) {
        "`zirc daemon start` starts its login service again".to_owned()
    } else {
        format!(
            "it runs in a terminal tab of its own with `{}`, or at login once `zirc daemon \
             install` has set it up",
            foreground_here(home)
        )
    };
    format!(
        "zircd isn't running on this machine. An agent never starts it: tell your user once \
         that {how}, and carry on without zirc until they say it's up; then start your watch \
         again."
    )
}

/// The zircd to run: the one asked for, the one next to this zirc, or the PATH's.
pub(crate) fn zircd_binary(asked: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = asked {
        return Ok(path);
    }
    let sibling = std::env::current_exe()?.with_file_name("zircd");
    Ok(if sibling.exists() {
        sibling
    } else {
        PathBuf::from("zircd")
    })
}

/// `zirc daemon`.
pub(crate) async fn daemon(
    home: &Path,
    socket: &Path,
    command: DaemonCommand,
    out: &mut impl Write,
) -> Result<()> {
    match command {
        DaemonCommand::Start { zircd } => {
            configured(home)?;
            start(home, &zircd_binary(zircd)?, out).await
        }
        DaemonCommand::Stop => stop(socket, out).await,
        DaemonCommand::Restart => {
            if !restart(home, out).await? {
                bail!("this zircd can't restart itself: zirc daemon stop, then zirc daemon start");
            }
            Ok(())
        }
        DaemonCommand::Install { zircd } => {
            configured(home)?;
            service::install(home, &zircd_binary(zircd)?, out)
        }
        DaemonCommand::Uninstall => service::uninstall(out),
    }
}
