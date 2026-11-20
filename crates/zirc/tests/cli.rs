//! The CLI against real daemons and a real Ergo, run the way people and agents run it.

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser as _;
use tempfile::TempDir;
use tokio::sync::mpsc;
use zirc::Cli;
use zirc_test_ergo::{Ergo, require_ergo};

const WAIT: Duration = Duration::from_secs(15);

/// Runs `zirc <args>` against `home` and returns what it printed.
async fn zirc(home: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut out = Vec::new();
    let mut all = vec!["zirc", "--home", home.to_str().unwrap()];
    all.extend(args);
    zirc::run(Cli::try_parse_from(all)?, &mut out).await?;
    Ok(String::from_utf8(out)?)
}

/// A zirc directory with config for `ergo`, short enough for a Unix socket path.
async fn home(ergo: &Ergo, nick: &str) -> TempDir {
    let home = tempfile::Builder::new()
        .prefix("zirc")
        .tempdir_in("/tmp")
        .unwrap();
    let port = ergo.port().to_string();
    let printed = zirc(
        home.path(),
        &[
            "init",
            "--server",
            "127.0.0.1",
            "--port",
            &port,
            "--no-tls",
            "--nick",
            nick,
        ],
    )
    .await
    .unwrap();
    assert!(printed.starts_with("wrote "), "{printed}");
    home
}

/// What a watch and the zircd started after the first stopped are doing, for the message of a
/// test that waited for the watch to say so in vain: which of them ended, how, and what
/// zircd's directory and status look like (these tests flaked on Linux CI with only "nothing
/// said in time" to go on).
async fn stuck<W: std::fmt::Debug, Z: std::fmt::Debug>(
    home: &Path,
    watch: &tokio::task::JoinHandle<W>,
    second: tokio::task::JoinHandle<Z>,
) -> String {
    let watch = if watch.is_finished() {
        "ended"
    } else {
        "running"
    };
    let second = if second.is_finished() {
        format!("ended with {:?}", second.await)
    } else {
        "running".to_owned()
    };
    let files: Vec<String> = std::fs::read_dir(home)
        .map(|dir| {
            dir.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    format!(
        "the watch is {watch}; the second zircd is {second}; status: {:?}; lock: {:?}; files: {files:?}",
        zirc(home, &["status"]).await,
        std::fs::read_to_string(home.join("zircd.lock")),
    )
}

async fn until_connected(home: &Path) {
    tokio::time::timeout(WAIT, async {
        while !zirc(home, &["status"])
            .await
            .is_ok_and(|status| status.starts_with("connected"))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("zircd didn't connect");
}

/// Until `home`'s zircd is in `channel` itself, as `zirc status` lists its channels: `#web`
/// is in none of `#web/agents`, `#web/landings` and `#web/ops`, which zircd joins too and
/// may be in first. A post to `#web` from a zircd still outside it is refused, and a test
/// that took one of those for it failed one run in five on CI (builds 2570 to 2834:
/// "catchup never had").
async fn until_in(home: &Path, channel: &str) {
    tokio::time::timeout(WAIT, async {
        while !zirc(home, &["status"])
            .await
            .unwrap()
            .split_whitespace()
            .any(|word| word == channel)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("zircd never joined {channel}"));
}

/// Until `home`'s zircd hears the server list a direct conversation with `nick`: from then
/// on a catch-up reads it. One begun before that leaves it out, rightly, and on CI the
/// next took 7 s and the one after outlasted the wait (build 3353), so the test asked for
/// what the server didn't have yet and never got to ask again.
async fn until_listed(home: &Path, nick: &str) {
    let listed = async {
        let list = zirc_core::ipc::Request::Conversations { seconds: 1800 };
        loop {
            let answer = zirc_core::ipc::request(&zirc_core::paths::socket(home), &list).await;
            if let Ok(zirc_core::ipc::Response::Conversations { targets }) = answer
                && targets.iter().any(|target| target == nick)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(WAIT, listed)
        .await
        .unwrap_or_else(|_| panic!("the server never listed a conversation with {nick}"));
}

/// How long `home`'s zircd takes to answer, now: its status, then a read of each of
/// `targets` as catch-up makes it. A catch-up is those one after another, so this says
/// whether one conversation is slow or zircd is, whatever it's asked.
async fn answer_times(home: &Path, targets: &[&str]) -> String {
    let socket = zirc_core::paths::socket(home);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let began = std::time::Instant::now();
    let _ = zirc_core::ipc::request(&socket, &zirc_core::ipc::Request::Status).await;
    let mut times = format!("status {:?}", began.elapsed());
    for target in targets {
        let read = zirc_core::ipc::Request::Read {
            target: (*target).to_owned(),
            limit: 200,
            since: Some(now.saturating_sub(1800)),
            until: None,
        };
        let began = std::time::Instant::now();
        let _ = zirc_core::ipc::request(&socket, &read).await;
        write!(times, ", {target} {:?}", began.elapsed()).unwrap();
    }
    times
}

/// Until `home`'s zircd lists a watch without a session: from then on it hears what's said.
/// Starting one is a task of its own, which a busy machine runs late.
async fn until_watching(home: &Path) {
    tokio::time::timeout(WAIT, async {
        while !zirc(home, &["status"])
            .await
            .is_ok_and(|status| status.contains("1 watch without a session"))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the watch never registered");
}

/// `zirc catchup --json` for `home` once it has a conversation with each of `targets`, and
/// what it printed: the server keeps a message a moment after it's sent. When one never
/// comes, it fails saying how each of `machines` stands and what it reads of the
/// conversation it names: a direct message from `ann-beta` went missing one run in 400,
/// with its post in `#web` there (builds 2864 and later), and only this says whether the
/// server ever kept it.
async fn catchup_with(
    home: &Path,
    targets: &[&str],
    machines: &[(&Path, &str)],
) -> (String, serde_json::Value) {
    let mut last = String::new();
    // How long each catch-up took, as it ended: one that outlasts WAIT is the only one
    // there was, and what it missed at its start is never looked for again.
    let tries = std::sync::Mutex::new(Vec::new());
    let found = tokio::time::timeout(WAIT, async {
        loop {
            let began = std::time::Instant::now();
            let printed = zirc(home, &["catchup", "--since", "30m", "--json"])
                .await
                .unwrap();
            tries.lock().unwrap().push(began.elapsed());
            let catchup: serde_json::Value = serde_json::from_str(&printed).unwrap();
            let has = |target: &&str| {
                catchup["conversations"]
                    .as_array()
                    .is_some_and(|all| all.iter().any(|it| it["target"] == *target))
            };
            if targets.iter().all(has) {
                return (printed, catchup);
            }
            last = printed;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    if let Ok(found) = found {
        return found;
    }
    let mut seen = String::new();
    for (machine, conversation) in machines {
        let status = zirc(machine, &["status"]).await;
        let read = zirc(machine, &["search", "--in", conversation]).await;
        // Which direct messages zircd hears the server list (`CHATHISTORY TARGETS`), which
        // catch-up reads but leaves out silently when it fails.
        let listed = zirc_core::ipc::request(
            &zirc_core::paths::socket(machine),
            &zirc_core::ipc::Request::Conversations { seconds: 1800 },
        )
        .await;
        // And the read catch-up makes of that conversation, as zircd answers it: with a
        // start, which `zirc search --in` reads without. Whether the message is missing
        // from it, or there and left out after, is what a failure hasn't said so far.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let since = zirc_core::ipc::request(
            &zirc_core::paths::socket(machine),
            &zirc_core::ipc::Request::Read {
                target: (*conversation).to_owned(),
                limit: 200,
                since: Some(now.saturating_sub(1800)),
                until: None,
            },
        )
        .await;
        let machine = machine.display();
        write!(
            seen,
            "\n{machine}: {status:?}\nits {conversation}: {read:?}\nits direct messages: {listed:?}\nits {conversation} since {}, at {now}: {since:?}",
            now.saturating_sub(1800)
        )
        .unwrap();
    }
    let tries = tries.into_inner().unwrap();
    let times = answer_times(home, &["#agents", "#ops", "#general"]).await;
    panic!(
        "catchup never had {targets:?} in {WAIT:?}, {} catch-ups ended, taking {tries:?}; zircd answers took: {times}; it had: {last}{seen}",
        tries.len()
    )
}

fn clone() -> TempDir {
    zirc_test_ergo::git::clone("https://github.com/acme/web.git")
}

/// What a writer is handed, a line at a time, as `zirc watch` prints.
struct Lines(mpsc::UnboundedSender<String>, Vec<u8>);

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.1.extend_from_slice(bytes);
        while let Some(end) = self.1.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.1.drain(..=end).collect();
            let _ = self
                .0
                .send(String::from_utf8_lossy(&line).trim_end().to_owned());
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The next event line a watch prints: zirc's hint lines under them are tested on their own.
async fn next_event(heard: &mut mpsc::UnboundedReceiver<String>) -> String {
    loop {
        let line = tokio::time::timeout(WAIT, heard.recv())
            .await
            .expect("nothing heard")
            .expect("watch ended");
        if !line.starts_with("  \u{21b3} hint (") {
            return line;
        }
    }
}

#[tokio::test]
async fn watches_a_repository_and_hears_another_machine() {
    let ergo = require_ergo!();
    let (alpha, beta) = (home(&ergo, "alpha").await, home(&ergo, "beta").await);
    let alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    let _beta_daemon = tokio::spawn(zircd::run(beta.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    until_connected(beta.path()).await;

    let (alpha_clone, beta_clone) = (clone(), clone());
    let added = zirc(
        alpha.path(),
        &["repo", "add", alpha_clone.path().to_str().unwrap()],
    )
    .await
    .unwrap();
    assert!(
        added.starts_with("watching ") && added.contains(" as web in #agents: "),
        "{added}"
    );
    zirc(
        beta.path(),
        &["repo", "add", beta_clone.path().to_str().unwrap()],
    )
    .await
    .unwrap();
    let listed = zirc(beta.path(), &["repo", "list", "--json"])
        .await
        .unwrap();
    assert!(listed.contains(r##""scope":"#web""##), "{listed}");
    one_organisation_says_nothing_of_it(&listed, beta.path(), ergo.port()).await;

    // Beta watches, as an agent's monitor would: it hears what concerns it, as posts naming it.
    let (lines, mut heard) = mpsc::unbounded_channel();
    let beta_home = beta.path().to_owned();
    tokio::spawn(async move {
        let cli = Cli::try_parse_from(["zirc", "--home", beta_home.to_str().unwrap(), "watch"]);
        let _ = zirc::run(cli.unwrap(), &mut Lines(lines, Vec::new())).await;
    });
    until_in(beta.path(), "#agents").await;
    // The watch registered before anything is said.
    until_watching(beta.path()).await;

    a_session_without_a_repository_is_told_how_to_add_one(beta.path()).await;

    zirc(
        alpha.path(),
        &[
            "post",
            "--repo",
            alpha_clone.path().to_str().unwrap(),
            "--session",
            "cc-7",
            // A machine's isn't a session's name, so a post to one is no note.
            "beta: reworking the router",
        ],
    )
    .await
    .unwrap();
    let mut next = async || next_event(&mut heard).await;
    assert!((next().await).starts_with("#agents alpha/cc-7: beta: reworking the router [id "));

    // Alpha's edit alone is left out of a quiet watch; beta changing the same file is what
    // beta hears about.
    std::fs::write(alpha_clone.path().join("router.ts"), "export {}\n").unwrap();
    tokio::time::timeout(WAIT, async {
        while !zirc(beta.path(), &["who"])
            .await
            .unwrap()
            .contains("    main: router.ts")
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("beta never saw alpha's working set");
    std::fs::write(beta_clone.path().join("router.ts"), "export { b }\n").unwrap();
    assert_eq!(
        next().await,
        "#web overlap: alpha is also changing router.ts"
    );
    // Each machine's changes are by branch.
    assert_eq!(
        zirc(beta.path(), &["who"]).await.unwrap(),
        "#web\n  alpha\n    main: router.ts\n  beta\n    main: router.ts\n"
    );

    // Stopping goes through the socket too.
    assert_eq!(
        zirc(alpha.path(), &["daemon", "stop"]).await.unwrap(),
        "zircd stopped\n"
    );
    tokio::time::timeout(WAIT, alpha_daemon)
        .await
        .expect("zircd didn't stop")
        .unwrap()
        .unwrap();
    let err = zirc(alpha.path(), &["status"]).await.unwrap_err();
    assert!(err.to_string().contains("zircd isn't running"), "{err}");
}

/// Signed in to one organisation, nothing says which: as before several organisations.
async fn one_organisation_says_nothing_of_it(listed: &str, home: &Path, port: u16) {
    assert!(!listed.contains(r#""org""#), "{listed}");
    let status = zirc(home, &["status"]).await.unwrap();
    assert!(
        status.starts_with(&format!(
            "connected to 127.0.0.1:{port}\nnick beta\nchannels "
        )),
        "{status}"
    );
}

/// Someone signed in to one organisation sees no change: no `--org` in the help.
#[test]
fn says_nothing_of_organisations_in_the_help() {
    use clap::CommandFactory as _;
    let mut command = Cli::command();
    for args in [
        &["repo", "add"][..],
        &["dm"],
        &["ask"],
        &["reply"],
        &["search"],
        &["trim"],
        &["who"],
    ] {
        let mut sub = &mut command;
        for arg in args {
            sub = sub.find_subcommand_mut(arg).unwrap();
        }
        let help = sub.render_long_help().to_string();
        assert!(!help.contains("--org <"), "{args:?}: {help}");
    }
}

/// `zirc state --help` points at `zirc wait state` for waiting, and keeps --check for CI
/// scripts: agents polled `zirc state --check` in a loop while it said "for a script to stop on".
#[test]
fn state_help_says_to_wait_for_it_not_to_poll() {
    use clap::CommandFactory as _;
    let mut command = Cli::command();
    let help = command
        .find_subcommand_mut("state")
        .unwrap()
        .render_long_help()
        .to_string();
    assert!(
        help.contains("zirc wait state master ok in the background"),
        "{help}"
    );
    assert!(help.contains("one-off check in a CI script"), "{help}");
    assert!(!help.contains("for a script to stop on"), "{help}");
}

#[tokio::test]
async fn starts_zircd_detached_and_stops_it() {
    let ergo = require_ergo!();
    let Some(zircd) = std::env::var_os("ZIRC_ZIRCD").map(PathBuf::from) else {
        eprintln!("ZIRC_ZIRCD isn't set: run this with bazel test");
        return;
    };
    let zircd = std::env::current_dir().unwrap().join(zircd);
    let home = home(&ergo, "gamma").await;
    let start = ["daemon", "start", "--zircd", zircd.to_str().unwrap()];

    let started = zirc(home.path(), &start).await.unwrap();
    assert!(started.starts_with("zircd is running"), "{started}");
    until_connected(home.path()).await;
    assert_eq!(
        zirc(home.path(), &start).await.unwrap(),
        "zircd is already running\n"
    );
    assert_eq!(
        zirc(home.path(), &["daemon", "stop"]).await.unwrap(),
        "zircd stopped\n"
    );
    assert!(!zirc_core::paths::socket(home.path()).exists());
}

#[tokio::test]
async fn explains_a_daemon_that_cannot_start() {
    let Some(zircd) = std::env::var_os("ZIRC_ZIRCD").map(PathBuf::from) else {
        return;
    };
    let zircd = std::env::current_dir().unwrap().join(zircd);
    // No config.toml: zircd exits at once, and zirc says why.
    let home = tempfile::Builder::new()
        .prefix("zirc")
        .tempdir_in("/tmp")
        .unwrap();
    let err = zirc(
        home.path(),
        &["daemon", "start", "--zircd", zircd.to_str().unwrap()],
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("zirc init"), "{err:#}");
}

/// A session watching before it's added a repository is told how to.
async fn a_session_without_a_repository_is_told_how_to_add_one(home: &Path) {
    let (told, mut stranger) = mpsc::unbounded_channel();
    let stranger_home = home.to_owned();
    tokio::spawn(async move {
        let cli = Cli::try_parse_from([
            "zirc",
            "--home",
            stranger_home.to_str().unwrap(),
            "watch",
            "--session",
            "cc-stranger",
        ]);
        let _ = zirc::run(cli.unwrap(), &mut Lines(told, Vec::new())).await;
    });
    let first = tokio::time::timeout(WAIT, stranger.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        first.contains("run `zirc repo add --session cc-stranger`"),
        "{first}"
    );
}

#[tokio::test]
async fn a_team_emits_its_own_events_and_a_filter_picks_them_out() {
    let ergo = require_ergo!();
    let (alpha, beta) = (home(&ergo, "alpha").await, home(&ergo, "beta").await);
    let _alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    let _beta_daemon = tokio::spawn(zircd::run(beta.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    until_connected(beta.path()).await;
    let status = zirc(beta.path(), &["status", "--json"]).await.unwrap();
    assert!(status.contains(r#""protocol":1"#), "{status}");

    let (alpha_clone, beta_clone) = (clone(), clone());
    let alpha_repo = alpha_clone.path().to_str().unwrap();
    zirc(alpha.path(), &["repo", "add", alpha_repo])
        .await
        .unwrap();
    zirc(
        beta.path(),
        &["repo", "add", beta_clone.path().to_str().unwrap()],
    )
    .await
    .unwrap();

    // Beta wants only deploys to production, and only their versions.
    let (lines, mut heard) = mpsc::unbounded_channel();
    let beta_home = beta.path().to_owned();
    tokio::spawn(async move {
        let cli = Cli::try_parse_from([
            "zirc",
            "--home",
            beta_home.to_str().unwrap(),
            "watch",
            "--filter",
            r#".message.event | select(.kind == "x-acme/deploy" and .data.env == "prod") | .data.version"#,
        ]);
        let _ = zirc::run(cli.unwrap(), &mut Lines(lines, Vec::new())).await;
    });
    until_in(beta.path(), "#agents").await;
    until_watching(beta.path()).await;

    let emit = async |json: &str, text: &str| {
        zirc(
            alpha.path(),
            &[
                "emit",
                "x-acme/deploy",
                "--repo",
                alpha_repo,
                "--json",
                json,
                text,
            ],
        )
        .await
    };
    assert_eq!(
        emit(
            r#"{"env":"staging","version":"1.4.1"}"#,
            "deployed 1.4.1 to staging"
        )
        .await
        .unwrap(),
        "sent to #agents\n"
    );
    zirc(
        alpha.path(),
        &["post", "--repo", alpha_repo, "deploying now"],
    )
    .await
    .unwrap();
    emit(
        r#"{"env":"prod","version":"1.4.2"}"#,
        "deployed 1.4.2 to prod",
    )
    .await
    .unwrap();
    let first = tokio::time::timeout(WAIT, heard.recv())
        .await
        .expect("nothing heard")
        .expect("watch ended");
    assert_eq!(first, r#""1.4.2""#);

    // Kinds are namespaced, and an event that won't fit isn't sent in pieces.
    let err = zirc(
        alpha.path(),
        &["emit", "deploy", "--repo", alpha_repo, "deployed"],
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("starts with x-"), "{err}");
    let big = format!(r#"{{"log":"{}"}}"#, "x".repeat(20_000));
    let err = emit(&big, "a long one").await.unwrap_err();
    assert!(err.to_string().contains("carry less data"), "{err}");
    let err = zirc(beta.path(), &["watch", "--filter", "select("])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not a jq filter"), "{err}");
}

#[tokio::test]
async fn a_watch_carries_on_through_zircd_stopping_and_starting_again() {
    let ergo = require_ergo!();
    let home = home(&ergo, "delta").await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let first = tokio::spawn(zircd::run(home.path().to_owned(), async {
        let _ = stopped.await;
    }));
    until_connected(home.path()).await;

    let (told, mut heard) = mpsc::unbounded_channel();
    let watch_home = home.path().to_owned();
    let watch = tokio::spawn(async move {
        let cli = Cli::try_parse_from(["zirc", "--home", watch_home.to_str().unwrap(), "watch"]);
        zirc::run(cli.unwrap(), &mut Lines(told, Vec::new())).await
    });
    tokio::time::timeout(WAIT, async {
        while !zirc(home.path(), &["status"])
            .await
            .is_ok_and(|status| status.contains("watching "))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the watch never started");

    // zircd stops without a word, as one that crashed, and starts again.
    stop.send(()).unwrap();
    first.await.unwrap().unwrap();
    let second = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    let Ok(line) = tokio::time::timeout(WAIT, heard.recv()).await else {
        panic!(
            "nothing said in time: {}",
            stuck(home.path(), &watch, second).await
        )
    };
    let line = line.expect("watch ended");
    assert_eq!(line, "zircd restarted; watching again");
    assert!(!watch.is_finished());
}

#[tokio::test]
async fn a_watch_carries_on_through_zircd_restarting_for_an_update() {
    let ergo = require_ergo!();
    let home = home(&ergo, "zeta").await;
    let (updated, update) = tokio::sync::oneshot::channel();
    let hooks = zircd::Hooks {
        updated: Some(update),
        ..zircd::Hooks::default()
    };
    let first = tokio::spawn(zircd::run_with(
        home.path().to_owned(),
        std::future::pending(),
        hooks,
    ));
    until_connected(home.path()).await;

    let (told, mut heard) = mpsc::unbounded_channel();
    let watch_home = home.path().to_owned();
    let watch = tokio::spawn(async move {
        let cli = Cli::try_parse_from(["zirc", "--home", watch_home.to_str().unwrap(), "watch"]);
        zirc::run(cli.unwrap(), &mut Lines(told, Vec::new())).await
    });
    tokio::time::timeout(WAIT, async {
        while !zirc(home.path(), &["status"])
            .await
            .is_ok_and(|status| status.contains("watching "))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the watch never started");

    // A release was installed: zircd ends to restart, and its main starts the new binary.
    updated.send(zirc_core::update::Version(99, 0, 0)).unwrap();
    assert_eq!(first.await.unwrap().unwrap(), zircd::Ended::Restart);
    let second = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    let Ok(line) = tokio::time::timeout(WAIT, heard.recv()).await else {
        panic!(
            "nothing said in time: {}",
            stuck(home.path(), &watch, second).await
        )
    };
    let line = line.expect("watch ended");
    assert_eq!(line, "zircd restarted; watching again");
    assert!(!watch.is_finished());
}

#[tokio::test]
async fn a_watch_for_a_while_ends_quietly() {
    let ergo = require_ergo!();
    let home = home(&ergo, "epsilon").await;
    let _daemon = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    until_connected(home.path()).await;
    let printed = tokio::time::timeout(WAIT, zirc(home.path(), &["watch", "--for", "0"]))
        .await
        .expect("the watch ended")
        .unwrap();
    // A last line would wake the session watching only to say nothing.
    assert_eq!(printed, "");
}

/// `zirc watch --once` says nothing until something concerns the session, then prints it
/// and ends, for an agent to start again after each wake.
#[tokio::test]
async fn a_watch_once_ends_after_the_first_thing_that_concerns_it() {
    let ergo = require_ergo!();
    let (alpha, beta) = (home(&ergo, "alpha").await, home(&ergo, "beta").await);
    let _alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    let _beta_daemon = tokio::spawn(zircd::run(beta.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    until_connected(beta.path()).await;
    let (alpha_clone, beta_clone) = (clone(), clone());
    let beta_repo = beta_clone.path().to_str().unwrap();
    for (home, repo) in [(&alpha, alpha_clone.path()), (&beta, beta_clone.path())] {
        zirc(home.path(), &["repo", "add", repo.to_str().unwrap()])
            .await
            .unwrap();
        until_in(home.path(), "#agents").await;
    }

    let alpha_home = alpha.path().to_owned();
    let watch = tokio::spawn(async move { zirc(&alpha_home, &["watch", "--once"]).await });
    until_watching(alpha.path()).await;
    // A post that doesn't concern alpha doesn't end it: the one after, which does, is all
    // it prints.
    zirc(
        beta.path(),
        &["post", "--repo", beta_repo, "lunch, anyone?"],
    )
    .await
    .unwrap();

    zirc(
        beta.path(),
        &["post", "--repo", beta_repo, "alpha: are you there?"],
    )
    .await
    .unwrap();
    let printed = tokio::time::timeout(WAIT, watch)
        .await
        .expect("the watch ended")
        .unwrap()
        .unwrap();
    // The line, and zirc's hint under it.
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines.len(), 2, "{printed}");
    assert!(
        lines[1].starts_with("  \u{21b3} hint (default): "),
        "{printed}"
    );
    assert!(lines[0].starts_with("#agents beta"), "{printed}");
    assert!(lines[0].contains("are you there?"), "{printed}");
}

/// Until `zirc <args>` run for `home` prints `want`.
async fn until_printed(home: &Path, args: &[&str], want: &str) {
    tokio::time::timeout(WAIT, async {
        while !zirc(home, args).await.is_ok_and(|said| said.contains(want)) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("zirc {} never said {want}", args.join(" ")));
}

/// Says how master stands in `repo`, as CI's `zirc state set` run in it does, through
/// `home`'s zircd.
async fn set_master(home: &Path, repo: &Path, level: zirc_proto::state::Level) {
    let request = zirc_core::ipc::Request::SetState {
        repo: repo.to_owned(),
        session: "ci".into(),
        name: "master".into(),
        level,
        why: None,
        owner: None,
        link: None,
        until: None,
    };
    let said = zirc_core::ipc::request(&zirc_core::paths::socket(home), &request).await;
    assert!(
        matches!(said, Ok(zirc_core::ipc::Response::Channel { .. })),
        "{said:?}"
    );
}

/// `zirc wait-for --state master` while master is broken: the watch stays quiet while it's
/// broken again, and wakes once, with its hint, when it's fine. No loop on `zirc state --check`.
#[tokio::test]
async fn a_watch_wakes_when_a_state_it_waits_on_changes() {
    use zirc_proto::state::Level;
    let ergo = require_ergo!();
    let (alpha, beta) = (home(&ergo, "alpha").await, home(&ergo, "beta").await);
    let _alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    let _beta_daemon = tokio::spawn(zircd::run(beta.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    until_connected(beta.path()).await;
    let (alpha_clone, beta_clone) = (clone(), clone());
    // Both watch a repository, so both are in #ops, where states are said.
    for (home, repo) in [(&alpha, alpha_clone.path()), (&beta, beta_clone.path())] {
        let repo = repo.to_str().unwrap();
        zirc(home.path(), &["repo", "add", "--session", "cc-a", repo])
            .await
            .unwrap();
    }
    set_master(beta.path(), beta_clone.path(), Level::Broken).await;
    // alpha reads it broken, and waits on it changing.
    until_printed(alpha.path(), &["state", "master"], "master is broken").await;
    // Under it, how to be woken when it changes, for the session asking.
    let shown = zirc(alpha.path(), &["state", "master"]).await.unwrap();
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines.len(), 2, "{shown}");
    assert_eq!(
        lines[1],
        "  \u{21b3} to be woken when it's fine: zirc wait state master ok, in the background"
    );
    let wait = ["wait-for", "--session", "cc-a", "--state", "master"];
    let waiting = zirc(alpha.path(), &wait).await.unwrap();
    assert!(
        waiting.starts_with("waiting on state master: "),
        "{waiting}"
    );
    let bad = zirc(alpha.path(), &[&wait[..4], &["Master"]].concat()).await;
    assert!(bad.is_err_and(|err| err.to_string().contains("lowercase")));

    // A watch says it as it starts, so the wait shows where the session's watch does.
    let brief = ["watch", "--session", "cc-a", "--for", "0"];
    let starting = zirc(alpha.path(), &brief).await.unwrap();
    assert!(
        starting.lines().any(|line| line == waiting.trim()),
        "{starting}"
    );

    let alpha_home = alpha.path().to_owned();
    let once = ["watch", "--once", "--session", "cc-a"];
    let watch = tokio::spawn(async move { zirc(&alpha_home, &once).await });
    until_printed(alpha.path(), &["status"], "sessions cc-a").await;
    set_master(beta.path(), beta_clone.path(), Level::Broken).await;
    set_master(beta.path(), beta_clone.path(), Level::Ok).await;
    let printed = tokio::time::timeout(WAIT, watch)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines.len(), 2, "{printed}");
    assert!(lines[0].contains("master is ok"), "{printed}");
    assert!(
        lines[1].starts_with("  \u{21b3} hint (default): It's fine again"),
        "{printed}"
    );
    // The wait is over.
    let left = zirc(alpha.path(), &wait[..3]).await.unwrap();
    assert_eq!(left.trim(), "waiting on nothing");
}

/// The passphrase of CI's account, `zirc-ci`, in a test that registers it.
const CI_PASSPHRASE: &str = "ci-passphrase";

/// Has CI say `text` with `data` in `#web`, as an event of `kind`: from its own account,
/// `zirc-ci`, registered by the test, as zirc.dev posts it, since zircd takes a landing's or
/// a deploy's event as CI's only from an integration's account.
fn ci_says(ergo: &zirc_test_ergo::Ergo, kind: &str, text: &str, data: serde_json::Value) {
    let event = zirc_proto::Event::Custom {
        kind: kind.into(),
        data,
        text: text.into(),
    };
    let message = zirc_proto::Message {
        repo: Some("web".into()),
        ..zirc_proto::Message::new("ci", event)
    };
    let line = zirc_proto::encode(&message).unwrap();
    zirc_test_ergo::signin::say_as(ergo.port(), "zirc-ci", CI_PASSPHRASE, "#agents", &line)
        .expect("CI said it");
}

/// Where no CI has said a landing yet, none would say this one: the wait says so at once and
/// waits for nothing (an agent in the evals' PN1 probe waited its whole run), unless it's
/// told to wait anyway, as for a CI's first.
#[tokio::test]
async fn a_wait_for_a_landing_no_ci_would_say_is_refused() {
    use serde_json::json;
    let ergo = require_ergo!();
    let alpha = home(&ergo, "alpha").await;
    let _alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    let alpha_clone = clone();
    let here = alpha_clone.path().to_str().unwrap().to_owned();
    zirc(alpha.path(), &["repo", "add", "--session", "cc-a", &here])
        .await
        .unwrap();
    until_in(alpha.path(), "#agents").await;
    let landing = zirc_proto::ci::LANDING_KIND;
    zirc_test_ergo::signin::register(ergo.port(), "zirc-ci", CI_PASSPHRASE).expect("CI's account");
    let refused = zirc(alpha.path(), &["wait", "landed", "login", "--repo", &here]).await;
    assert!(
        refused.is_err_and(|err| err.to_string().contains("no CI has said a landing")),
        "a wait no CI would end"
    );
    let alpha_home = alpha.path().to_owned();
    let repo = here.clone();
    let waiting = tokio::spawn(async move {
        let wait = ["wait", "landed", "first", "--anyway", "--repo", &repo];
        zirc(&alpha_home, &wait).await
    });
    until_watching(alpha.path()).await;
    let first = "cc-2: landed first as 0f0f0f0 (build 6)";
    let data = json!({"branch": "first", "build": "6", "commit": "0f0f0f0f0f", "state": "passed"});
    ci_says(&ergo, landing, first, data);
    let waited = tokio::time::timeout(WAIT, waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(waited.contains(first), "{waited}");
}

/// `zirc wait` is a command that ends when CI says what it waits for happened: at once for
/// what was said before it started, read back from the channel, and as failed when CI says
/// it failed. No loop on a channel's history.
#[tokio::test]
async fn a_wait_ends_when_ci_says_it_happened_or_failed() {
    use serde_json::json;
    use zirc_proto::state::Level;
    let ergo = require_ergo!();
    let (alpha, beta) = (home(&ergo, "alpha").await, home(&ergo, "beta").await);
    let _alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    let _beta_daemon = tokio::spawn(zircd::run(beta.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    until_connected(beta.path()).await;
    let (alpha_clone, beta_clone) = (clone(), clone());
    for (home, repo) in [(&alpha, alpha_clone.path()), (&beta, beta_clone.path())] {
        let repo = repo.to_str().unwrap();
        zirc(home.path(), &["repo", "add", "--session", "cc-a", repo])
            .await
            .unwrap();
    }
    // A wait reads back the channels zircd has joined, and hears what's said in them: one
    // started before alpha was in #web read nothing back and never heard CI, which had
    // spoken already, and waited till Bazel's 300s (build 3346).
    until_in(alpha.path(), "#agents").await;
    let here = alpha_clone.path().to_str().unwrap().to_owned();
    let landing = zirc_proto::ci::LANDING_KIND;
    let deploy = zirc_proto::ci::DEPLOY_KIND;
    zirc_test_ergo::signin::register(ergo.port(), "zirc-ci", CI_PASSPHRASE).expect("CI's account");

    // Said before the wait started: it reads it back, and ends at once.
    let landed = "cc-2: landed login as 1a2b3c4 (build 7)";
    let data = json!({"branch": "login", "build": "7", "commit": "1a2b3c4d5e", "state": "passed"});
    ci_says(&ergo, landing, landed, data);
    let waited = zirc(alpha.path(), &["wait", "landed", "login", "--repo", &here])
        .await
        .unwrap();
    assert!(waited.contains(landed), "{waited}");
    assert_eq!(waited.lines().count(), 1, "{waited}");

    // Said while it waits: the deploy of the commit it landed as, with the app asked for.
    let alpha_home = alpha.path().to_owned();
    let repo = here.clone();
    let waiting = tokio::spawn(async move {
        let wait = ["wait", "deployed", "1a2b3c4", "--app", "controller"];
        zirc(&alpha_home, &[&wait[..], &["--repo", &repo]].concat()).await
    });
    until_watching(alpha.path()).await;
    let data = json!({"branch": "login", "build": "7", "commit": "1a2b3c4d5e", "state": "passed"});
    let edge = "cc-2: deployed to production: the edge at 1a2b3c4 \u{2713}";
    let deployed = "cc-2: deployed to production: controller at 1a2b3c4 \u{2713}";
    ci_says(&ergo, deploy, edge, data.clone());
    ci_says(&ergo, deploy, deployed, data);
    let waited = tokio::time::timeout(WAIT, waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(waited.contains(deployed), "{waited}");
    assert!(!waited.contains("the edge"), "{waited}");

    // A build that failed ends its wait as failed, so nothing chained after it runs.
    let failed = "cc-2: build 8 failed: //web:unit";
    let data = json!({"branch": "signup", "build": "8", "state": "failed"});
    ci_says(&ergo, landing, failed, data);
    let waited = zirc(
        alpha.path(),
        &["wait", "landed", "build-8", "--repo", &here],
    )
    .await;
    assert!(
        waited.is_err_and(|err| err.to_string().contains("CI says it failed")),
        "a failed build"
    );

    // A state never said is fine, so a wait for it to be ok ends at once; one for a broken
    // master ends when it's said to be fine.
    let fine = ["wait", "state", "master", "ok", "--repo", &here];
    let waited = zirc(alpha.path(), &fine).await.unwrap();
    assert_eq!(waited.trim(), "nothing says otherwise: it's fine");
    set_master(beta.path(), beta_clone.path(), Level::Broken).await;
    until_printed(alpha.path(), &["state", "master"], "master is broken").await;
    let alpha_home = alpha.path().to_owned();
    let repo = here.clone();
    let waiting = tokio::spawn(async move {
        zirc(
            &alpha_home,
            &["wait", "state", "master", "ok", "--repo", &repo],
        )
        .await
    });
    until_watching(alpha.path()).await;
    set_master(beta.path(), beta_clone.path(), Level::Ok).await;
    let waited = tokio::time::timeout(WAIT, waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(waited.contains("master is ok"), "{waited}");
}

/// A wait for a branch landed again after it failed waits for that landing: the earlier
/// build's failure, read back, doesn't end it (an agent's in an eval took it for this one).
#[tokio::test]
async fn a_wait_for_a_branch_landed_again_waits_past_its_earlier_failure() {
    use serde_json::json;
    let ergo = require_ergo!();
    let alpha = home(&ergo, "alpha").await;
    let _daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    let clone = clone();
    let here = clone.path().to_str().unwrap().to_owned();
    zirc(alpha.path(), &["repo", "add", "--session", "cc-a", &here])
        .await
        .unwrap();
    until_in(alpha.path(), "#agents").await;
    let landing = zirc_proto::ci::LANDING_KIND;
    zirc_test_ergo::signin::register(ergo.port(), "zirc-ci", CI_PASSPHRASE).expect("CI's account");
    let failed = json!({"branch": "money", "build": "1", "state": "failed"});
    ci_says(
        &ergo,
        landing,
        "cc-a: build 1 failed: money in test",
        failed,
    );
    let started = json!({"branch": "money", "build": "2", "state": "running"});
    ci_says(&ergo, landing, "landing started", started);

    let alpha_home = alpha.path().to_owned();
    let repo = here.clone();
    let waiting = tokio::spawn(async move {
        zirc(&alpha_home, &["wait", "landed", "money", "--repo", &repo]).await
    });
    until_watching(alpha.path()).await;
    let landed = "cc-a: landed money as 7d6db44 (build 2)";
    let data = json!({"branch": "money", "build": "2", "state": "passed"});
    ci_says(&ergo, landing, landed, data);
    let waited = tokio::time::timeout(WAIT, waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(waited.contains(landed), "{waited}");
}

#[tokio::test]
async fn says_who_may_query_this_machines_agents_and_warns_before_anyone() {
    let ergo = require_ergo!();
    let home = home(&ergo, "alpha").await;
    let _daemon = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
    until_connected(home.path()).await;

    let said = zirc(home.path(), &["agents", "queries"]).await.unwrap();
    assert_eq!(said, "queries to this machine's agents: mine\n");
    // Anyone only by a person at a terminal, never an agent or script, as here.
    let refused = zirc(home.path(), &["agents", "queries", "anyone", "--yes"])
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("only a person at a terminal"),
        "{refused:#}"
    );
    let said = zirc(home.path(), &["agents", "queries", "off"])
        .await
        .unwrap();
    assert_eq!(said, "queries to this machine's agents: off\n");

    // A session asks another here, which hears it on its watch.
    zirc(home.path(), &["agents", "queries", "mine"])
        .await
        .unwrap();
    let asked = zirc(
        home.path(),
        &["ask", "--session", "cc-1", "--to", "alpha", "busy?"],
    )
    .await
    .unwrap();
    assert!(asked.starts_with("asked alpha [q"), "{asked}");
}

#[tokio::test]
async fn catches_up_on_what_people_said_not_on_zircds_events() {
    let ergo = require_ergo!();
    let (alpha, beta) = (
        home(&ergo, "ann-alpha").await,
        home(&ergo, "ann-beta").await,
    );
    let _alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    let _beta_daemon = tokio::spawn(zircd::run(beta.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    until_connected(beta.path()).await;
    let (alpha_clone, beta_clone) = (clone(), clone());
    for (home, clone) in [(&alpha, &alpha_clone), (&beta, &beta_clone)] {
        zirc(
            home.path(),
            &["repo", "add", clone.path().to_str().unwrap()],
        )
        .await
        .unwrap();
    }
    // Both in #web: beta's post from outside it is refused, and never in its history.
    for home in [&alpha, &beta] {
        until_in(home.path(), "#agents").await;
    }

    // Beta's working set, which zircd announces in #web, and a post naming alpha.
    std::fs::write(beta_clone.path().join("README.md"), "beta's take\n").unwrap();
    zirc(
        beta.path(),
        &[
            "post",
            "--session",
            "cc-b",
            "--repo",
            beta_clone.path().to_str().unwrap(),
            "ann-alpha: can you look at the login page?",
        ],
    )
    .await
    .unwrap();
    // And a question to alpha's agents, which the server keeps as a direct message.
    zirc(
        beta.path(),
        &[
            "ask",
            "--session",
            "cc-b",
            "--to",
            "ann-alpha",
            "is the login page yours?",
        ],
    )
    .await
    .unwrap();
    // Once the server has both: the post in #agents and the question as a direct message.
    until_listed(alpha.path(), "ann-beta").await;
    let machines = [(alpha.path(), "ann-beta"), (beta.path(), "ann-alpha")];
    let (printed, catchup) = catchup_with(alpha.path(), &["#agents", "ann-beta"], &machines).await;
    let web = catchup["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|it| it["target"] == "#agents")
        .unwrap_or_else(|| panic!("no #agents: {printed}"));
    let texts: Vec<&str> = web["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        ["ann-alpha: can you look at the login page?"],
        "{printed}"
    );
    assert_eq!(
        catchup["mentions"][0]["text"],
        "ann-alpha: can you look at the login page?"
    );
    // zirc search shows the same post, and none of zircd's working sets: from the machine's
    // own server, as it isn't signed in through zirc.dev.
    let read = zirc(alpha.path(), &["search", "--in", "#agents"])
        .await
        .unwrap();
    assert!(read.contains("can you look at the login page?"), "{read}");
    assert!(!read.contains("working on"), "{read}");

    // Direct messages come first.
    let first = &catchup["conversations"][0];
    assert_eq!(first["target"], "ann-beta", "{printed}");
    assert_eq!(first["direct"], true);
    assert!(
        first["messages"][0]["text"]
            .as_str()
            .unwrap()
            .contains("is the login page yours?"),
        "{printed}"
    );
}

#[tokio::test]
async fn a_watch_prints_everything_said_in_a_channel_it_keeps_an_eye_on() {
    let ergo = require_ergo!();
    let (alpha, beta) = (home(&ergo, "alpha").await, home(&ergo, "beta").await);
    let _alpha_daemon = tokio::spawn(zircd::run(alpha.path().to_owned(), std::future::pending()));
    let _beta_daemon = tokio::spawn(zircd::run(beta.path().to_owned(), std::future::pending()));
    until_connected(alpha.path()).await;
    until_connected(beta.path()).await;
    let (alpha_clone, beta_clone) = (clone(), clone());
    for (home, clone, session) in [(&alpha, &alpha_clone, "cc-a"), (&beta, &beta_clone, "cc-b")] {
        let path = clone.path().to_str().unwrap();
        zirc(home.path(), &["repo", "add", "--session", session, path])
            .await
            .unwrap();
    }
    until_in(alpha.path(), "#general").await;

    // A channel this machine isn't in is refused, saying how to join it.
    let refused = zirc(alpha.path(), &["watch", "--channel", "#deploys"])
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("zirc join --org deploys"), "{refused}");

    let (lines, mut heard) = mpsc::unbounded_channel();
    let alpha_home = alpha.path().to_owned();
    tokio::spawn(async move {
        let cli = Cli::try_parse_from([
            "zirc",
            "--home",
            alpha_home.to_str().unwrap(),
            "watch",
            "--session",
            "cc-a",
            "--channel",
            "#general",
        ]);
        let _ = zirc::run(cli.unwrap(), &mut Lines(lines, Vec::new())).await;
    });
    tokio::time::timeout(WAIT, async {
        while !zirc(alpha.path(), &["status"])
            .await
            .unwrap()
            .contains("cc-a")
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the watch");

    // Chatter that names nobody, in #general: printed, as asked.
    zirc(
        beta.path(),
        &[
            "post",
            "--session",
            "cc-b",
            "--repo",
            beta_clone.path().to_str().unwrap(),
            "--to",
            "org",
            "deploying web to staging",
        ],
    )
    .await
    .unwrap();
    let line = tokio::time::timeout(WAIT, heard.recv())
        .await
        .expect("nothing printed")
        .unwrap();
    assert!(
        line.starts_with("#general beta/cc-b: deploying web to staging [id "),
        "{line}"
    );
}
