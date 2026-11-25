//! Who is told when two sessions change the same files, and when a commit is news.

use std::collections::BTreeMap;
use zirc_core::ipc::{Heard, Overlap, Request, Response, Why};
use zirc_proto::Event;
use zirc_test_ergo::git::git;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn both_machines_hear_when_they_change_the_same_file() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    let mut alpha_hears = alpha.watch(None).await;
    let mut beta_hears = beta.watch(None).await;

    // Each touches the same file, and beta something of its own as well.
    std::fs::write(alpha_clone.path().join("README.md"), "alpha's take\n").unwrap();
    assert_eq!(next_working(&mut beta_hears, "alpha").await, ["README.md"]);
    std::fs::write(beta_clone.path().join("README.md"), "beta's take\n").unwrap();
    std::fs::write(beta_clone.path().join("notes.md"), "beta only\n").unwrap();

    let expected = |nick: &str| Overlap {
        channel: "#api".into(),
        nick: nick.into(),
        paths: vec!["README.md".into()],
        why: Some(Why::Overlap),
    };
    assert_eq!(next_overlap(&mut beta_hears).await, expected("alpha"));
    assert_eq!(next_overlap(&mut alpha_hears).await, expected("beta"));

    // `who` shows both working sets.
    let Response::Who { channels } = alpha.request(Request::Who).await else {
        panic!("not a who")
    };
    let members: Vec<(String, Vec<String>)> = channels[0]
        .members
        .iter()
        .map(|member| (member.nick.clone(), member.paths.clone()))
        .collect();
    assert_eq!(
        members,
        [
            ("alpha".to_owned(), vec!["README.md".to_owned()]),
            (
                "beta".to_owned(),
                vec!["README.md".to_owned(), "notes.md".to_owned()]
            ),
        ]
    );

    // Once beta commits, its working set no longer has the file. Earlier announcements may
    // still be queued, so wait for this one.
    git(
        beta_clone.path(),
        &["commit", "--quiet", "--all", "--message", "Beta's take"],
    );
    while next_working(&mut alpha_hears, "beta").await != ["notes.md"] {}
}

#[tokio::test]
async fn an_overlap_is_news_once() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    let mut beta_hears = beta.watch(None).await;
    let _ = &alpha;

    std::fs::write(alpha_clone.path().join("README.md"), "alpha's take\n").unwrap();
    assert_eq!(next_working(&mut beta_hears, "alpha").await, ["README.md"]);
    std::fs::write(beta_clone.path().join("README.md"), "beta's take\n").unwrap();
    assert_eq!(next_overlap(&mut beta_hears).await.paths, ["README.md"]);

    // Alpha's working set changes, and README.md still overlaps: not news. A second file
    // overlapping is, on its own.
    std::fs::write(alpha_clone.path().join("notes.md"), "alpha's notes\n").unwrap();
    assert_eq!(
        next_working(&mut beta_hears, "alpha").await,
        ["README.md", "notes.md"]
    );
    std::fs::write(beta_clone.path().join("README.md"), "beta's second take\n").unwrap();
    std::fs::write(beta_clone.path().join("notes.md"), "beta's notes\n").unwrap();
    assert_eq!(next_overlap(&mut beta_hears).await.paths, ["notes.md"]);
}

#[tokio::test]
async fn a_worktree_halfway_through_updating_overlaps_with_nobody() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (_beta, beta_clone)] = two_machines(&ergo).await;
    let mut alpha_hears = alpha.watch(None).await;

    // The remote's main has a newer README.md, which beta's worktree has too, though its
    // HEAD is older: what a worktree caught halfway through an update looks like. That's not
    // beta's work. Beta's notes are.
    let beta_repo = beta_clone.path();
    std::fs::write(beta_repo.join("README.md"), "api, version 2\n").unwrap();
    git(
        beta_repo,
        &["commit", "--quiet", "--all", "--message", "Version 2"],
    );
    git(
        beta_repo,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(beta_repo, &["reset", "--quiet", "HEAD~1"]);
    std::fs::write(beta_repo.join("notes.md"), "beta's notes\n").unwrap();
    // Beta may have been scanned on the way there; alpha starts once it's settled.
    until_who(
        &alpha,
        |channels| {
            channels.iter().any(|c| {
                c.members
                    .iter()
                    .any(|m| m.nick == "beta" && m.paths == ["notes.md"])
            })
        },
        "beta's notes alone",
    )
    .await;

    std::fs::write(alpha_clone.path().join("README.md"), "alpha's take\n").unwrap();
    std::fs::write(alpha_clone.path().join("notes.md"), "alpha's notes\n").unwrap();
    assert_eq!(next_overlap(&mut alpha_hears).await.paths, ["notes.md"]);
}

#[tokio::test]
async fn tells_worktrees_changing_the_same_lines_from_ones_that_will_merge() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    // A file both worktrees have, from the commit they share.
    std::fs::write(main_root.join("lines.txt"), ten_lines(0, "")).unwrap();
    git(&main_root, &["add", "lines.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Lines"]);
    git(&login_root, &["merge", "--quiet", "--ff-only", "main"]);
    let mut hears = machine.watch(None).await;

    std::fs::write(main_root.join("lines.txt"), ten_lines(1, "main's first")).unwrap();
    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    std::fs::write(login_root.join("lines.txt"), ten_lines(10, "login's last")).unwrap();
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    let (mut same_lines, mut apart) = (BTreeMap::new(), Vec::new());
    // The overlap may come in parts, as each worktree's scan lands.
    tokio::time::timeout(WAIT, async {
        while same_lines.is_empty() || apart.is_empty() {
            if let Heard::LocalOverlap(overlap) = heard(&mut hears).await {
                same_lines.extend(overlap.same_lines);
                apart.extend(overlap.apart);
            }
        }
    })
    .await
    .expect("the overlap");
    assert_eq!(same_lines, [("README.md".to_owned(), vec![[1, 1]])].into());
    assert_eq!(apart, ["lines.txt"]);
}

#[tokio::test]
async fn a_worktree_left_behind_by_its_branch_overlaps_only_with_its_own_work() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    // Both worktrees start from the default branch as it was.
    std::fs::write(main_root.join("lines.txt"), ten_lines(0, "")).unwrap();
    git(&main_root, &["add", "lines.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Lines"]);
    git(&login_root, &["merge", "--quiet", "--ff-only", "main"]);
    // A landing changes line 5, and main, cut after it, has it.
    std::fs::write(main_root.join("lines.txt"), ten_lines(5, "landed")).unwrap();
    git(
        &main_root,
        &["commit", "--quiet", "--all", "--message", "Landed"],
    );
    git(
        &main_root,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    // Login's branch is updated onto the landing, but its files stay as they were: the
    // landing shows there as undone, which is nobody's work. README.md is login's own.
    git(&login_root, &["reset", "--quiet", "--soft", "main"]);
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    let mut hears = machine.watch(None).await;

    // Main changes the landing's line, and README.md.
    std::fs::write(main_root.join("lines.txt"), ten_lines(5, "main's take")).unwrap();
    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    let overlapped = |heard: &[Heard]| -> Vec<String> {
        heard
            .iter()
            .filter_map(|heard| match heard {
                Heard::LocalOverlap(overlap) => Some(overlap.paths.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    };
    let touches = |heard: &Heard, path: &str| matches!(heard, Heard::LocalOverlap(overlap) if overlap.paths.iter().any(|p| p == path));
    let mut paths =
        overlapped(&until_marker(&mut hears, |heard| touches(heard, "README.md")).await);
    // Then both change a file of their own, a sentinel: a scan that says it has looked at
    // lines.txt again too, so had lines.txt met, it would have been said by then.
    std::fs::write(main_root.join("marker.txt"), "main's\n").unwrap();
    std::fs::write(login_root.join("marker.txt"), "login's\n").unwrap();
    paths.extend(overlapped(
        &until_marker(&mut hears, |heard| touches(heard, "marker.txt")).await,
    ));
    paths.retain(|path| path != "marker.txt");
    paths.dedup();
    assert_eq!(paths, ["README.md"]);

    // Login changing the landing's line itself does meet main's change.
    std::fs::write(login_root.join("lines.txt"), ten_lines(5, "login's take")).unwrap();
    let overlap = tokio::time::timeout(WAIT, async {
        loop {
            if let Heard::LocalOverlap(overlap) = heard(&mut hears).await
                && overlap.paths.iter().any(|path| path == "lines.txt")
            {
                return overlap;
            }
        }
    })
    .await
    .expect("the overlap in lines.txt");
    assert_eq!(overlap.same_lines["lines.txt"], [[5, 5]]);
}

#[tokio::test]
async fn warns_a_worktree_of_a_push_that_changes_the_same_lines() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("ZIRC_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    std::fs::write(main_root.join("lines.txt"), ten_lines(0, "")).unwrap();
    git(&main_root, &["add", "lines.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Lines"]);
    git(&login_root, &["merge", "--quiet", "--ff-only", "main"]);
    // A remote both worktrees push to.
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    let url = remote.path().to_str().unwrap();
    git(&main_root, &["remote", "set-url", "origin", url]);
    git(&main_root, &["push", "--quiet", "origin", "main"]);
    let mut login_hears = machine.watch(Some("cc-login")).await;

    // The login session changes line 3 and hasn't pushed; main changes line 4 and pushes.
    std::fs::write(login_root.join("lines.txt"), ten_lines(3, "login's line")).unwrap();
    std::fs::write(main_root.join("lines.txt"), ten_lines(4, "main's line")).unwrap();
    git(
        &main_root,
        &["commit", "--quiet", "--all", "--message", "Line 4"],
    );
    git(&main_root, &["push", "--quiet", "origin", "main"]);

    // What it heard meanwhile and what `who` says, if it never does: this flaked on Linux CI
    // with only "a conflict: Elapsed" to go on.
    let mut meanwhile = Vec::new();
    let conflict = tokio::time::timeout(WAIT, async {
        loop {
            match heard(&mut login_hears).await {
                Heard::Conflict(conflict) => return conflict,
                other => meanwhile.push(format!("{other:?}")),
            }
        }
    })
    .await;
    let Ok(conflict) = conflict else {
        let who = who(&machine).await;
        panic!("no conflict ahead; heard instead: {meanwhile:#?}; who: {who:#?}")
    };
    assert_eq!(conflict.branch, "main");
    assert_eq!(conflict.worktree.path, login_root);
    assert_eq!(
        conflict.same_lines,
        [("lines.txt".to_owned(), vec![[3, 4]])].into()
    );
    assert_eq!(conflict.why, Some(Why::Conflict));

    // And `who` lists it among what's landed.
    until_who(
        &machine,
        |channels| {
            channels.iter().any(|c| {
                c.landings
                    .iter()
                    .any(|l| l.branch == "main" && l.subjects.contains(&"Line 4".to_owned()))
            })
        },
        "the landing",
    )
    .await;
}

#[tokio::test]
async fn an_overlap_a_session_agreed_on_by_naming_the_other_stays_quiet_for_it() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    let mut login_hears = machine.watch(Some("cc-login")).await;
    // cc-main settled a split of README.md with cc-login, and says so naming it.
    machine
        .request(Request::Intend {
            repo: main_root.clone(),
            session: "cc-main".into(),
            summary: "README.md split with cc-login: I take the top".into(),
            paths: vec!["README.md".into()],
            seconds: 600,
        })
        .await;
    until_who(
        &machine,
        |channels| channels.iter().any(|c| !c.intents.is_empty()),
        "the intent",
    )
    .await;

    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    assert_eq!(
        next_local_overlap(&mut main_hears).await.why,
        None,
        "cc-main agreed on it"
    );
    assert_eq!(
        next_local_overlap(&mut login_hears).await.why,
        Some(Why::Overlap),
        "cc-login didn't say so"
    );
}

#[tokio::test]
async fn a_session_hears_when_the_other_side_of_its_overlap_commits() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    assert_eq!(
        next_local_overlap(&mut main_hears).await.why,
        Some(Why::Overlap)
    );

    git(
        &login_root,
        &["commit", "--quiet", "--all", "--message", "Login's take"],
    );
    let commit = loop {
        let delivery = next(&mut main_hears).await;
        if matches!(delivery.message.event, Event::Commit { .. }) {
            break delivery;
        }
    };
    assert_eq!(commit.message.session, "cc-login");
    assert_eq!(commit.why, Some(Why::Overlap));
}

#[tokio::test]
async fn a_settled_pair_wakes_for_neither_the_others_commits_nor_another_file_they_share() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    std::fs::write(main_root.join("README.md"), "main's take\n").unwrap();
    std::fs::write(login_root.join("README.md"), "login's take\n").unwrap();
    assert_eq!(
        next_local_overlap(&mut main_hears).await.why,
        Some(Why::Overlap)
    );
    // They settle README.md: cc-login lands first.
    let settled = machine
        .request(Request::Settle {
            repo: main_root.clone(),
            session: "cc-main".into(),
            with: "cc-login".into(),
            first: "them".into(),
            paths: vec![main_root.join("README.md")],
            same_lines: true,
        })
        .await;
    assert!(
        matches!(settled, Response::Settled { said: true, .. }),
        "not settled: {settled:?}"
    );
    loop {
        let delivery = next(&mut main_hears).await;
        if zirc_proto::woke::Settled::of(&delivery.message.event).is_some() {
            break;
        }
    }

    // cc-login's commit is heard, and wakes nobody: cc-main waits for its landing.
    git(
        &login_root,
        &["commit", "--quiet", "--all", "--message", "Login's take"],
    );
    let commit = loop {
        let delivery = next(&mut main_hears).await;
        if matches!(delivery.message.event, Event::Commit { .. }) {
            break delivery;
        }
    };
    assert_eq!(commit.message.session, "cc-login");
    assert_eq!(commit.why, None, "the two settled who lands first");

    // Nor does a file the settle never named, where they meet next.
    std::fs::write(main_root.join("lines.txt"), "main's lines\n").unwrap();
    std::fs::write(login_root.join("lines.txt"), "login's lines\n").unwrap();
    let next_file = loop {
        let overlap = next_local_overlap(&mut main_hears).await;
        if overlap.paths.iter().any(|path| path == "lines.txt") {
            break overlap;
        }
    };
    assert_eq!(next_file.why, None, "settled wherever they meet");
}

#[tokio::test]
async fn a_commit_in_other_parts_of_your_files_is_no_news() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    std::fs::write(main_root.join("lines.txt"), ten_lines(0, "")).unwrap();
    git(&main_root, &["add", "lines.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Lines"]);
    git(&login_root, &["merge", "--quiet", "--ff-only", "main"]);
    let mut hears = machine.watch(None).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    std::fs::write(main_root.join("lines.txt"), ten_lines(1, "main's first")).unwrap();
    std::fs::write(login_root.join("lines.txt"), ten_lines(10, "login's last")).unwrap();
    // Compared, the two change different parts of the file.
    tokio::time::timeout(WAIT, async {
        loop {
            if let Heard::LocalOverlap(overlap) = heard(&mut hears).await
                && overlap.apart.contains(&"lines.txt".to_owned())
            {
                break;
            }
        }
    })
    .await
    .expect("the overlap, apart");

    git(
        &login_root,
        &["commit", "--quiet", "--all", "--message", "Login's last"],
    );
    let commit = loop {
        let delivery = next(&mut main_hears).await;
        if matches!(delivery.message.event, Event::Commit { .. })
            && delivery.message.session == "cc-login"
        {
            break delivery;
        }
    };
    assert_eq!(commit.why, None, "its lines and cc-main's merge");
}

#[tokio::test]
async fn another_machine_hears_of_a_conflict_ahead_without_the_pushed_commits() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("ZIRC_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    // A remote both push to, and a file both have from it.
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    let url = remote.path().to_str().unwrap();
    std::fs::write(alpha_clone.path().join("lines.txt"), ten_lines(0, "")).unwrap();
    git(alpha_clone.path(), &["add", "lines.txt"]);
    git(
        alpha_clone.path(),
        &["commit", "--quiet", "--message", "Lines"],
    );
    for clone in [&alpha_clone, &beta_clone] {
        git(clone.path(), &["remote", "set-url", "origin", url]);
    }
    let mut alpha_hears = alpha.watch(None).await;
    git(alpha_clone.path(), &["push", "--quiet", "origin", "main"]);
    // Once alpha has told of that push, the next one is counted from it.
    while !matches!(
        next(&mut alpha_hears).await.message.event,
        Event::Push { .. }
    ) {}
    git(beta_clone.path(), &["fetch", "--quiet", "origin"]);
    git(
        beta_clone.path(),
        &["checkout", "--quiet", "-B", "main", "origin/main"],
    );
    let mut beta_hears = beta.watch(None).await;

    // Beta changes line 4 and hasn't pushed; alpha changes it too and pushes. Beta never
    // fetches, so it has only what the push says.
    std::fs::write(beta_clone.path().join("lines.txt"), ten_lines(4, "beta's")).unwrap();
    std::fs::write(
        alpha_clone.path().join("lines.txt"),
        ten_lines(4, "alpha's"),
    )
    .unwrap();
    git(
        alpha_clone.path(),
        &["commit", "--quiet", "--all", "--message", "Line 4"],
    );
    git(alpha_clone.path(), &["push", "--quiet", "origin", "main"]);

    let conflict = tokio::time::timeout(WAIT, async {
        loop {
            if let Heard::Conflict(conflict) = heard(&mut beta_hears).await {
                return conflict;
            }
        }
    })
    .await
    .expect("a conflict ahead");
    assert_eq!(conflict.nick, "alpha");
    assert_eq!(conflict.branch, "main");
    assert_eq!(
        conflict.same_lines,
        [("lines.txt".to_owned(), vec![[4, 4]])].into()
    );
}

#[tokio::test]
async fn a_commit_elsewhere_touching_your_files_is_news_before_it_lands() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    let _ = &alpha;
    beta.request(Request::RepoAdd {
        path: beta_clone.path().to_owned(),
        session: Some("cc-b".into()),
    })
    .await;
    std::fs::write(beta_clone.path().join("README.md"), "beta's take\n").unwrap();
    until_own_sets(
        &beta,
        "beta",
        &[own_set("main", Some("cc-b"), &["README.md"])],
        "beta's change",
    )
    .await;
    let mut cc_b = beta.watch(Some("cc-b")).await;

    std::fs::write(alpha_clone.path().join("README.md"), "alpha's take\n").unwrap();
    git(
        alpha_clone.path(),
        &["commit", "--quiet", "--all", "--message", "Alpha's take"],
    );
    let commit = loop {
        let delivery = next(&mut cc_b).await;
        if matches!(delivery.message.event, Event::Commit { .. }) {
            break delivery;
        }
    };
    assert_eq!(commit.from, "alpha");
    let Event::Commit { paths, .. } = &commit.message.event else {
        unreachable!()
    };
    assert_eq!(paths, &["README.md"]);
    assert_eq!(commit.why, Some(Why::Paths));
}
