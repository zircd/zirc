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

    // Each hears of its own worktree's, with the other machine: whether with their lines
    // is another test's.
    let told = |overlap: Overlap, clone: &tempfile::TempDir| {
        let here = overlap.worktree.map(|worktree| worktree.path);
        assert_eq!(here, clone.path().canonicalize().ok());
        (overlap.nick, overlap.paths, overlap.why)
    };
    let expected = |nick: &str| (nick.to_owned(), readme(), Some(Why::Overlap));
    let heard = next_overlap(&mut beta_hears).await;
    assert_eq!(told(heard, &beta_clone), expected("alpha"));
    let heard = next_overlap(&mut alpha_hears).await;
    assert_eq!(told(heard, &alpha_clone), expected("beta"));

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

fn readme() -> Vec<String> {
    vec!["README.md".to_owned()]
}

#[tokio::test]
async fn another_machines_change_wakes_a_session_only_once_their_lines_meet() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    let _remote = share_lines(&alpha_clone, &beta_clone);
    let (theirs, mine) = (alpha_clone.path(), beta_clone.path());
    let session = Some("cc-b".to_owned());
    let path = mine.to_owned();
    beta.request(Request::RepoAdd { path, session }).await;
    let mut cc_b = beta.watch(Some("cc-b")).await;
    let _ = &alpha;

    // Different parts of a file both have: they merge, and it's told as no news to it.
    std::fs::write(theirs.join("lines.txt"), ten_lines(1, "alpha's first")).unwrap();
    std::fs::write(mine.join("lines.txt"), ten_lines(10, "beta's last")).unwrap();
    let apart = next_overlap(&mut cc_b).await;
    assert_eq!(apart.apart, ["lines.txt"], "{apart:?}");
    assert_eq!(apart.why, None);

    // The line alpha changes: told again, with the lines and the session to ask.
    std::fs::write(mine.join("lines.txt"), ten_lines(1, "beta's first")).unwrap();
    let same = next_overlap(&mut cc_b).await;
    assert_eq!(same.same_lines.keys().collect::<Vec<_>>(), ["lines.txt"]);
    assert_eq!(
        (same.nick.as_str(), &same.sessions[..]),
        ("alpha", &["watcher".to_owned()][..])
    );
    assert_eq!(same.why, Some(Why::Overlap));
}

#[tokio::test]
async fn machines_without_a_commit_in_common_compare_lines_as_each_counts_them_and_say_so() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    let _ = &alpha;
    let (theirs, mine) = (alpha_clone.path(), beta_clone.path());
    // Alpha is built on a commit beta has never fetched: its lines are of that one.
    std::fs::write(theirs.join("alpha.md"), "alpha only\n").unwrap();
    git(theirs, &["add", "alpha.md"]);
    git(theirs, &["commit", "--quiet", "--message", "Alpha's"]);
    for clone in [theirs, mine] {
        git(clone, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    }
    let session = Some("cc-b".to_owned());
    let path = mine.to_owned();
    beta.request(Request::RepoAdd { path, session }).await;
    let mut cc_b = beta.watch(Some("cc-b")).await;

    std::fs::write(theirs.join("README.md"), "alpha's take\n").unwrap();
    std::fs::write(mine.join("README.md"), "beta's take\n").unwrap();
    let rough = loop {
        let overlap = next_overlap(&mut cc_b).await;
        if !overlap.same_lines.is_empty() {
            break overlap;
        }
    };
    assert_eq!(rough.same_lines.keys().collect::<Vec<_>>(), ["README.md"]);
    assert!(rough.no_common_base, "{rough:?}");
    assert_eq!(
        rough.why,
        Some(Why::Overlap),
        "it wakes, as lines that meet do"
    );
}

#[tokio::test]
async fn two_clones_on_one_machine_are_told_whether_their_lines_meet() {
    let ergo = require_ergo!();
    let machine = Machine::start(&ergo, "solo").await;
    let (first, second) = (clone(), clone());
    let _remote = share_lines(&first, &second);
    for (clone, session) in [(&first, "cc-1"), (&second, "cc-2")] {
        let path = clone.path().to_owned();
        let session = Some(session.to_owned());
        machine.request(Request::RepoAdd { path, session }).await;
    }
    let mut hears = machine.watch(None).await;

    // Different parts of a file both have: Git can't rebase one clone's tree onto the
    // other's, so the lines each changes are compared.
    std::fs::write(first.path().join("lines.txt"), ten_lines(1, "first's")).unwrap();
    std::fs::write(second.path().join("lines.txt"), ten_lines(10, "second's")).unwrap();
    let apart = next_local_overlap(&mut hears).await;
    assert_eq!(apart.apart, ["lines.txt"], "{apart:?}");

    // Then the same line.
    std::fs::write(second.path().join("lines.txt"), ten_lines(1, "second's")).unwrap();
    let same = next_local_overlap(&mut hears).await;
    assert_eq!(same.same_lines.keys().collect::<Vec<_>>(), ["lines.txt"]);
}

#[tokio::test]
async fn worktrees_whose_changes_come_to_meet_are_told_again() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    std::fs::write(main_root.join("lines.txt"), ten_lines(0, "")).unwrap();
    git(&main_root, &["add", "lines.txt"]);
    git(&main_root, &["commit", "--quiet", "--message", "Lines"]);
    git(&login_root, &["merge", "--quiet", "--ff-only", "main"]);
    // As far as the repository knows, origin's main: what each worktree's lines are of.
    git(
        &main_root,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    let mut hears = machine.watch(None).await;
    let mut login_hears = machine.watch(Some("cc-login")).await;

    // Different parts of the file: told, to the person here.
    std::fs::write(main_root.join("lines.txt"), ten_lines(1, "main's")).unwrap();
    std::fs::write(login_root.join("lines.txt"), ten_lines(10, "login's")).unwrap();
    let apart = next_local_overlap(&mut hears).await;
    assert_eq!(apart.apart, ["lines.txt"], "{apart:?}");

    // Its change reaches main's line: told again, and the session wakes for it, the first
    // it's woken with.
    std::fs::write(login_root.join("lines.txt"), ten_lines(1, "login's")).unwrap();
    let same = loop {
        let overlap = next_local_overlap(&mut login_hears).await;
        if overlap.why.is_some() {
            break overlap;
        }
    };
    assert_eq!(same.same_lines.keys().collect::<Vec<_>>(), ["lines.txt"]);
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
                Heard::Overlap(pushed) if pushed.pushed.is_some() => return pushed,
                other => meanwhile.push(format!("{other:?}")),
            }
        }
    })
    .await;
    let Ok(conflict) = conflict else {
        let who = who(&machine).await;
        panic!("no conflict ahead; heard instead: {meanwhile:#?}; who: {who:#?}")
    };
    assert_eq!(conflict.pushed.as_deref(), Some("main"));
    assert_eq!(
        conflict.worktree.map(|worktree| worktree.path),
        Some(login_root)
    );
    assert_eq!(
        conflict.same_lines,
        [("lines.txt".to_owned(), vec![[3, 4]])].into()
    );
    assert_eq!(conflict.why, Some(Why::Overlap));

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
async fn a_commit_by_the_other_side_of_an_overlap_is_heard_and_wakes_nobody() {
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
    // It was told when their lines met, and hears when cc-login lands: a commit between the
    // two gives it nothing to do.
    assert_eq!(commit.why, None);
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
            agreed: zirc_proto::woke::Agreed::Order,
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
async fn two_sessions_that_settle_a_second_time_are_told_to_agree_who_continues() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, _login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    for file in ["README.md", "lines.txt"] {
        let settled = machine
            .request(Request::Settle {
                repo: main_root.clone(),
                session: "cc-main".into(),
                with: "cc-login".into(),
                first: "them".into(),
                paths: vec![main_root.join(file)],
                same_lines: true,
                agreed: zirc_proto::woke::Agreed::Order,
            })
            .await;
        assert!(
            matches!(settled, Response::Settled { said: true, .. }),
            "not settled: {settled:?}"
        );
    }
    // zircd's own line, naming it, wakes it: the other session gets one too.
    let told = loop {
        let delivery = next(&mut main_hears).await;
        if matches!(&delivery.message.event, Event::Text { body } if body.starts_with("cc-main: you and"))
        {
            break delivery;
        }
    };
    let Event::Text { body } = &told.message.event else {
        unreachable!()
    };
    assert!(
        body.starts_with("cc-main: you and cc-login keep meeting: you've settled 2 times"),
        "{body}"
    );
    assert!(told.why.is_some(), "it wakes the session it names");
}

/// cc-main's and cc-login's worktrees come to change the same line of three files both
/// have, and the person's watch has heard so of the last.
async fn meet_in_three_files(
    machine: &Machine,
    main_root: &std::path::Path,
    login_root: &std::path::Path,
) {
    let files = ["one.txt", "two.txt", "three.txt"];
    for file in files {
        std::fs::write(main_root.join(file), ten_lines(0, "")).unwrap();
    }
    // By name: cc-login's worktree is inside the checkout.
    git(main_root, &["add", "one.txt", "two.txt", "three.txt"]);
    git(
        main_root,
        &["commit", "--quiet", "--message", "Three files"],
    );
    git(login_root, &["merge", "--quiet", "--ff-only", "main"]);
    let mut hears = machine.watch(None).await;
    for file in files {
        std::fs::write(main_root.join(file), ten_lines(1, "main's")).unwrap();
        std::fs::write(login_root.join(file), ten_lines(1, "login's")).unwrap();
    }
    let mut met = std::collections::BTreeSet::new();
    while met.len() < files.len() {
        met.extend(next_local_overlap(&mut hears).await.same_lines.into_keys());
    }
}

fn keeps_meeting(heard: &Heard) -> bool {
    says(heard, "cc-main: you and cc-login keep meeting")
}

#[tokio::test]
async fn two_sessions_whose_lines_meet_in_three_files_are_told_they_keep_meeting() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    meet_in_three_files(&machine, &main_root, &login_root).await;
    let heard = until_marker(&mut main_hears, keeps_meeting).await;
    assert!(heard.iter().any(keeps_meeting));
}

#[tokio::test]
async fn a_pair_with_a_settle_standing_isnt_told_they_keep_meeting() {
    let ergo = require_ergo!();
    let (machine, _main, main_root, login_root) = machine_with_a_session_worktree(&ergo).await;
    let mut main_hears = machine.watch(Some("cc-main")).await;
    let settled = machine
        .request(Request::Settle {
            repo: main_root.clone(),
            session: "cc-main".into(),
            with: "cc-login".into(),
            first: "them".into(),
            paths: vec![main_root.join("README.md")],
            same_lines: true,
            agreed: zirc_proto::woke::Agreed::Order,
        })
        .await;
    assert!(
        matches!(settled, Response::Settled { said: true, .. }),
        "{settled:?}"
    );
    meet_in_three_files(&machine, &main_root, &login_root).await;
    // A word to it comes after whatever the meeting said.
    post(&machine, &login_root, "cc-login", "@cc-main a word").await;
    let heard = until_marker(&mut main_hears, |heard| says(heard, "a word")).await;
    assert!(!heard.iter().any(keeps_meeting), "{heard:?}");
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
            if let Heard::Overlap(pushed) = heard(&mut beta_hears).await
                && pushed.pushed.is_some()
            {
                return pushed;
            }
        }
    })
    .await
    .expect("a conflict ahead");
    assert_eq!(conflict.nick, "alpha");
    assert_eq!(conflict.pushed.as_deref(), Some("main"));
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
