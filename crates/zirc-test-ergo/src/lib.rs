//! A throwaway Ergo server for integration tests: the pinned release with its own
//! `default.yaml`, a fresh database in a temporary directory, and two listeners on free
//! loopback ports: plaintext IRC, and IRC over WebSocket for the web client. It stops when
//! dropped.
//!
//! Bazel hands tests the binary and config through `ZIRC_ERGO` and `ZIRC_ERGO_CONFIG`.
//! Without them, as under plain `cargo test`, [`Ergo::start`] returns `None` and the test
//! should return early: it has said it skipped. Under Bazel a test without them fails.
//!
//! [`wait`] is how a test waits for something, and [`require_ergo!`] how it skips without
//! Ergo, saying so.
//!
//! [`web`] stands in for zirc.dev, for the clients that log in there, [`jwt`] for the
//! tokens it signs, and [`git`] makes the repositories they work in.
//!
//! [`signin`] is a zirc.dev that signs an app in for real, registering its account on the
//! server, for end-to-end runs of an app, which `serve` starts when asked.
//!
//! [`stage`] is for tests in other languages, which start a server with the `serve` binary:
//! zirc's events as the IRC lines that say them, so no harness writes zirc's framing itself.
//!
//! [`scenario`] is the one path every app's end-to-end test walks, the teammate's side of
//! it and what the server heard said once, and [`control`] is how a driver asks `serve` for
//! it over HTTP. `e2e.sh` is that for a driver in shell.

pub mod control;
pub mod git;
mod http;
pub mod jwt;
pub mod scenario;
pub mod signin;
pub mod stage;
pub mod wait;
pub mod web;

pub use wait::wait;
/// The overrides that keep history as zirc's servers do, in SQLite across restarts, for
/// [`Ergo::start_with`]: without them a test's server keeps it in memory only.
pub use zirc_sidecar::environment::HISTORY;

use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs, io::Write as _};

use tempfile::TempDir;

/// How long a server gets to start listening.
const STARTUP: Duration = Duration::from_secs(15);

/// How many ports a server tries before giving up.
const ATTEMPTS: usize = 5;

/// Starts an [`Ergo`], or ends the test, which returns nothing, outside Bazel, where
/// [`Ergo::start`] has said it skipped.
///
/// ```ignore
/// let ergo = zirc_test_ergo::require_ergo!();
/// ```
#[macro_export]
macro_rules! require_ergo {
    () => {
        match $crate::Ergo::start() {
            Some(ergo) => ergo,
            None => return,
        }
    };
}

/// A running Ergo.
pub struct Ergo {
    child: Child,
    port: u16,
    ws_port: u16,
    /// Where it keeps its config, database and history.
    dir: PathBuf,
    /// The temporary directory `dir` is, removed with the server; none for a directory given
    /// to [`Ergo::start_in`], which stays.
    _temporary: Option<TempDir>,
    /// How to run it again: the binary and its environment.
    binary: PathBuf,
    env: Vec<(String, String)>,
}

/// The operator every test server has, its name and password, as a tenant's sidecar has one
/// (`OPER admin <password>`): to set machines' hosts, as the sidecar does.
pub const OPER: (&str, &str) = ("admin", "zirc-test-oper");

/// [`OPER`]'s password, as `ergo genpasswd` hashed it.
const OPER_PASSWORD_HASH: &str = "$2a$04$kSlxwEfJ7JEZ1afiRUlCZey11yz4ZUhMznqxn4zJ0G9ehAQqTFWKC";

impl Ergo {
    /// Starts a server with the release's defaults, or returns `None` outside Bazel.
    ///
    /// # Panics
    ///
    /// When the server doesn't start.
    #[must_use]
    pub fn start() -> Option<Self> {
        Self::start_with(&[])
    }

    /// Starts a server with extra environment overrides, such as
    /// `("ERGO__SERVER__PASSWORD", hash)`. See Ergo's manual on environment variables.
    ///
    /// # Panics
    ///
    /// When the server doesn't start.
    #[must_use]
    pub fn start_with(overrides: &[(&str, &str)]) -> Option<Self> {
        let (binary, config) = locate()?;
        // A free port can be taken by another test's server before this one listens on it,
        // as tests in other processes do: then this one gives up, and starts on another.
        for _ in 0..ATTEMPTS {
            let dir = tempfile::tempdir().expect("temporary directory");
            let [port, ws_port] = free_ports();
            let path = dir.path().to_owned();
            if let Some(server) = Self::launch(
                &binary,
                &config,
                path,
                Some(dir),
                [port, ws_port],
                overrides,
            ) {
                return Some(server);
            }
        }
        panic!("Ergo found no free port in {ATTEMPTS} tries");
    }

    /// Starts a server kept in `dir`, which stays when it stops, on `port` for IRC and
    /// `ws_port` for IRC over WebSocket, with extra environment overrides: for a server a
    /// developer runs, such as a team seeded with history (zirc-app's `team`). Its database
    /// is made if `dir` has none. `None` outside Bazel, like [`Ergo::start`].
    ///
    /// # Panics
    ///
    /// When the server doesn't start, as when another has one of the ports.
    #[must_use]
    pub fn start_in(
        dir: &Path,
        port: u16,
        ws_port: u16,
        overrides: &[(&str, &str)],
    ) -> Option<Self> {
        let (binary, config) = locate()?;
        fs::create_dir_all(dir).expect("making the server's directory");
        let server = Self::launch(
            &binary,
            &config,
            dir.to_owned(),
            None,
            [port, ws_port],
            overrides,
        );
        assert!(
            server.is_some(),
            "Ergo couldn't listen on {port} or {ws_port}: is another server running?"
        );
        server
    }

    /// Runs `binary` with the release's `config` in `dir` on `ports`, once its database is
    /// made, or `None` when another took one of the ports first.
    fn launch(
        binary: &Path,
        config: &Path,
        dir: PathBuf,
        temporary: Option<TempDir>,
        [port, ws_port]: [u16; 2],
        overrides: &[(&str, &str)],
    ) -> Option<Self> {
        // Written anew rather than copied, which would carry over Bazel's read-only
        // permissions.
        let default_yaml = fs::read_to_string(config).expect("reading Ergo's default.yaml");
        fs::write(
            dir.join("ircd.yaml"),
            zirc_sidecar::config::without_listeners(&default_yaml),
        )
        .expect("writing ircd.yaml");
        let listeners = format!(
            r#"{{"127.0.0.1:{port}": {{}}, "127.0.0.1:{ws_port}": {{"websocket": true}}}}"#
        );

        // As fast as zirc's servers let a connection send, not as Ergo's default does (5
        // lines, then 2 a second, with 16 history reads free): past those, a second
        // `zirc catchup` on one connection took 7.4 s, and zirc_cli_test's wait ran out
        // on it in about half the published tree's runs.
        let rate = zirc_proto::desired::RateLimit::USUAL;
        let (burst, per_second) = (rate.burst.to_string(), rate.per_second.to_string());
        // Later ones win: the overrides.
        let env: Vec<(String, String)> = [
            ("ERGO__FAKELAG__BURST_LIMIT", burst.as_str()),
            ("ERGO__FAKELAG__MESSAGES_PER_WINDOW", per_second.as_str()),
            ("ERGO__LANGUAGES__ENABLED", "false"),
            ("ERGO__OPERS__ADMIN__PASSWORD", OPER_PASSWORD_HASH),
            ("ERGO__NETWORK__NAME", "zirc-test"),
            ("ERGO__SERVER__NAME", "irc.zirc.test"),
            ("ERGO__SERVER__LISTENERS", &listeners),
        ]
        .into_iter()
        .chain(overrides.iter().copied())
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();

        if !dir.join("ircd.db").exists() {
            let initdb = command(binary, &dir, &env, &["initdb", "--quiet"])
                .output()
                .expect("running ergo initdb");
            assert!(
                initdb.status.success(),
                "ergo initdb failed:\n{}{}",
                String::from_utf8_lossy(&initdb.stdout),
                String::from_utf8_lossy(&initdb.stderr)
            );
        }
        let child = run(binary, &dir, &env);
        let mut server = Self {
            child,
            port,
            ws_port,
            dir,
            _temporary: temporary,
            binary: binary.to_owned(),
            env,
        };
        server.listens(0).then_some(server)
    }

    /// Stops the server and starts it again on the same port with the same database, as a
    /// server restarting for an upgrade does: its clients' connections end, and it's
    /// listening again once this returns.
    ///
    /// # Panics
    ///
    /// When it doesn't start again.
    pub fn restart(&mut self) {
        // Stopped as an upgrade stops it, so it finishes writing what it took to its history
        // first: killed, a message heard live just before could be gone after (build 3581).
        // By the shell's own kill, as CI's image has no kill command (build 3586), and killed
        // after all if it hasn't ended in a while, so a restart never waits forever.
        let _ = Command::new("sh")
            .args(["-c", &format!("kill -TERM {}", self.child.id())])
            .status();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while matches!(self.child.try_wait(), Ok(None)) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let before = self.log().matches(&Self::listening(self.port)).count();
        self.child = run(&self.binary, &self.dir, &self.env);
        assert!(self.listens(before), "Ergo didn't listen on its port again");
    }

    /// The port to connect to on 127.0.0.1.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The port to connect to on 127.0.0.1 for IRC over WebSocket (`ws://`, subprotocol
    /// `text.ircv3.net`), from any origin.
    #[must_use]
    pub fn ws_port(&self) -> u16 {
        self.ws_port
    }

    /// Its history database, as Ergo keeps it beside its config.
    #[must_use]
    pub fn history(&self) -> std::path::PathBuf {
        self.dir.join("ergo_history.db")
    }

    /// The server's output so far, for failure messages.
    #[must_use]
    pub fn log(&self) -> String {
        fs::read_to_string(self.dir.join("ergo.log")).unwrap_or_default()
    }

    /// The bcrypt hash Ergo's config wants for `password`, as `ergo genpasswd` makes it.
    /// `None` outside Bazel, like [`Ergo::start`].
    ///
    /// # Panics
    ///
    /// When `ergo genpasswd` fails.
    #[must_use]
    pub fn hash_password(password: &str) -> Option<String> {
        let (binary, _) = locate()?;
        let mut child = Command::new(binary)
            .args(["genpasswd", "--quiet"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("running ergo genpasswd");
        // It asks for the password twice.
        writeln!(child.stdin.take().expect("stdin"), "{password}\n{password}")
            .expect("writing to ergo genpasswd");
        let output = child.wait_with_output().expect("ergo genpasswd");
        assert!(output.status.success(), "ergo genpasswd failed");
        let hash = String::from_utf8(output.stdout).expect("UTF-8 hash");
        Some(hash.trim().to_owned())
    }

    /// What the log says once the server listens on `port`.
    fn listening(port: u16) -> String {
        listening_line(port)
    }

    /// Whether the server listens on its ports, once its log says so more than `before`
    /// times, or `false` when another took one of them first.
    fn listens(&mut self, before: usize) -> bool {
        let listening = [Self::listening(self.port), Self::listening(self.ws_port)];
        let deadline = Instant::now() + STARTUP;
        loop {
            let log = self.log();
            if listening
                .iter()
                .all(|line| log.matches(line.as_str()).count() > before)
            {
                return true;
            }
            if self.child.try_wait().ok().flatten().is_some() {
                assert!(log.contains("couldn't listen"), "Ergo stopped:\n{log}");
                return false;
            }
            assert!(
                Instant::now() < deadline,
                "Ergo didn't start listening within {STARTUP:?}:\n{log}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Ergo {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `ergo` with `args`, in the server's directory `dir` and environment `env`.
fn command(binary: &Path, dir: &Path, env: &[(String, String)], args: &[&str]) -> Command {
    let mut command = Command::new(binary);
    command
        .args(args)
        .arg("--conf")
        .arg(dir.join("ircd.yaml"))
        .current_dir(dir)
        .envs(env.iter().map(|(key, value)| (key, value)));
    command
}

/// Runs the server in `dir`, adding to its log there.
fn run(binary: &Path, dir: &Path, env: &[(String, String)]) -> Child {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("ergo.log"))
        .expect("opening ergo.log");
    command(binary, dir, env, &["run"])
        .stdout(log.try_clone().expect("log file"))
        .stderr(log)
        .spawn()
        .expect("starting ergo")
}

/// The Ergo binary and its `default.yaml`, from the environment Bazel sets.
fn locate() -> Option<(PathBuf, PathBuf)> {
    let found = env::var_os("ZIRC_ERGO").zip(env::var_os("ZIRC_ERGO_CONFIG"));
    if found.is_none() {
        missing();
    }
    found.map(|(binary, config)| (absolute(binary.as_ref()), absolute(config.as_ref())))
}

/// Says there's no Ergo, where the person running the test sees it: a test without one
/// passes having tested nothing, so it says it skipped, on stderr itself, as the test harness
/// keeps a passing test's `eprintln!` to itself.
///
/// # Panics
///
/// Under Bazel (`TEST_TARGET` is set), which gives Ergo to the targets that start it: one
/// without it lacks `ergo = True` in its BUILD file, and passing would hide that.
fn missing() {
    use std::io::Write as _;
    let thread = std::thread::current();
    let test = thread.name().filter(|name| *name != "main");
    if let Some(target) = env::var_os("TEST_TARGET") {
        panic!(
            "{} needs Ergo, but {} isn't given it: say ergo = True in its BUILD file",
            test.unwrap_or("this test"),
            target.to_string_lossy()
        );
    }
    let said = match test {
        Some(test) => format!("skipped {test}: it needs Ergo, which bazel test gives it"),
        None => "ZIRC_ERGO isn't set, so there's no Ergo to start: run it with Bazel".to_owned(),
    };
    let _ = writeln!(std::io::stderr(), "{said}");
}

/// Bazel's paths are relative to the test's working directory, which Ergo doesn't share.
fn absolute(path: &Path) -> PathBuf {
    env::current_dir().expect("working directory").join(path)
}

/// What Ergo's log says once it listens on `port` of 127.0.0.1: how a test knows the server
/// on a port is its own, where a connection could reach another test's that took it first.
#[must_use]
pub fn listening_line(port: u16) -> String {
    format!("now listening on 127.0.0.1:{port},")
}

/// Ports nothing listens on right now, all different: each is held until all are picked,
/// since Linux can give a port it just gave back out again. Two the same would make Ergo's
/// listeners one, a WebSocket one, which a plain client finds closed before it registers.
/// Nor is any port given twice in a process: a test's server, not yet listening on one it was
/// given, would otherwise share it with another test's, which then found it in use or
/// answered the first one's requests (zirc-server's media tests, build 3494).
/// Another process could take one before Ergo does; [`Ergo::start_with`] tries again then.
///
/// # Panics
///
/// When the system has no free port to give.
#[must_use]
pub fn free_ports<const N: usize>() -> [u16; N] {
    static GIVEN: std::sync::Mutex<std::collections::BTreeSet<u16>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let mut given = GIVEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Those given before stay held while they're passed over, so the next is another.
    let mut passed = Vec::new();
    let held: [TcpListener; N] = std::array::from_fn(|_| {
        loop {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a free port");
            let port = listener.local_addr().expect("its address").port();
            if given.insert(port) {
                break listener;
            }
            passed.push(listener);
        }
    });
    held.map(|listener| listener.local_addr().expect("its address").port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_gives_a_port_twice() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..500 {
            let [port] = free_ports();
            assert!(seen.insert(port), "{port} given twice");
        }
    }

    #[test]
    fn picks_ports_that_differ() {
        for _ in 0..1000 {
            let [a, b] = free_ports();
            assert_ne!(a, b);
        }
    }
}
