//! The CLI on a machine signed in to two organisations, each with its own Ergo: commands
//! run in a repository act in that repository's organisation without being told which.

use std::path::Path;
use std::time::Duration;

use clap::Parser as _;
use tempfile::TempDir;
use zirc::Cli;
use zirc_test_ergo::{Ergo, require_ergo};

const WAIT: Duration = Duration::from_secs(15);

/// The first organisation's name: its server's host, as a server not on zirc.dev is named.
const ACME: &str = "127.0.0.1";
const GLOBEX: &str = "globex";

/// A machine in acme and globex, with a clone of acme's web repository added to acme.
struct Machine {
    home: TempDir,
    repo: TempDir,
}

impl Machine {
    async fn new(acme: &Ergo, globex: &Ergo) -> Self {
        let home = tempfile::Builder::new()
            .prefix("zirc")
            .tempdir_in("/tmp")
            .unwrap();
        let port = acme.port().to_string();
        let init = [
            "init",
            "--server",
            "127.0.0.1",
            "--port",
            &port,
            "--no-tls",
            "--nick",
            "m",
        ];
        run(home.path(), &init).await.unwrap();
        let config = home.path().join("config.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        let globex = format!(
            "\n[[org]]\nname = \"{GLOBEX}\"\nnick = \"m\"\n\
             [org.server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n",
            globex.port()
        );
        std::fs::write(&config, format!("owner = \"maya\"\n{text}{globex}")).unwrap();
        tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
        let repo = zirc_test_ergo::git::clone("https://github.com/acme/web.git");
        let machine = Self { home, repo };
        let added = machine
            .until(&["repo", "add", "--org", ACME], "as web in #agents")
            .await;
        assert!(added.contains("as web in"), "{added}");
        machine
    }

    /// `zirc <args>` in the machine's clone, as a person types it there.
    async fn zirc(&self, args: &[&str]) -> anyhow::Result<String> {
        std::env::set_current_dir(self.repo.path()).unwrap();
        run(self.home.path(), args).await
    }

    /// `zirc <args>` again until it succeeds saying `has`: what it said then.
    async fn until(&self, args: &[&str], has: &str) -> String {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Ok(out) = self.zirc(args).await
                    && out.contains(has)
                {
                    return out;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("zirc {args:?} never said {has:?}"))
    }
}

async fn run(home: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut out = Vec::new();
    let mut all = vec!["zirc", "--home", home.to_str().unwrap()];
    all.extend(args);
    zirc::run(Cli::try_parse_from(all)?, &mut out).await?;
    Ok(String::from_utf8(out)?)
}

/// The id `zirc decisions` shows for the decision about `about`.
fn id_about(decisions: &str, about: &str) -> String {
    let line = decisions
        .lines()
        .find(|line| line.contains(about))
        .unwrap_or_else(|| panic!("no decision about {about:?} in {decisions}"));
    let (_, id) = line.rsplit_once("[id ").unwrap();
    id.trim_end_matches(']').to_owned()
}

#[tokio::test]
async fn a_decision_replaces_one_in_its_repositorys_organisation() {
    let acme = require_ergo!();
    let Some(globex) = Ergo::start() else {
        return;
    };
    let machine = Machine::new(&acme, &globex).await;
    let decide = ["decide", "--session", "cc-1"];
    let first = [
        &decide[..],
        &["--about", "the release day", "--quote", "Tuesday"],
    ]
    .concat();
    machine.zirc(&first).await.unwrap();
    let decisions = machine.until(&["decisions"], "the release day").await;
    let id = id_about(&decisions, "the release day");

    // Run in the repository, --replaces finds the old one in the repository's organisation,
    // with no "say which with --org" though the machine is in two.
    let moves = ["--about", "the release day moves", "--quote", "Wednesday"];
    let second = [&decide[..], &moves, &["--replaces", &id]].concat();
    machine.zirc(&second).await.unwrap();
    let current = machine.until(&["decisions"], "the release day moves").await;
    assert!(!current.contains("\"Tuesday\""), "{current}");
}
