//! A machine signed in to two organisations, each with its own Ergo: its repositories
//! belong to one organisation each, and nothing about one ever reaches the other's server.

use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;
use zirc_core::ipc::{self, Delivery, Heard, Request, Response, Target, Watching};
use zirc_core::paths;
use zirc_core::settings::Settings;
use zirc_proto::Event;
use zirc_test_ergo::Ergo;
use zirc_test_irc::register;

mod common;
use common::{Machine, WAIT};

/// The first organisation's name: its server's host, as a server not on zirc.dev is named.
const ACME: &str = "127.0.0.1";
const GLOBEX: &str = "globex";
const ACME_REPO: &str = "git@github.com:acme/rockets.git";
const GLOBEX_REPO: &str = "git@github.com:globex/magnets.git";

/// The messages heard from `nick`, until one in `channel` that says `text`.
async fn heard_until(
    watching: &mut Watching,
    nick: &str,
    channel: &str,
    text: &str,
) -> Vec<Delivery> {
    let mut heard = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let Heard::Message(delivery) = watching.next().await.unwrap().unwrap() else {
                continue;
            };
            if delivery.from != nick {
                continue;
            }
            let done = delivery.channel == channel
                && matches!(&delivery.message.event, Event::Text { body } if body == text);
            heard.push(delivery);
            if done {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("didn't hear {text:?} in {channel}"));
    heard
}

/// Someone in the organisation whose server is `ergo` with clones of both projects, so it
/// would pass on what reached its server about either, and its watch.
async fn observer(ergo: &Ergo, nick: &str) -> (Machine, [TempDir; 2], Watching) {
    let observer = Machine::connect(ergo, nick).await;
    let clones = [ACME_REPO, GLOBEX_REPO].map(zirc_test_ergo::git::clone);
    for clone in &clones {
        observer.add_repo(None, clone.path()).await.unwrap();
    }
    observer
        .eventually(
            |status| {
                status.channels
                    == [
                        "#decisions",
                        "#general",
                        "#magnets",
                        "#magnets/agents",
                        "#magnets/gotchas",
                        "#magnets/landings",
                        "#ops",
                        "#rockets",
                        "#rockets/agents",
                        "#rockets/gotchas",
                        "#rockets/landings",
                    ]
            },
            "joining",
        )
        .await;
    let watching = observer.watch(None).await;
    (observer, clones, watching)
}

/// Adds `rockets` to acme and `magnets` and another clone of it to globex, checking how a
/// repository's organisation is decided. Answers the other clone.
async fn bind(machine: &Machine, rockets: &TempDir, magnets: &TempDir) -> TempDir {
    // With two organisations, a repository's organisation is asked for...
    let unsaid = machine.add_repo(None, rockets.path()).await.unwrap_err();
    assert!(unsaid.to_string().contains("--org"), "{unsaid}");
    machine.add_repo(Some(ACME), rockets.path()).await.unwrap();
    machine
        .add_repo(Some(GLOBEX), magnets.path())
        .await
        .unwrap();
    // ...and is never another's too.
    let twice = machine
        .add_repo(Some(GLOBEX), rockets.path())
        .await
        .unwrap_err();
    assert!(
        twice.to_string().contains("belongs to 127.0.0.1"),
        "{twice}"
    );
    // Another clone of a project is its organisation's, without saying.
    let another = zirc_test_ergo::git::clone(GLOBEX_REPO);
    machine.add_repo(None, another.path()).await.unwrap();
    let Ok(Response::Repos { repos }) = machine.request_in(None, Request::RepoList).await else {
        panic!("not a repo list")
    };
    let orgs = |path: &Path| {
        let root = std::fs::canonicalize(path).unwrap();
        repos
            .iter()
            .find(|repo| repo.path == root)
            .and_then(|repo| repo.org.clone())
    };
    assert_eq!(orgs(rockets.path()).as_deref(), Some(ACME));
    assert_eq!(orgs(magnets.path()).as_deref(), Some(GLOBEX));
    assert_eq!(orgs(another.path()).as_deref(), Some(GLOBEX));
    machine
        .eventually(
            |status| {
                status.orgs[0].channels
                    == [
                        "#decisions",
                        "#general",
                        "#ops",
                        "#rockets",
                        "#rockets/agents",
                        "#rockets/gotchas",
                        "#rockets/landings",
                    ]
                    && status.orgs[1].channels
                        == [
                            "#decisions",
                            "#general",
                            "#magnets",
                            "#magnets/agents",
                            "#magnets/gotchas",
                            "#magnets/landings",
                            "#ops",
                        ]
            },
            "joining each organisation's channels",
        )
        .await;
    another
}

/// The observer's nick in `org`.
fn org_nick(org: &str) -> &'static str {
    if org == ACME { "acme" } else { "globex" }
}

#[tokio::test]
async fn repositories_never_cross_organisations() {
    let (Some(acme), Some(globex)) = (Ergo::start(), Ergo::start()) else {
        return;
    };
    let mut observers = [
        observer(&acme, "acme-eve").await,
        observer(&globex, "globex-eve").await,
    ];
    let machine = Machine::with_config(&format!(
        "nick = \"m\"\n[server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n\n\
         [[org]]\nname = \"{GLOBEX}\"\nnick = \"m\"\n\
         [org.server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n",
        acme.port(),
        globex.port()
    ))
    .await;
    machine
        .eventually(
            |status| status.orgs.len() == 2 && status.orgs.iter().all(|org| org.connected),
            "connecting to both",
        )
        .await;
    let rockets = zirc_test_ergo::git::clone(ACME_REPO);
    let magnets = zirc_test_ergo::git::clone(GLOBEX_REPO);

    let _another = bind(&machine, &rockets, &magnets).await;

    // Work in globex's repository: a working set and a post...
    std::fs::write(magnets.path().join("README.md"), "changed\n").unwrap();
    let post = |repo: &TempDir, text: &str| Request::Post {
        repo: repo.path().to_owned(),
        session: "cc-1".into(),
        text: text.into(),
        to: Target::Repo,
        app: None,
        everyone: false,
        force: true,
        reply: None,
    };
    machine
        .request_in(None, post(&magnets, "magnets news"))
        .await
        .unwrap();
    let (_, _, globex_hears) = &mut observers[1];
    let heard = heard_until(globex_hears, "m", "#magnets", "magnets news").await;
    assert!(heard.iter().all(|delivery| delivery.channel == "#magnets"));
    // ...then in acme's.
    machine
        .request_in(None, post(&rockets, "rockets news"))
        .await
        .unwrap();
    let (_, _, acme_hears) = &mut observers[0];
    let heard = heard_until(acme_hears, "m", "#rockets", "rockets news").await;
    // Acme's server heard nothing about globex's repository, which went first.
    assert!(
        heard.iter().all(|delivery| delivery.channel == "#rockets"),
        "{heard:?}"
    );
    // And globex's heard nothing about acme's.
    machine
        .request_in(None, post(&magnets, "magnets again"))
        .await
        .unwrap();
    let (_, _, globex_hears) = &mut observers[1];
    let heard = heard_until(globex_hears, "m", "#magnets", "magnets again").await;
    assert!(
        heard.iter().all(|delivery| delivery.channel == "#magnets"),
        "{heard:?}"
    );

    // One watch hears both organisations, each line saying whose it is.
    let mut watching = machine.watch(None).await;
    for (at, org, channel) in [(0, ACME, "#rockets"), (1, GLOBEX, "#magnets")] {
        let (observer, clones, _) = &observers[at];
        let text = format!("hello from {org}");
        observer
            .request_in(None, post(&clones[at], &text))
            .await
            .unwrap();
        let heard = heard_until(
            &mut watching,
            &format!("{}-eve", org_nick(org)),
            channel,
            &text,
        )
        .await;
        assert_eq!(heard.last().unwrap().org.as_deref(), Some(org));
    }

    // Who is one organisation's: asked of neither, it asks which; asked of each, its own.
    let asked = machine.request_in(None, Request::Who).await;
    let Err(refused) = asked else {
        panic!("who answered for both organisations: {asked:?}")
    };
    assert!(
        refused.to_string().contains(&format!(
            "this machine is in {ACME} and {GLOBEX}: say which with --org"
        )),
        "{refused}"
    );
    for (org, channel) in [(ACME, "#rockets"), (GLOBEX, "#magnets")] {
        let Ok(Response::Who { channels }) = machine.request_in(Some(org), Request::Who).await
        else {
            panic!("not a who")
        };
        let theirs: Vec<&str> = channels.iter().map(|it| it.channel.as_str()).collect();
        assert!(theirs.contains(&channel), "{org}: {theirs:?}");
    }
}

/// A machine in two organisations, `acme` on `acme` and `globex` on `globex`, each with an
/// account of its own whose passphrase there is `old-horse`, and `webs` as their web apps.
fn two_organisations(acme: &Ergo, globex: &Ergo, webs: [&str; 2]) -> String {
    let [acme_web, globex_web] = webs;
    format!(
        "nick = \"m-acme\"\nweb = \"{acme_web}\"\n\
         [server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n\
         [account]\nname = \"m-acme\"\npassword = \"old-horse\"\n\n\
         [[org]]\nname = \"globex\"\nnick = \"m-globex\"\nweb = \"{globex_web}\"\n\
         [org.server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n\
         [org.account]\nname = \"m-globex\"\npassword = \"old-horse\"\n",
        acme.port(),
        globex.port()
    )
}

/// The credentials a web app gives `account` on the server `ergo`.
fn current(ergo: &Ergo, account: &str) -> String {
    format!(
        r#"{{"server":{{"host":"127.0.0.1","port":{},"tls":false}},"account":{{"name":"{account}","password":"new-horse"}},"nick":"{account}"}}"#,
        ergo.port()
    )
}

#[tokio::test]
async fn in_several_organisations_a_refresh_for_another_server_is_refused() {
    let (Some(acme), Some(globex)) = (Ergo::start(), Ergo::start()) else {
        return;
    };
    register(&acme, "m-acme", "old-horse").await;
    register(&globex, "m-globex", "new-horse").await;
    // Globex refuses the machine's passphrase, and the web app answers with acme's server:
    // taking it would announce globex's repositories on acme's server.
    let (web, asked) = zirc_test_ergo::web::serve(vec![(200, current(&acme, "m-globex"))]);
    let machine = Machine::with_config(&two_organisations(&acme, &globex, [&web, &web])).await;
    let home = &machine.home;
    // Waited for off the test's thread, which zircd's refresh runs on too.
    let asked = tokio::task::spawn_blocking(move || asked.join().unwrap());
    let request = tokio::time::timeout(WAIT, asked)
        .await
        .expect("the web app was asked")
        .unwrap()
        .remove(0);
    assert!(request.starts_with("POST /api/cli/machine "), "{request}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let text = std::fs::read_to_string(paths::config(home.path())).unwrap();
    let settings = Settings::from_toml(&text).unwrap();
    let saved = settings.organisation("globex").unwrap();
    assert_eq!(saved.server.port, Some(globex.port()), "{text}");
    assert_eq!(saved.account.unwrap().password, "old-horse");
    let Ok(Response::Status(status)) =
        ipc::request(&paths::socket(home.path()), &Request::Status).await
    else {
        panic!("no status")
    };
    assert!(
        status.orgs[0].connected && !status.orgs[1].connected,
        "{status:?}"
    );
}

#[tokio::test]
async fn two_organisations_refused_at_once_both_keep_their_new_passphrase() {
    let (Some(acme), Some(globex)) = (Ergo::start(), Ergo::start()) else {
        return;
    };
    register(&acme, "m-acme", "new-horse").await;
    register(&globex, "m-globex", "new-horse").await;
    // Both refuse at once; each organisation's answer is saved without losing the other's.
    let (acme_web, _) = zirc_test_ergo::web::serve(vec![(200, current(&acme, "m-acme"))]);
    let (globex_web, _) = zirc_test_ergo::web::serve(vec![(200, current(&globex, "m-globex"))]);
    let machine =
        Machine::with_config(&two_organisations(&acme, &globex, [&acme_web, &globex_web])).await;
    let home = &machine.home;
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Response::Status(status)) =
                ipc::request(&paths::socket(home.path()), &Request::Status).await
                && status.orgs.iter().all(|org| org.connected)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("both connected");
    let text = std::fs::read_to_string(paths::config(home.path())).unwrap();
    let settings = Settings::from_toml(&text).unwrap();
    for org in settings.organisations() {
        assert_eq!(org.account.unwrap().password, "new-horse", "{text}");
    }
}

#[tokio::test]
async fn watches_no_repository_on_zirc_devs_public_server() {
    let (Some(acme), Some(public)) = (Ergo::start(), Ergo::start()) else {
        return;
    };
    let machine = Machine::with_config(&format!(
        "nick = \"m\"\n[server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n\n\
         [[org]]\nname = \"public\"\nnick = \"m\"\n\
         [org.server]\nhost = \"127.0.0.1\"\nport = {}\ntls = false\n",
        acme.port(),
        public.port()
    ))
    .await;
    let repo = zirc_test_ergo::git::clone(ACME_REPO);
    let refused = machine
        .add_repo(Some("public"), repo.path())
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("public server"), "{refused}");
    // Its own organisation's repositories are watched as ever.
    machine.add_repo(Some(ACME), repo.path()).await.unwrap();
}
