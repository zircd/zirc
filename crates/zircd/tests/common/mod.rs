//! What zircd's integration tests share: a machine is a daemon with its own zirc directory,
//! talking to a test Ergo. Each test file says `mod common;`, and what it doesn't use is
//! allowed to be dead there.
#![allow(
    dead_code,
    reason = "each test file says `mod common;` and uses its own part of it"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;
use zirc_core::ipc::{
    self, Delivery, Heard, LocalOverlap, Overlap, Request, Response, Target, Watching, Why,
};
use zirc_core::paths;
use zirc_proto::Event;
use zirc_test_ergo::Ergo;
use zirc_test_ergo::git::git;
use zirc_test_irc::eventually;

/// How long a machine gets to do what a test waits for.
pub const WAIT: Duration = Duration::from_secs(15);

/// A daemon with its own zirc directory.
pub struct Machine {
    pub home: TempDir,
    pub daemon: tokio::task::JoinHandle<anyhow::Result<zircd::Ended>>,
}

impl Machine {
    /// A daemon on `ergo` as `nick`, its working sets told at once, as tests wait on them.
    pub async fn start(ergo: &Ergo, nick: &str) -> Self {
        Self::start_with(ergo, nick, "").await
    }

    /// Like [`Machine::start`], with `settings` in its `config.toml` before the `[server]`;
    /// naming `announce_every` there keeps the working sets from being told at once.
    pub async fn start_with(ergo: &Ergo, nick: &str, settings: &str) -> Self {
        let now = if settings.contains("announce_every") {
            ""
        } else {
            "announce_every = 0\n"
        };
        Self::with_config(&format!(
            "nick = \"{nick}\"\n{now}{settings}{}",
            server(ergo)
        ))
        .await
    }

    /// A daemon on `ergo` as `nick`, with the defaults for everything else.
    pub async fn connect(ergo: &Ergo, nick: &str) -> Self {
        Self::with_config(&format!("nick = \"{nick}\"\n{}", server(ergo))).await
    }

    /// A daemon whose `config.toml` is `config`, once it's connected.
    pub async fn with_config(config: &str) -> Self {
        let machine = Self::spawn(config);
        machine.connected().await;
        machine
    }

    /// A daemon whose `config.toml` is `config`, not yet connected.
    pub fn spawn(config: &str) -> Self {
        Self::spawn_with(config, zircd::Hooks::default())
    }

    /// A daemon on `ergo` as `nick`, as [`Machine::start`], with `hooks` standing in for its
    /// update check and clock, once it's connected.
    pub async fn hooked(ergo: &Ergo, nick: &str, hooks: zircd::Hooks) -> Self {
        Self::hooked_with(ergo, nick, "", hooks).await
    }

    /// Like [`Machine::hooked`], with `settings` in its `config.toml`, such as an `owner`.
    pub async fn hooked_with(ergo: &Ergo, nick: &str, settings: &str, hooks: zircd::Hooks) -> Self {
        let config = format!(
            "nick = \"{nick}\"\nannounce_every = 0\n{settings}{}",
            server(ergo)
        );
        let machine = Self::spawn_with(&config, hooks);
        machine.connected().await;
        machine
    }

    /// A daemon whose `config.toml` is `config`, with `hooks`, not yet connected.
    pub fn spawn_with(config: &str, hooks: zircd::Hooks) -> Self {
        // Unix socket paths are short (104 bytes on macOS), so not Bazel's deep temp dir.
        let home = tempfile::Builder::new()
            .prefix("zirc")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::write(paths::config(home.path()), config).unwrap();
        let daemon = tokio::spawn(zircd::run_with(
            home.path().to_owned(),
            std::future::pending(),
            hooks,
        ));
        Self { home, daemon }
    }

    /// The same machine after its daemon stops and starts again on the same directory, once
    /// the new one answers.
    pub async fn restart(self) -> Self {
        self.restart_after(async {}).await
    }

    /// The same machine after its daemon stops, `meanwhile` runs, and it starts again, as
    /// [`Machine::restart`].
    pub async fn restart_after(self, meanwhile: impl Future<Output = ()>) -> Self {
        let Self { home, daemon } = self;
        daemon.abort();
        let _ = daemon.await;
        // Aborting asks the daemon's tasks to stop; until its socket stops answering, a new
        // daemon would rightly refuse to start, as one is still running.
        let socket = paths::socket(home.path());
        tokio::time::timeout(WAIT, async {
            while tokio::net::UnixStream::connect(&socket).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the first daemon's socket kept answering");
        meanwhile.await;
        let daemon = tokio::spawn(zircd::run(home.path().to_owned(), std::future::pending()));
        tokio::time::timeout(WAIT, async {
            while ipc::request(&socket, &Request::RepoList).await.is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("restarted daemon");
        Self { home, daemon }
    }

    /// Another daemon on this machine's home once this one has ended, as zircd's `main`
    /// starts it again after a restart: the same zirc directory, its state and socket.
    pub fn again(&mut self) {
        assert!(
            self.daemon.is_finished(),
            "the daemon before is still running"
        );
        self.daemon = tokio::spawn(zircd::run(
            self.home.path().to_owned(),
            std::future::pending(),
        ));
    }

    pub fn socket(&self) -> PathBuf {
        paths::socket(self.home.path())
    }

    /// The answer to `request`, failing the test if there is none.
    pub async fn request(&self, request: Request) -> Response {
        self.try_request(request).await.unwrap()
    }

    pub async fn try_request(&self, request: Request) -> Result<Response, ipc::Error> {
        ipc::request(&self.socket(), &request).await
    }

    /// `request` for one of the machine's organisations.
    pub async fn request_in(
        &self,
        org: Option<&str>,
        request: Request,
    ) -> Result<Response, ipc::Error> {
        ipc::request_in(&self.socket(), org, &request).await
    }

    /// Puts the clone at `repo` among the repositories it watches.
    pub async fn add_repo(&self, org: Option<&str>, repo: &Path) -> Result<Response, ipc::Error> {
        self.request_in(
            org,
            Request::RepoAdd {
                path: repo.to_owned(),
                session: None,
            },
        )
        .await
    }

    pub async fn status(&self) -> Option<ipc::Status> {
        match self.try_request(Request::Status).await {
            Ok(Response::Status(status)) => Some(status),
            _ => None,
        }
    }

    /// A watch as `session`, once zircd tells it what it hears: before that, it could miss
    /// what the test does next.
    pub async fn watch(&self, session: Option<&str>) -> Watching {
        let watching = ipc::watch(&self.socket(), session).await.unwrap();
        let session = session.map(str::to_owned);
        self.eventually(|status| status.watching.contains(&session), "watching")
            .await;
        watching
    }

    /// Waits until the machine's status satisfies `done`.
    pub async fn eventually(&self, done: impl Fn(&ipc::Status) -> bool, what: &str) {
        eventually(what, || async {
            self.status().await.is_some_and(|s| done(&s))
        })
        .await;
    }

    /// Waits until the machine is connected, and returns its status.
    pub async fn connected(&self) -> ipc::Status {
        eventually("a connection", || async {
            self.status().await.filter(|status| status.connected)
        })
        .await
    }
}

/// The `[server]` table of a config pointing at `ergo`.
fn server(ergo: &Ergo) -> String {
    format!(
        "[server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n",
        ergo.port()
    )
}

// What the machine tests listen for and ask with, shared by the files by topic.

pub const REMOTE: &str = "git@github.com:acme/api.git";

pub fn clone() -> TempDir {
    zirc_test_ergo::git::clone(REMOTE)
}

pub async fn heard(watching: &mut Watching) -> Heard {
    tokio::time::timeout(WAIT, watching.next())
        .await
        .expect("nothing heard in time")
        .unwrap()
        .expect("zircd stopped")
}

pub async fn next(watching: &mut Watching) -> Delivery {
    loop {
        if let Heard::Message(delivery) = heard(watching).await
            && !matches!(delivery.message.event, Event::Working { .. })
        {
            return delivery;
        }
    }
}

pub async fn next_post(watching: &mut Watching) -> Delivery {
    loop {
        let delivery = next(watching).await;
        if matches!(delivery.message.event, Event::Text { .. }) {
            return delivery;
        }
    }
}

pub async fn next_working(watching: &mut Watching, nick: &str) -> Vec<String> {
    loop {
        if let Heard::Message(delivery) = heard(watching).await
            && delivery.from == nick
            && let Event::Working { paths, .. } = delivery.message.event
        {
            return paths;
        }
    }
}

pub async fn next_overlap(watching: &mut Watching) -> Overlap {
    loop {
        if let Heard::Overlap(overlap) = heard(watching).await {
            return overlap;
        }
    }
}

pub async fn next_local_overlap(watching: &mut Watching) -> LocalOverlap {
    loop {
        if let Heard::LocalOverlap(overlap) = heard(watching).await {
            return overlap;
        }
    }
}

/// Every channel a machine watching a clone of the test repository joins, sorted, as its
/// status says them once it's settled: the organisation's.
pub const JOINED: [&str; 5] = ["#agents", "#decisions", "#general", "#gotchas", "#ops"];

pub async fn two_machines(ergo: &Ergo) -> [(Machine, TempDir); 2] {
    let mut machines = Vec::new();
    for nick in ["alpha", "beta"] {
        let machine = Machine::start(ergo, nick).await;
        let clone = clone();
        let Response::Repos { repos } = machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: None,
            })
            .await
        else {
            panic!("not a repo list")
        };
        assert_eq!(repos[0].scope, "#api");
        machine
            .eventually(|status| status.channels == JOINED, "joining")
            .await;
        machines.push((machine, clone));
    }
    machines.try_into().unwrap_or_else(|_| unreachable!())
}

pub async fn machine_with_a_session_worktree(ergo: &Ergo) -> (Machine, TempDir, PathBuf, PathBuf) {
    machine_with_a_session_worktree_and(ergo, "").await
}

/// Like [`machine_with_a_session_worktree`], with `settings` in the machine's `config.toml`.
pub async fn machine_with_a_session_worktree_and(
    ergo: &Ergo,
    settings: &str,
) -> (Machine, TempDir, PathBuf, PathBuf) {
    let machine = Machine::start_with(ergo, "solo", settings).await;
    // A session's worktree inside the checkout, where Claude Code keeps them.
    let main = clone();
    git(
        main.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "login",
            ".claude/worktrees/login",
        ],
    );
    let root = |path: &Path| path.canonicalize().unwrap();
    let (main_root, login_root) = (
        root(main.path()),
        root(&main.path().join(".claude/worktrees/login")),
    );
    for (path, session) in [(&main_root, "cc-main"), (&login_root, "cc-login")] {
        machine
            .request(Request::RepoAdd {
                path: path.clone(),
                session: Some(session.into()),
            })
            .await;
    }
    machine
        .eventually(
            |status| status.channels.iter().any(|c| c == "#agents"),
            "joining",
        )
        .await;
    (machine, main, main_root, login_root)
}

pub async fn until_refused(machine: &Machine, request: &Request, why: &str) {
    tokio::time::timeout(WAIT, async {
        while !ipc::request(&machine.socket(), request)
            .await
            .is_err_and(|err| err.to_string().contains(why))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for a refusal saying {why}"));
}

/// CI's line about the test repository, as it says in `#agents`.
pub fn ci_message(event: Event) -> zirc_proto::Message {
    zirc_proto::Message {
        repo: Some("api".into()),
        ..zirc_proto::Message::new("ci", event)
    }
}

pub fn plain_post(repo: &Path, session: &str, text: &str, to: Target) -> Request {
    Request::Post {
        repo: repo.to_owned(),
        session: session.into(),
        text: text.into(),
        to,
        app: None,
        everyone: false,
        reply: None,
    }
}

pub async fn post(machine: &Machine, repo: &Path, session: &str, text: &str) {
    machine
        .request(Request::Post {
            repo: repo.to_owned(),
            session: session.into(),
            text: text.into(),
            to: Target::Agents,
            app: None,
            everyone: false,
            reply: None,
        })
        .await;
}

pub async fn next_post_why(watching: &mut Watching) -> (String, Option<Why>) {
    let delivery = next_post(watching).await;
    let Event::Text { body } = delivery.message.event else {
        unreachable!()
    };
    (body, delivery.why)
}

pub async fn who(machine: &Machine) -> Vec<ipc::ChannelPresence> {
    let Response::Who { channels } = machine.request(Request::Who).await else {
        panic!("not a who")
    };
    channels
}

pub async fn until_who(
    machine: &Machine,
    done: impl Fn(&[ipc::ChannelPresence]) -> bool,
    what: &str,
) {
    tokio::time::timeout(WAIT, async {
        while !done(&who(machine).await) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

pub fn ten_lines(line: usize, text: &str) -> String {
    (1..=10)
        .map(|i| {
            if i == line {
                format!("{text}\n")
            } else {
                format!("line {i}\n")
            }
        })
        .collect()
}

pub async fn next_concerning(watching: &mut Watching) -> (Event, Why) {
    loop {
        let delivery = next(watching).await;
        if let Some(why) = delivery.why {
            return (delivery.message.event, why);
        }
    }
}

pub async fn next_reaction(watching: &mut Watching) -> ipc::Reaction {
    loop {
        if let Heard::Reaction(reaction) = heard(watching).await {
            return reaction;
        }
    }
}

pub async fn own_sets(machine: &Machine, nick: &str) -> Vec<OwnSet> {
    let mut sets: Vec<_> = who(machine)
        .await
        .into_iter()
        .filter(|channel| channel.channel == "#api")
        .flat_map(|channel| channel.members)
        .filter(|member| member.nick == nick)
        .map(|member| (member.branch, member.session, member.paths))
        .collect();
    sets.sort();
    sets
}

pub type OwnSet = (Option<String>, Option<String>, Vec<String>);

pub fn own_set(branch: &str, session: Option<&str>, paths: &[&str]) -> OwnSet {
    (
        Some(branch.to_owned()),
        session.map(str::to_owned),
        paths.iter().map(|&path| path.to_owned()).collect(),
    )
}

pub async fn until_own_sets(machine: &Machine, nick: &str, expected: &[OwnSet], what: &str) {
    tokio::time::timeout(WAIT, async {
        while own_sets(machine, nick).await != expected {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

pub async fn own_overlap(watching: &mut Watching) -> zirc_core::presence::LocalOverlap {
    loop {
        if let Heard::LocalOverlap(overlap) = heard(watching).await
            && overlap.why.is_some()
        {
            return overlap;
        }
    }
}

pub fn to_deploys(clone: &TempDir, session: &str, text: &str) -> Request {
    Request::Post {
        repo: clone.path().to_owned(),
        session: session.into(),
        text: text.into(),
        to: Target::Topic {
            name: "deploys".into(),
        },
        app: None,
        everyone: !session.is_empty() && text.starts_with("@everyone"),
        reply: None,
    }
}

/// A machine that watches a clone of the project, as `session` when it has one, and has
/// joined `#api`. What it does is said as verbs, so a test names the action and not the
/// requests that carry it out.
pub struct Peer {
    pub machine: Machine,
    pub clone: TempDir,
    pub session: Option<&'static str>,
}

impl Peer {
    /// `machine` watching a fresh clone of the project, once it is in `#api`.
    pub async fn join(machine: Machine, session: Option<&'static str>) -> Self {
        let clone = clone();
        machine
            .request(Request::RepoAdd {
                path: clone.path().to_owned(),
                session: session.map(Into::into),
            })
            .await;
        machine
            .eventually(
                // #general too: leases are agreed there, and it's joined after #agents.
                |status| {
                    ["#agents", "#general"]
                        .iter()
                        .all(|want| status.channels.iter().any(|c| c == want))
                },
                "joining",
            )
            .await;
        Self {
            machine,
            clone,
            session,
        }
    }

    /// A watch as this peer's session, once zircd tells it what it hears.
    pub async fn watch(&self) -> Watching {
        self.machine.watch(self.session).await
    }

    /// Changes the file `path` in the clone, making the directories above it.
    pub fn edits(&self, path: &str, content: &str) {
        let file = self.clone.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    /// Says to the others that this session will change `paths`.
    pub async fn intends(&self, summary: &str, paths: &[&str]) -> Response {
        self.machine
            .request(Request::Intend {
                repo: self.clone.path().to_owned(),
                session: self.session.unwrap().into(),
                summary: summary.into(),
                paths: paths.iter().map(PathBuf::from).collect(),
                seconds: 600,
            })
            .await
    }

    /// Posts `text` to the repository's channel as this session.
    pub async fn says(&self, text: &str) {
        post(
            &self.machine,
            self.clone.path(),
            self.session.unwrap(),
            text,
        )
        .await;
    }

    /// Waits until this machine lists an intent of `session` that satisfies `holds`.
    pub async fn sees_intent_of(&self, session: &str, holds: impl Fn(&[String]) -> bool) {
        until_who(
            &self.machine,
            |channels| {
                channels.iter().any(|c| {
                    c.intents
                        .iter()
                        .any(|i| i.session == session && holds(&i.paths))
                })
            },
            &format!("{session}'s intent"),
        )
        .await;
    }

    /// Waits until this machine lists no intent of `session`.
    pub async fn sees_no_intent_of(&self, session: &str) {
        until_who(
            &self.machine,
            |channels| {
                channels
                    .iter()
                    .all(|c| c.intents.iter().all(|i| i.session != session))
            },
            &format!("{session}'s intent to end"),
        )
        .await;
    }

    /// Waits until this machine lists `nick` as changing files.
    pub async fn sees_changes_by(&self, nick: &str) {
        until_who(
            &self.machine,
            |channels| {
                channels
                    .iter()
                    .any(|c| c.members.iter().any(|m| m.nick == nick))
            },
            &format!("{nick}'s change"),
        )
        .await;
    }
}

/// Two machines, `alpha` and `beta`, each watching its own clone of the project, in `#api`
/// on one Ergo.
pub struct Fleet {
    pub alpha: Peer,
    pub beta: Peer,
    _ergo: Ergo,
}

impl Fleet {
    /// Both machines with a session each: `cc-a` on alpha and `cc-b` on beta.
    pub async fn with_sessions(ergo: Ergo) -> Self {
        let alpha = Peer::join(Machine::start(&ergo, "alpha").await, Some("cc-a")).await;
        let beta = Peer::join(Machine::start(&ergo, "beta").await, Some("cc-b")).await;
        Self {
            alpha,
            beta,
            _ergo: ergo,
        }
    }
}

/// Collects what `watching` hears up to and including the first that `marks`, which the
/// test makes sure comes after whatever it says shouldn't be heard: a sentinel. Everything
/// sent before the marker has arrived by then, so a test checks what's absent among these
/// rather than waiting a while and hoping.
pub async fn until_marker(watching: &mut Watching, marks: impl Fn(&Heard) -> bool) -> Vec<Heard> {
    let mut all = Vec::new();
    loop {
        let heard = heard(watching).await;
        let done = marks(&heard);
        all.push(heard);
        if done {
            return all;
        }
    }
}

/// Whether `heard` is a post whose text contains `text`.
pub fn says(heard: &Heard, text: &str) -> bool {
    matches!(heard, Heard::Message(delivery)
        if matches!(&delivery.message.event, Event::Text { body } if body.contains(text)))
}

/// The next intent heard.
pub async fn next_intent(watching: &mut Watching) -> Delivery {
    loop {
        let delivery = next(watching).await;
        if matches!(delivery.message.event, Event::Intent { .. }) {
            return delivery;
        }
    }
}
