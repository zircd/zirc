//! The terminals zirc.dev shows (fixtures/home/terminals.json): the morning of the team on zirc.dev's home page (fixtures/home), run with
//! the real CLI and zircd against a real Ergo, as Henrik's and Raj's machines would run it, then
//! made the same each run: the server's address, the scratch directories and message ids
//! read as the team's. The test fails when what zirc prints has changed, and leaves what it
//! printed now in its outputs: `bazel run //fixtures/home:terminals` copies that in.

use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use clap::Parser as _;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::mpsc;
use zirc::Cli;
use zirc_test_ergo::{Ergo, require_ergo};

const WAIT: Duration = Duration::from_secs(15);

/// How long a watch is given to say nothing more after what it was waited for.
const QUIET: Duration = Duration::from_millis(800);

/// One machine of the team's: its zirc directory and its clone of zirc/server.
struct Machine {
    home: TempDir,
    repo: TempDir,
}

impl Machine {
    /// `owner`'s machine `nick`, whose decisions are theirs, as `zirc login` sets it up.
    async fn new(ergo: &Ergo, nick: &str, owner: &str) -> Self {
        let home = tempfile::Builder::new()
            .prefix("zirc")
            .tempdir_in("/tmp")
            .unwrap();
        let port = ergo.port().to_string();
        let args = [
            "init",
            "--server",
            "127.0.0.1",
            "--port",
            &port,
            "--no-tls",
            "--nick",
            nick,
        ];
        run(home.path(), &args).await.unwrap();
        let config = home.path().join("config.toml");
        // At the top, before any table.
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, format!("owner = \"{owner}\"\n{text}")).unwrap();
        let repo = zirc_test_ergo::git::clone("https://github.com/zirc/server.git");
        Self { home, repo }
    }

    /// `zirc <args>` in the machine's clone, as a person types it, less what this machine's
    /// load adds: `zirc status` says when zircd was busy ([`zirc::BUSY`]), which a loaded CI
    /// machine made it say in the home page's terminals (build 3413).
    async fn zirc(&self, args: &[&str]) -> String {
        std::env::set_current_dir(self.repo.path()).unwrap();
        match run(self.home.path(), args).await {
            Ok(out) => out
                .split_inclusive('\n')
                .filter(|line| !line.starts_with(zirc::BUSY))
                .collect(),
            Err(err) => format!("zirc: {err:#}\n"),
        }
    }

    fn edit(&self, path: &str, text: &str) {
        let file = self.repo.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

async fn run(home: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut out = Vec::new();
    let mut all = vec!["zirc", "--home", home.to_str().unwrap()];
    all.extend(args);
    zirc::run(Cli::try_parse_from(all)?, &mut out).await?;
    Ok(String::from_utf8(out)?)
}

/// Henrik's `zirc who`, and, when it lacks Raj's set, as it did on CI now and then (flaky-73,
/// flaky-75), what each machine knew then and a moment later: which lost it, and for how long.
async fn who_noting_a_missing_set(henrik: &Machine, raj: &Machine) -> String {
    let who = henrik.zirc(&["who"]).await;
    if !who.contains("cc-91b7 is changing tests/e2e/harness.rs") {
        eprintln!("henrik's who lacks raj's set:\n{who}");
        eprintln!(
            "henrik's, as JSON:\n{}",
            henrik.zirc(&["who", "--json"]).await
        );
        eprintln!("raj's own:\n{}", raj.zirc(&["who", "--json"]).await);
        tokio::time::sleep(Duration::from_secs(1)).await;
        eprintln!("henrik's, a second later:\n{}", henrik.zirc(&["who"]).await);
    }
    who
}

async fn until(machine: &Machine, args: &[&str], has: &str) {
    tokio::time::timeout(WAIT, async {
        while !machine.zirc(args).await.contains(has) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("zirc {args:?} never said {has:?}"));
}

/// What a watch prints, a line at a time.
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

/// The lines a watch prints from now until it's quiet, waiting for the first.
async fn heard(lines: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
    let mut all = vec![match tokio::time::timeout(WAIT, lines.recv()).await {
        Ok(Some(line)) => line,
        _ => return vec!["(nothing)".to_owned()],
    }];
    while let Ok(Some(line)) = tokio::time::timeout(QUIET, lines.recv()).await {
        all.push(line);
    }
    all
}

/// A terminal: what was typed, and what came back.
#[derive(Default)]
struct Terminal {
    title: &'static str,
    lines: Vec<Value>,
}

impl Terminal {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            lines: Vec::new(),
        }
    }

    fn typed(&mut self, command: &str, output: &str) {
        self.lines.push(json!({ "command": command }));
        for line in output.lines() {
            self.lines.push(json!({ "output": line }));
        }
    }

    fn printed(&mut self, lines: &[String]) {
        for line in lines {
            self.lines.push(json!({ "output": line }));
        }
    }
}

#[tokio::test]
async fn the_terminals_are_as_stored() {
    let ergo = require_ergo!();
    // Absolute: the commands below run in each machine's clone.
    let Some(stored) =
        std::env::var_os("ZIRC_TERMINALS").map(|path| std::env::current_dir().unwrap().join(path))
    else {
        eprintln!("ZIRC_TERMINALS isn't set: run this with bazel test");
        return;
    };
    let henrik = Machine::new(&ergo, "henrik-mbp", "henrik").await;
    let raj = Machine::new(&ergo, "raj-studio", "raj").await;
    let ci = Machine::new(&ergo, "ci-runner", "ci").await;
    for machine in [&henrik, &raj, &ci] {
        let home = machine.home.path().to_owned();
        tokio::spawn(zircd::run(home, std::future::pending()));
    }
    until(&henrik, &["status"], "connected").await;
    until(&raj, &["status"], "connected").await;
    until(&ci, &["status"], "connected").await;
    let mut terminals: Vec<(&str, Terminal)> = Vec::new();

    terminals.push(("repo-watch", watching_repo(&henrik).await));

    terminals.push(("repo-add", adding_repos(&henrik, &raj, &ci).await));

    // Raj's agent has been in the e2e harness a while; it watches for what concerns it.
    raj.edit("tests/e2e/harness.rs", "pub const RETRIES: u32 = 3;\n");
    until(&henrik, &["who"], "tests/e2e/harness.rs").await;
    let mut lines = watching(&raj, "cc-91b7").await;
    let mut watch = Terminal::new("zirc watch --session cc-91b7");
    watch
        .lines
        .push(json!({ "command": "zirc watch --session cc-91b7" }));

    // Henrik's agent says what it's about to change, and Raj's hears it.
    let mut coordinate = Terminal::new("~/src/server");
    let intent = [
        "intend",
        "--session",
        "cc-7d3a",
        "--name",
        "main-fix",
        "tests/e2e/reconnect.rs",
        "tests/e2e/harness.rs",
        "-m",
        "reconnect test: bind port 0",
    ];
    let said = henrik.zirc(&intent).await;
    coordinate.typed(
        "zirc intend --session cc-7d3a tests/e2e/reconnect.rs tests/e2e/harness.rs -m \"reconnect test: bind port 0\"",
        &said,
    );
    watch.printed(&heard(&mut lines).await);
    henrik.edit("tests/e2e/harness.rs", "pub const RETRIES: u32 = 2;\n");
    watch.printed(&heard(&mut lines).await);
    let who = who_noting_a_missing_set(&henrik, &raj).await;
    coordinate.typed("zirc who", &who);
    terminals.push(("coordinate", coordinate));

    // Raj's agent needs main-fix to land first: it says so, then watches, and is called back
    // when it does.
    let mut callback = Terminal::new("~/src/server");
    callback.typed(
        "zirc wait-for --session cc-91b7 main-fix",
        &raj.zirc(&["wait-for", "--session", "cc-91b7", "main-fix"])
            .await,
    );
    callback
        .lines
        .push(json!({ "command": "zirc watch --session cc-91b7" }));

    // CI says the landing went in, naming the session that asked for it.
    let mut landing = Terminal::new("ci");
    let landed = [
        "emit",
        "x-build/landing",
        "--session",
        "ci",
        "--json",
        r#"{"branch":"main-fix","commit":"9c1e2ab","build":1419}"#,
        "cc-91b7: main-fix landed as 9c1e2ab (build 1419): rebase onto it",
    ];
    let sent = ci.zirc(&landed).await;
    landing.typed(
        "zirc emit x-build/landing --session ci --json '{\"branch\":\"main-fix\",\"commit\":\"9c1e2ab\",\"build\":1419}' \"cc-91b7: main-fix landed as 9c1e2ab (build 1419): rebase onto it\"",
        &sent,
    );
    let called_back = heard(&mut lines).await;
    watch.printed(&called_back);
    callback.printed(&called_back);
    terminals.push(("ci", landing));
    terminals.push(("callback", callback));

    terminals.push(("decide", deciding(&henrik).await));
    terminals.push(("watch", watch));

    let mut now = serde_json::Map::new();
    for (name, terminal) in terminals {
        let lines: Vec<Value> = terminal
            .lines
            .into_iter()
            .map(|line| as_the_teams(line, &ergo, &[&henrik, &raj, &ci]))
            .collect();
        now.insert(
            name.to_owned(),
            json!({ "title": terminal.title, "lines": lines }),
        );
    }
    let now = serde_json::to_string_pretty(&Value::Object(now)).unwrap() + "\n";
    same_as_stored(&stored, &now);
}

/// Getting started: Henrik watches the repository from her clone, as zirc.dev's home page shows
/// it, and zircd says where.
async fn watching_repo(henrik: &Machine) -> Terminal {
    let mut watched = Terminal::new("~/src/server");
    watched.typed("zirc repo add", &henrik.zirc(&["repo", "add"]).await);
    until(henrik, &["status"], "#server").await;
    watched.typed("zirc status", &henrik.zirc(&["status"]).await);
    watched
}

/// Henrik's agent adds the repository as its own session's, as Raj's and CI do theirs.
async fn adding_repos(henrik: &Machine, raj: &Machine, ci: &Machine) -> Terminal {
    let mut start = Terminal::new("~/src/server");
    let added = henrik.zirc(&["repo", "add", "--session", "cc-7d3a"]).await;
    start.typed("zirc repo add --session cc-7d3a", &added);
    raj.zirc(&["repo", "add", "--session", "cc-91b7"]).await;
    ci.zirc(&["repo", "add"]).await;
    until(henrik, &["status"], "#server").await;
    start.typed("zirc status", &henrik.zirc(&["status"]).await);
    start
}

/// Raj's machine's `zirc watch --session <session>`, once zircd has it: what it prints.
async fn watching(machine: &Machine, session: &'static str) -> mpsc::UnboundedReceiver<String> {
    let (sender, lines) = mpsc::unbounded_channel();
    let home = machine.home.path().to_owned();
    tokio::spawn(async move {
        let args = [
            "zirc",
            "--home",
            home.to_str().unwrap(),
            "watch",
            "--session",
            session,
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        let _ = zirc::run(cli, &mut Lines(sender, Vec::new())).await;
    });
    until(machine, &["status"], session).await;
    lines
}

/// Henrik decides, and her agent records it in her words.
async fn deciding(henrik: &Machine) -> Terminal {
    let mut decide = Terminal::new("~/src/server");
    let decided = [
        "decide",
        "--session",
        "cc-7d3a",
        "--quote",
        "land it ahead of the queue",
        "--about",
        "the fix for main",
    ];
    decide.typed(
        "zirc decide --session cc-7d3a --quote \"land it ahead of the queue\" --about \"the fix for main\"",
        &henrik.zirc(&decided).await,
    );
    decide.typed("zirc decisions", &henrik.zirc(&["decisions"]).await);
    decide
}

/// Fails, leaving `now` in the test's outputs, when it isn't what's `stored`.
fn same_as_stored(stored: &Path, now: &str) {
    let before = std::fs::read_to_string(stored).unwrap();
    if now != before {
        if let Some(out) = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR") {
            std::fs::write(Path::new(&out).join("terminals.json"), now).unwrap();
        }
        let mut diff = String::new();
        let (old, new): (Vec<_>, Vec<_>) = (before.lines().collect(), now.lines().collect());
        for i in (0..old.len().max(new.len()))
            .filter(|&i| old.get(i) != new.get(i))
            .take(10)
        {
            writeln!(
                diff,
                "- {}\n+ {}",
                old.get(i).unwrap_or(&""),
                new.get(i).unwrap_or(&"")
            )
            .unwrap();
        }
        panic!(
            "zirc prints something else now than oss/fixtures/home/terminals.json says:\n{diff}\n\
             bring it up to date with: bazel run //fixtures/home:terminals"
        );
    }
}

/// A line as the team's: the test server's address as the team's, scratch directories as the paths
/// people have, message ids as one id, times as the morning of the team on zirc.dev's home page (fixtures/home), and zirc's version as
/// `<version>`, which the page fills in.
fn as_the_teams(line: Value, ergo: &Ergo, machines: &[&Machine]) -> Value {
    let Value::Object(mut fields) = line else {
        return line;
    };
    for text in fields.values_mut() {
        let Value::String(s) = text else { continue };
        let mut out = s.replace(
            &format!("127.0.0.1:{}", ergo.port()),
            "zirc.irc.zirc.dev:6697",
        );
        for machine in machines {
            for (dir, as_shown) in [
                (machine.repo.path(), "~/src/server"),
                (machine.home.path(), "~/.zirc"),
            ] {
                // macOS's /tmp is /private/tmp, which the CLI may print either way.
                let dir = dir.to_string_lossy();
                out = out
                    .replace(&format!("/private{dir}"), as_shown)
                    .replace(dir.as_ref(), as_shown);
            }
        }
        *s = same_times(&same_ids(&out)).replace(
            concat!("zircd ", env!("CARGO_PKG_VERSION")),
            "zircd <version>",
        );
    }
    Value::Object(fields)
}

/// `[id 1a2b3c]` as `[id 7f3a2c]`: a message's id is the server's, new each run.
fn same_ids(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find("[id ") {
        out.push_str(&rest[..at + 4]);
        rest = &rest[at + 4..];
        let end = rest.find(']').unwrap_or(rest.len());
        out.push_str("7f3a2c");
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A time, `2026-11-08T17:17:23.534Z`, as the morning's `2026-11-05T09:33:00.000Z`, and an
/// intent's `(29m left)` as `(30m left)`: both are when the test ran.
fn same_times(line: &str) -> String {
    const MORNING: &str = "2026-11-05T09:33:00.000Z";
    let shape = |c: u8, want: u8| match want {
        b'9' => c.is_ascii_digit(),
        _ => c == want,
    };
    let bytes = line.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let long = b"9999-99-99T99:99:99.999Z";
        let short = b"9999-99-99T99:99:99Z";
        let matches = |pattern: &[u8]| {
            bytes.len() >= i + pattern.len()
                && pattern.iter().zip(&bytes[i..]).all(|(&p, &c)| shape(c, p))
        };
        if let Some(len) = [long.len(), short.len()]
            .into_iter()
            .find(|&len| matches(if len == long.len() { long } else { short }))
        {
            out.push_str(MORNING);
            i += len;
        } else {
            let c = line[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    let mut left = String::new();
    let mut rest = out.as_str();
    while let Some(at) = rest.find("m left)") {
        let start = rest[..at].rfind('(').map_or(at, |p| p + 1);
        if rest[start..at].bytes().all(|b| b.is_ascii_digit()) && start < at {
            left.push_str(&rest[..start]);
            left.push_str("30");
        } else {
            left.push_str(&rest[..at]);
        }
        left.push_str("m left)");
        rest = &rest[at + "m left)".len()..];
    }
    left.push_str(rest);
    left
}
