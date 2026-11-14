//! Sessions in worktrees of one machine, and how they hear each other.

use std::path::PathBuf;
use std::time::Duration;
use zirc_core::ipc::{Heard, LocalOverlap, Request, Response, Target, Why, Worktree};
use zirc_proto::Event;
use zirc_test_ergo::git::git;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn sessions_in_worktrees_on_one_machine_hear_each_other() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut hears = machine.watch(None).await;

    // Both sessions change the same file.
    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    let overlap = loop {
        if let Heard::LocalOverlap(overlap) = heard(&mut hears).await {
            break overlap;
        }
    };
    assert_eq!(
        overlap,
        LocalOverlap {
            channel: "#api".into(),
            worktrees: [
                Worktree {
                    path: main_root.clone(),
                    branch: Some("main".into())
                },
                Worktree {
                    path: login_root.clone(),
                    branch: Some("login".into())
                },
            ],
            paths: vec!["README.md".into()],
            // Both change README.md's only line.
            same_lines: [("README.md".to_owned(), vec![[1, 1]])].into(),
            apart: Vec::new(),
            why: Some(Why::Overlap),
        }
    );

    // `who` lists each worktree, and the nested one isn't a change of the main checkout's.
    let Response::Who { channels } = machine.request(Request::Who).await else {
        panic!("not a who")
    };
    let members: Vec<(Option<PathBuf>, Vec<String>)> = channels[0]
        .members
        .iter()
        .map(|member| {
            let path = member
                .worktree
                .as_ref()
                .map(|worktree| worktree.path.clone());
            (path, member.paths.clone())
        })
        .collect();
    assert_eq!(
        members,
        [
            (Some(main_root.clone()), vec!["README.md".to_owned()]),
            (Some(login_root.clone()), vec!["README.md".to_owned()]),
        ]
    );

    // What one session says, and its commits, reach the other here too.
    machine
        .request(Request::Post {
            repo: login_root.clone(),
            session: "cc-login".into(),
            text: "taking: the login flow".into(),
            to: Target::Repo,
            app: None,
            everyone: false,
            force: true,
            reply: None,
        })
        .await;
    let said = next(&mut hears).await;
    assert_eq!(
        (said.from.as_str(), said.message.session.as_str()),
        ("solo", "cc-login")
    );
    // ...and `who` shows what it's taken on.
    let Response::Who { channels } = machine.request(Request::Who).await else {
        panic!("not a who")
    };
    let claims: Vec<(&str, &str)> = channels[0]
        .claims
        .iter()
        .map(|claim| (claim.session.as_str(), claim.task.as_str()))
        .collect();
    assert_eq!(claims, [("cc-login", "the login flow")]);
}

#[tokio::test]
async fn announces_commits_any_tool_makes_and_forgets_removed_worktrees() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut hears = machine.watch(None).await;
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();

    // A commit made the way some tools make them, without a reflog entry saying
    // "commit", is still announced, and signed by the session working there.
    git(
        &login_root,
        &[
            "-c",
            "core.logAllRefUpdates=false",
            "commit",
            "--quiet",
            "--all",
            "--message",
            "Login's take",
        ],
    );
    let commit = next(&mut hears).await;
    assert!(
        matches!(&commit.message.event, Event::Commit { branch, subject, .. }
            if branch == "login" && subject == "Login's take"),
        "{commit:?}"
    );
    assert_eq!(commit.message.session, "cc-login");

    // When a session's worktree is removed, it's no longer watched.
    git(
        &main_root,
        &["worktree", "remove", "--force", ".claude/worktrees/login"],
    );
    tokio::time::timeout(WAIT, async {
        loop {
            if let Response::Repos { repos } = machine.request(Request::RepoList).await
                && repos.len() == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the removed worktree is still watched");
}

#[tokio::test]
async fn others_see_what_a_worktrees_work_is_built_on_once_its_committed() {
    let ergo = require_ergo!();
    let (solo, _main, _main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let beta = Machine::start(&ergo, "beta").await;
    let clone = clone();
    beta.request(Request::RepoAdd {
        path: clone.path().to_owned(),
        session: None,
    })
    .await;
    beta.eventually(
        |status| status.channels.iter().any(|c| c == "#api"),
        "joining",
    )
    .await;

    // The clone has no remote to fetch from: its default branch is where the worktree began.
    git(
        &login_root,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    let fork = std::process::Command::new("git")
        .args(["merge-base", "HEAD", "origin/main"])
        .current_dir(&login_root)
        .output()
        .unwrap();
    let fork = String::from_utf8(fork.stdout).unwrap().trim().to_owned();
    assert_eq!(fork.len(), 40, "the worktree left origin/main somewhere");
    let fork = fork[..zirc_proto::BASE_DIGITS].to_owned();
    // Committed, with one more change not yet: nothing of the commit's is uncommitted, so
    // only what the machine says of its commits tells others of login.rs.
    std::fs::write(login_root.join("login.rs"), "fn login() {}\n").unwrap();
    git(&login_root, &["add", "."]);
    git(&login_root, &["commit", "--quiet", "-m", "Add login"]);
    std::fs::write(login_root.join("logout.rs"), "fn logout() {}\n").unwrap();
    let want = (
        Some("login".to_owned()),
        Some("cc-login".to_owned()),
        fork,
        vec!["login.rs".to_owned(), "logout.rs".to_owned()],
    );
    // As the other machine has it, and as the worktree's own machine does.
    for (machine, who) in [(&beta, "beta"), (&solo, "solo")] {
        tokio::time::timeout(WAIT, async {
            loop {
                let Response::Who { channels } = machine.request(Request::Who).await else {
                    panic!("not a who")
                };
                let built: Vec<_> = channels
                    .into_iter()
                    .flat_map(|channel| channel.built)
                    .filter(|built| {
                        built.nick == "solo" && built.branch.as_deref() == Some("login")
                    })
                    .map(|built| (built.branch, built.session, built.base, built.changed))
                    .collect();
                if built == [want.clone()] {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{who} seeing what solo's login worktree is built on"));
    }
    drop(solo);
}

#[tokio::test]
async fn others_see_each_worktrees_changes_by_branch() {
    let ergo = require_ergo!();
    let (solo, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let beta = Machine::start(&ergo, "beta").await;
    let clone = clone();
    beta.request(Request::RepoAdd {
        path: clone.path().to_owned(),
        session: None,
    })
    .await;
    beta.eventually(
        |status| status.channels.iter().any(|c| c == "#api"),
        "joining",
    )
    .await;

    std::fs::write(main_root.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(login_root.join("login.rs"), "fn login() {}\n").unwrap();
    let solos = || async {
        let Response::Who { channels } = beta.request(Request::Who).await else {
            panic!("not a who")
        };
        channels
            .into_iter()
            .flat_map(|channel| channel.members)
            .filter(|member| member.nick == "solo")
            .map(|member| (member.branch, member.session, member.paths))
            .collect::<Vec<_>>()
    };
    let want = vec![
        (
            Some("login".to_owned()),
            Some("cc-login".to_owned()),
            vec!["login.rs".to_owned()],
        ),
        (
            Some("main".to_owned()),
            Some("cc-main".to_owned()),
            vec!["main.rs".to_owned()],
        ),
    ];
    tokio::time::timeout(WAIT, async {
        loop {
            let mut got = solos().await;
            got.sort();
            if got == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("beta seeing solo's worktrees by branch");
    drop(solo);
}

#[tokio::test]
async fn a_session_whose_worktree_is_removed_hears_how_to_be_watched_again() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut login_hears = machine.watch(Some("cc-login")).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    // The main checkout's own change: a path the gone worktree's path has in it too.
    std::fs::write(main_root.join("worktrees"), "notes\n").unwrap();
    until_who(
        &machine,
        |channels| {
            channels
                .iter()
                .flat_map(|c| &c.members)
                .any(|m| m.paths.iter().any(|p| p == "worktrees"))
        },
        "cc-main changing worktrees",
    )
    .await;

    git(
        &main_root,
        &["worktree", "remove", "--force", ".claude/worktrees/login"],
    );
    // Touched, so the next scan finds it gone.
    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    let (body, why) = loop {
        let (body, why) = next_post_why(&mut login_hears).await;
        if body.contains("is gone") {
            break (body, why);
        }
    };
    assert_eq!(
        body,
        format!(
            "cc-login: {} is gone, so it's no longer watched for you; run `zirc repo add \
             --session cc-login` where you work now",
            login_root.display()
        )
    );
    // Housekeeping: it waits for the session's next wake rather than waking it.
    assert_eq!(why, None);
    // And it's no news to another session, whatever paths its words share with that one's.
    let (_, why) = loop {
        let (body, why) = next_post_why(&mut main_hears).await;
        if body.contains("is gone") {
            break (body, why);
        }
    };
    assert_eq!(why, None, "the line names cc-login, not cc-main");
}

/// Waits until zircd's watched repositories are as `want` says.
async fn until_repos(
    machine: &Machine,
    want: impl Fn(&[zirc_core::ipc::Repo]) -> bool,
    what: &str,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Response::Repos { repos } = machine.request(Request::RepoList).await
                && want(&repos)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

/// Adds a worktree on a new branch `name` to the main checkout at `main_root`: its root.
fn add_worktree(main_root: &std::path::Path, name: &str) -> PathBuf {
    let at = format!(".claude/worktrees/{name}");
    git(main_root, &["worktree", "add", "--quiet", "-b", name, &at]);
    main_root.join(at).canonicalize().unwrap()
}

/// How many sessions are added with the watched repository at `root`: `None` when it isn't.
fn sessions_at(root: &std::path::Path, repos: &[zirc_core::ipc::Repo]) -> Option<usize> {
    repos
        .iter()
        .find(|repo| repo.path == root)
        .map(|repo| repo.sessions.len())
}

#[tokio::test]
async fn a_new_worktree_of_a_watched_checkout_is_watched_by_itself() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;
    let fresh = add_worktree(&main_root, "fresh");
    // For no session until one adds itself there.
    until_repos(
        &machine,
        |repos| sessions_at(&fresh, repos) == Some(0),
        "the new worktree isn't watched",
    )
    .await;
}

#[tokio::test]
async fn a_worktree_a_person_stops_watching_stays_unwatched_till_added() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    machine
        .request(Request::RepoRemove {
            path: login_root.clone(),
            session: None,
        })
        .await;
    // Once a worktree made after that is watched, zircd has looked again since.
    let later = add_worktree(&main_root, "later");
    until_repos(
        &machine,
        |repos| sessions_at(&later, repos).is_some(),
        "the later worktree isn't watched",
    )
    .await;
    let Response::Repos { repos } = machine.request(Request::RepoList).await else {
        panic!("repos");
    };
    assert_eq!(sessions_at(&login_root, &repos), None, "watched by itself");

    // Added by hand, it's watched again.
    machine
        .request(Request::RepoAdd {
            path: login_root.clone(),
            session: Some("cc-login".into()),
        })
        .await;
    until_repos(
        &machine,
        |repos| sessions_at(&login_root, repos) == Some(1),
        "the worktree added again isn't watched",
    )
    .await;
}

#[tokio::test]
async fn a_session_moves_between_worktrees_of_a_repository_without_a_remote() {
    let ergo = require_ergo!();
    let machine = Machine::start(&ergo, "solo").await;
    let main = clone();
    git(main.path(), &["remote", "remove", "origin"]);
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
    let main_root = main.path().canonicalize().unwrap();
    let login_root = main_root.join(".claude/worktrees/login");
    for path in [&main_root, &login_root] {
        machine
            .request(Request::RepoAdd {
                path: path.clone(),
                session: Some("cc-1".into()),
            })
            .await;
    }
    let Response::Repos { repos } = machine.request(Request::RepoList).await else {
        panic!("repos");
    };
    let sessions = |root: &PathBuf| {
        repos
            .iter()
            .find(|repo| repo.path == *root)
            .map(|repo| repo.sessions.len())
    };
    assert_eq!(sessions(&main_root), Some(0), "{repos:?}");
    assert_eq!(sessions(&login_root), Some(1), "{repos:?}");
}
