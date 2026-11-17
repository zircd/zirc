//! `zirc stack`: building on another session's branch before it lands, rather than waiting
//! on it with `zirc wait-for`. zircd keeps the head built on, wakes the session's watch when
//! the branch lands or moves, and this says the rebase to run then: a landing rebases the
//! branch, so its commits come back with new hashes and only `git rebase --onto` past the
//! old head leaves them behind. `--rebase` runs it.
//!
//! A session needn't say what it's stacked on: without a branch, this finds from git the
//! branches on origin its branch holds that haven't landed ([`inferred`]) and has zircd keep
//! them as if it had said so, so its watch wakes for them all the same.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use clap::Args;
use zirc_core::ipc::{Request, Response, Stack};

use crate::args::{OrgArg, session_watched};
use crate::client::{ask_in, org_here};

// `zirc stack`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct StackArgs {
    /// The branch, another session's not landed yet, that yours builds on. Without one, it
    /// says what yours is stacked on, those it finds from git too, and the rebase to run for
    /// each that landed or moved.
    pub(crate) branch: Option<String>,
    /// Forget the stack on <BRANCH>.
    #[arg(long, requires = "branch")]
    pub(crate) drop: bool,
    #[command(flatten)]
    pub(crate) mode: Mode,
    /// The session whose branch is stacked, whose watch is woken. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// What `zirc stack` without a branch does besides saying the stacks: one at most.
#[derive(Debug, Args)]
pub(crate) struct Mode {
    /// Fail while your branch is stacked on one that hasn't landed, or that landed and you
    /// haven't rebased past: for a landing script to stop on.
    #[arg(long, conflicts_with_all = ["branch", "drop"])]
    pub(crate) check: bool,
    /// Saying what yours is stacked on, print JSON: an array on one line.
    #[arg(long, conflicts_with_all = ["drop", "check"])]
    pub(crate) json: bool,
    /// Run the rebase for a branch yours is stacked on that landed or moved: onto the default
    /// branch, or the branch's new head, past the head you built on. Only with no uncommitted
    /// changes; it keeps your branch as it was in refs/zirc/backup/, and aborts on a conflict.
    #[arg(long, conflicts_with_all = ["branch", "drop", "check", "json"])]
    pub(crate) rebase: bool,
}

/// `zirc stack`.
pub(crate) async fn stack(
    socket: &std::path::Path,
    args: StackArgs,
    out: &mut impl Write,
) -> Result<()> {
    let StackArgs {
        branch,
        drop,
        mode: Mode {
            check,
            json,
            rebase,
        },
        session,
        org,
    } = args;
    let session = session_watched(session, "stack")?;
    let org = org_here(socket, org).await?;
    let zircd = Zircd {
        socket,
        org: org.as_deref(),
        session: &session,
    };
    if let Some(branch) = branch {
        return stack_on(&zircd, &branch, drop, out).await;
    }
    let here = Path::new(".");
    let default = default_branch(here);
    let mut stacks = zircd.ask(None, None, false).await?;
    // What git says it's stacked on, kept as if said: a session that never said `zirc stack`
    // is woken, and refused landing, as one that did.
    for (branch, tip) in inferred(here, &default, current_branch(here).as_deref()) {
        if stacks.iter().any(|stack| stack.branch == branch) {
            continue;
        }
        stacks = zircd.ask(Some(&branch), Some(&tip), false).await?;
        if !json {
            writeln!(
                out,
                "found from git: stacked on {branch} at {}",
                short(&tip)
            )?;
        }
    }
    if json {
        writeln!(out, "{}", serde_json::to_string(&stacks)?)?;
        return Ok(());
    }
    if rebase {
        return rebase_one(&zircd, here, &stacks, &default, out).await;
    }
    say_stacks(&zircd, &stacks, &default, check, out).await
}

/// The session's zircd, for its stacks.
struct Zircd<'a> {
    socket: &'a std::path::Path,
    org: Option<&'a str>,
    session: &'a str,
}

impl Zircd<'_> {
    /// Notes, forgets or (with neither) says the session's stacks; each way, all of them now.
    async fn ask(&self, branch: Option<&str>, tip: Option<&str>, drop: bool) -> Result<Vec<Stack>> {
        let request = Request::Stack {
            session: self.session.to_owned(),
            branch: branch.map(str::to_owned),
            tip: tip.map(str::to_owned),
            drop,
        };
        let Response::Stacks { stacks } = ask_in(self.socket, self.org, request).await? else {
            return Err(crate::client::unexpected());
        };
        Ok(stacks)
    }
}

/// `zirc stack <branch>`, or with `--drop`.
async fn stack_on(zircd: &Zircd<'_>, branch: &str, drop: bool, out: &mut impl Write) -> Result<()> {
    if drop {
        zircd.ask(Some(branch), None, true).await?;
        writeln!(out, "not stacked on {branch} any more")?;
        return Ok(());
    }
    let (tip, gone) = tip_of(branch)?;
    zircd.ask(Some(branch), Some(&tip), false).await?;
    if gone {
        writeln!(
            out,
            "{branch} is gone from origin: stacked on it at {}, as your clone last saw it. If \
             it landed, `zirc stack` says so now; if it was dropped, your watch wakes within \
             a minute or two with what to do. Don't land yours with its commits in it.",
            short(&tip)
        )?;
        return Ok(());
    }
    writeln!(
        out,
        "stacked on {branch} at {}: your watch wakes when it lands or moves, and `zirc stack \
         --rebase` then rebases yours past it. Land yours only after it has.",
        short(&tip)
    )?;
    Ok(())
}

/// Says each stack and what's to do about it, forgetting those rebased past; with `check`,
/// fails while one stops the branch landing.
async fn say_stacks(
    zircd: &Zircd<'_>,
    stacks: &[Stack],
    default: &str,
    check: bool,
    out: &mut impl Write,
) -> Result<()> {
    let mut blocking = Vec::new();
    let mut said = String::new();
    for stack in stacks {
        match step(stack, has_commit(Path::new("."), &stack.tip), default) {
            Step::Done => {
                // Rebased past it: nothing's left to say of it.
                let _ = zircd.ask(Some(&stack.branch), None, true).await;
            }
            Step::Say { line, blocks } => {
                said.push_str(&line);
                said.push('\n');
                if blocks {
                    blocking.push(stack.branch.clone());
                }
            }
        }
    }
    write!(out, "{said}")?;
    if check {
        if !blocking.is_empty() {
            bail!(
                "not landing: this branch is stacked on {}, as above",
                blocking.join(", ")
            );
        }
        return Ok(());
    }
    if said.is_empty() {
        writeln!(out, "stacked on nothing")?;
    }
    Ok(())
}

/// What's to do about a stack.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// The branch landed and this one has been rebased past its old head: forget it.
    Done,
    /// Say `line`; `blocks` while this branch can't land yet.
    Say { line: String, blocks: bool },
}

/// What's to do about `stack`, this branch still holding its head (`has_tip`) or not, the
/// repository's default branch being `default`.
fn step(stack: &Stack, has_tip: bool, default: &str) -> Step {
    let Stack {
        branch,
        tip,
        landed,
        moved,
        dropped,
    } = stack;
    if *dropped {
        if !has_tip {
            return Step::Done;
        }
        return Step::Say {
            line: format!(
                "{branch} was deleted from origin without landing: dropped, so it won't land, \
                 and don't land it yourself. Build yours on {default} without it: `zirc stack \
                 --rebase` rebases yours onto {default} past it (`git fetch origin {default} && \
                 git rebase --onto origin/{default} {tip}`); where yours used what {branch} \
                 added, do it again in yours as {default} has it"
            ),
            blocks: true,
        };
    }
    if *landed {
        if !has_tip {
            return Step::Done;
        }
        return Step::Say {
            line: format!(
                "{branch} landed: `zirc stack --rebase` rebases yours onto {default} past the \
                 head you built on (`git fetch origin {default} && git rebase --onto \
                 origin/{default} {tip}`), then land"
            ),
            blocks: true,
        };
    }
    let line = match moved {
        Some(moved) => format!(
            "{branch} moved to {}: `zirc stack --rebase` rebases yours onto it (`git fetch \
             origin {branch} && git rebase --onto origin/{branch} {tip}`)",
            short(moved)
        ),
        None => format!("{branch} at {}: not landed yet", short(tip)),
    };
    Step::Say { line, blocks: true }
}

/// A rebase `--rebase` runs: onto `onto` (a ref, such as `origin/master`), past `past`, the
/// head built on, which `branch` landed, moved or was dropped from.
#[derive(Debug, PartialEq, Eq)]
struct Move {
    branch: String,
    onto: String,
    past: String,
    /// Whether `branch` is gone from the stack once rebased past: landed, or dropped.
    landed: bool,
}

/// The rebases due for `stacks`, oldest first: each that landed or moved while this branch,
/// at `dir`, still holds the head built on.
fn moves(dir: &Path, stacks: &[Stack], default: &str) -> Vec<Move> {
    stacks
        .iter()
        .filter(|stack| has_commit(dir, &stack.tip))
        .filter_map(|stack| {
            let onto = if stack.landed || stack.dropped {
                format!("origin/{default}")
            } else {
                stack.moved.as_ref()?;
                format!("origin/{}", stack.branch)
            };
            Some(Move {
                branch: stack.branch.clone(),
                onto,
                past: stack.tip.clone(),
                landed: stack.landed || stack.dropped,
            })
        })
        .collect()
}

/// `zirc stack --rebase`: the first rebase due, then what zircd keeps of that stack (none once
/// it landed, the new head once it moved).
async fn rebase_one(
    zircd: &Zircd<'_>,
    dir: &Path,
    stacks: &[Stack],
    default: &str,
    out: &mut impl Write,
) -> Result<()> {
    let due = moves(dir, stacks, default);
    let Some(next) = due.first() else {
        writeln!(
            out,
            "nothing to rebase: no branch yours is stacked on has landed or moved"
        )?;
        return Ok(());
    };
    let done = rebase(dir, next)?;
    if next.landed {
        zircd.ask(Some(&next.branch), None, true).await?;
    } else {
        zircd
            .ask(Some(&next.branch), Some(&done.onto), false)
            .await?;
    }
    writeln!(
        out,
        "rebased onto {} past {}: {} commits, as they were; yours as it was is kept at {}",
        next.onto,
        short(&next.past),
        done.commits,
        done.backup
    )?;
    if !done.dropped.is_empty() {
        writeln!(out, "dropped as already there: {}", done.dropped.join(", "))?;
    }
    if due.len() > 1 {
        writeln!(out, "more to rebase: `zirc stack --rebase` again")?;
    } else if next.landed {
        writeln!(out, "now land it")?;
    }
    Ok(())
}

/// What a rebase did.
#[derive(Debug)]
struct Rebased {
    /// The commit it went onto.
    onto: String,
    /// How many of the branch's commits it rebased.
    commits: usize,
    /// Those it dropped, their subjects, as the commit they went onto already had them.
    dropped: Vec<String>,
    /// The ref keeping the branch as it was.
    backup: String,
}

/// Rebases the branch at `dir` as `next` says, safely: not with uncommitted changes or an
/// operation under way, its old head kept under refs/zirc/backup/ first, aborted on a
/// conflict, and refused afterwards, put back, if any commit came out other than it went in
/// (`git range-diff`): a rebase that silently changes what a branch holds is how a landing
/// undoes others' work.
fn rebase(dir: &Path, next: &Move) -> Result<Rebased> {
    refuse_unclean(dir)?;
    let fetch = next.onto.strip_prefix("origin/").unwrap_or(&next.onto);
    run(dir, &["fetch", "-q", "origin", fetch]).context("fetching what to rebase onto")?;
    let onto = rev(dir, &next.onto)?;
    let head = rev(dir, "HEAD")?;
    let backup = format!(
        "refs/zirc/backup/{}/{}",
        current_branch(dir).unwrap_or_else(|| "HEAD".to_owned()),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |at| at.as_secs())
    );
    run(dir, &["update-ref", &backup, &head])?;
    let rebased = git(dir, &["rebase", "-q", "--onto", &onto, &next.past])?;
    if !rebased.status.success() {
        let conflicted = lines(&git(dir, &["diff", "--name-only", "--diff-filter=U"])?);
        let _ = git(dir, &["rebase", "--abort"]);
        bail!(
            "the rebase onto {} conflicts in {}: your branch is as it was. Resolve it by hand: \
             git rebase --onto {} {}",
            next.onto,
            if conflicted.is_empty() {
                "something".to_owned()
            } else {
                conflicted.join(", ")
            },
            next.onto,
            next.past
        );
    }
    let compared = git(
        dir,
        &[
            "range-diff",
            "--no-color",
            &format!("{}..{head}", next.past),
            &format!("{onto}..HEAD"),
        ],
    )?;
    let (commits, dropped, changed) = compare(&lines(&compared));
    if !compared.status.success() || !changed.is_empty() {
        run(dir, &["reset", "-q", "--hard", &head])?;
        bail!(
            "the rebase changed {}, so your branch is put back as it was: rebase it by hand, \
             git rebase --onto {} {}",
            if changed.is_empty() {
                "what range-diff couldn't say".to_owned()
            } else {
                changed.join(", ")
            },
            next.onto,
            next.past
        );
    }
    Ok(Rebased {
        onto,
        commits,
        dropped,
        backup,
    })
}

/// From `git range-diff`'s lines (`1:  abc1234 = 1:  def5678 subject`): how many commits it
/// matched unchanged, the subjects of those dropped (`<`, already on what it went onto) and
/// of those changed or new (`!`, `>`).
fn compare(lines: &[String]) -> (usize, Vec<String>, Vec<String>) {
    let (mut same, mut dropped, mut changed) = (0, Vec::new(), Vec::new());
    for line in lines {
        let words: Vec<&str> = line.split_whitespace().collect();
        let (Some(mark), Some(subject)) = (words.get(2), words.get(5..)) else {
            continue;
        };
        let subject = subject.join(" ");
        match *mark {
            "=" => same += 1,
            "<" => dropped.push(subject),
            "!" | ">" => changed.push(subject),
            _ => {}
        }
    }
    (same, dropped, changed)
}

/// Refuses a rebase at `dir` with uncommitted changes, or a rebase, merge or cherry-pick
/// under way: either is someone's work in progress.
fn refuse_unclean(dir: &Path) -> Result<()> {
    let status = git(dir, &["status", "--porcelain", "--untracked-files=no"])?;
    if !status.stdout.is_empty() {
        bail!("not rebasing with uncommitted changes: commit or stash them first");
    }
    for under_way in [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
    ] {
        let path = lines(&git(dir, &["rev-parse", "--git-path", under_way])?);
        if path.first().is_some_and(|path| dir.join(path).exists()) {
            bail!("not rebasing while a {under_way} is under way: finish or abort it first");
        }
    }
    Ok(())
}

/// The branches on origin HEAD at `dir` is stacked on, `(branch, head)`, as git says
/// ([`zirc_core::stacked::stacked_on`]).
fn inferred(dir: &Path, default: &str, own: Option<&str>) -> Vec<(String, String)> {
    let Ok(head) = rev(dir, "HEAD") else {
        return Vec::new();
    };
    zirc_core::stacked::stacked_on(dir, default, own, &head)
}

/// A commit as people say it.
fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// `branch`'s head on origin, fetched now, which this branch must already hold, and whether
/// origin no longer has it: then its head as this clone last saw it, which zircd finds
/// landed or dropped. An agent that built on a branch deleted since was told to push it
/// first, kept no stack and waited on it for good (the agent evals' PL2, 17 Nov 2026).
fn tip_of(branch: &str) -> Result<(String, bool)> {
    let here = Path::new(".");
    let fetched = git(here, &["fetch", "-q", "origin", branch])?;
    let gone = !fetched.status.success();
    if gone && !rev(here, &format!("origin/{branch}")).is_ok_and(|tip| has_commit(here, &tip)) {
        bail!("couldn't fetch {branch} from origin: push it first, or name a branch that's there");
    }
    let Ok(tip) = rev(here, &format!("origin/{branch}")) else {
        bail!("origin has no {branch}");
    };
    if !has_commit(here, &tip) {
        bail!(
            "your branch doesn't hold {branch}'s head yet: `git rebase origin/{branch}` first, \
             then stack again"
        );
    }
    Ok((tip, gone))
}

/// Whether HEAD at `dir` holds `commit`.
fn has_commit(dir: &Path, commit: &str) -> bool {
    git(dir, &["merge-base", "--is-ancestor", commit, "HEAD"]).is_ok_and(|out| out.status.success())
}

/// The branch checked out at `dir`, if one is.
fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| lines(&out).into_iter().next())
}

/// The repository's default branch, as origin's HEAD says; `master` when it doesn't.
fn default_branch(dir: &Path) -> String {
    git(
        dir,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .filter(|out| out.status.success())
    .and_then(|out| {
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .strip_prefix("origin/")
            .map(str::to_owned)
    })
    .unwrap_or_else(|| "master".to_owned())
}

/// The commit `name` is at `dir`.
fn rev(dir: &Path, name: &str) -> Result<String> {
    let out = git(
        dir,
        &["rev-parse", "--verify", "-q", &format!("{name}^{{commit}}")],
    )?;
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || commit.is_empty() {
        bail!("no commit {name}");
    }
    Ok(commit)
}

/// An output's lines, trimmed, the empty ones left out.
fn lines(out: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Runs git at `dir`, failing, with what it said, when git does.
fn run(dir: &Path, args: &[&str]) -> Result<()> {
    let out = git(dir, args)?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Runs git at `dir`.
fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .context("running git")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(landed: bool, moved: Option<&str>) -> Stack {
        Stack {
            branch: "api-auth".into(),
            tip: "abcdef0123".into(),
            landed,
            moved: moved.map(str::to_owned),
            dropped: false,
        }
    }

    #[test]
    fn a_dropped_base_says_not_to_land_it_and_rebases_onto_the_default_branch() {
        let dropped = Stack {
            dropped: true,
            ..stack(false, None)
        };
        let Step::Say { line, blocks } = step(&dropped, true, "main") else {
            panic!("a dropped base still held says what to do");
        };
        assert!(blocks);
        assert!(
            line.starts_with("api-auth was deleted from origin without landing"),
            "{line}"
        );
        assert!(line.contains("don't land it yourself"), "{line}");
        assert_eq!(step(&dropped, false, "main"), Step::Done);
    }

    #[test]
    fn says_the_rebase_for_a_branch_that_landed_or_moved_and_forgets_one_rebased_past() {
        assert_eq!(
            step(&stack(false, None), true, "master"),
            Step::Say {
                line: "api-auth at abcdef0: not landed yet".into(),
                blocks: true
            }
        );
        let Step::Say { line, blocks } = step(&stack(false, Some("9876543210")), true, "master")
        else {
            panic!("a move is said");
        };
        assert!(line.starts_with("api-auth moved to 9876543: `zirc stack --rebase`"));
        assert!(line.contains("git rebase --onto origin/api-auth abcdef0123"));
        assert!(blocks);
        let Step::Say { line, blocks } = step(&stack(true, None), true, "main") else {
            panic!("a landing is said");
        };
        assert!(line.contains("`zirc stack --rebase`"));
        assert!(line.contains("git fetch origin main && git rebase --onto origin/main abcdef0123"));
        assert!(blocks);
        assert_eq!(step(&stack(true, None), false, "main"), Step::Done);
    }

    /// Runs git at `dir`, as a test's setup does: it must work.
    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = git(dir, args).unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A clone of a bare origin with master's first commit, in `root`.
    fn repo(root: &Path) -> std::path::PathBuf {
        let origin = root.join("origin.git");
        let work = root.join("work");
        sh(
            root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "master",
                origin.to_str().unwrap(),
            ],
        );
        sh(
            root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        sh(&work, &["config", "user.email", "t@example.com"]);
        sh(&work, &["config", "user.name", "t"]);
        sh(&work, &["symbolic-ref", "HEAD", "refs/heads/master"]);
        commit(&work, "m", "0");
        sh(&work, &["push", "-q", "origin", "master"]);
        work
    }

    /// A commit writing `text` to `file`, its subject `file`.
    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        sh(dir, &["add", file]);
        sh(dir, &["commit", "-qm", file]);
    }

    /// `branch` from `from` with a commit of its own to `file`, pushed.
    fn pushed(dir: &Path, branch: &str, from: &str, file: &str) {
        sh(dir, &["checkout", "-q", "-b", branch, from]);
        commit(dir, file, branch);
        sh(dir, &["push", "-q", "origin", branch]);
    }

    /// `branch` lands: its commits put onto master as new ones, as a landing's rebase does,
    /// onto another landing's: onto master alone, in the same second, a cherry-pick would
    /// make the very same commit.
    fn lands(dir: &Path, branch: &str) {
        sh(dir, &["checkout", "-q", "master"]);
        commit(dir, &format!("before-{branch}"), "another landing");
        sh(dir, &["cherry-pick", branch]);
        sh(dir, &["push", "-q", "origin", "master"]);
        sh(dir, &["fetch", "-q", "origin"]);
    }

    /// B built on A, A landed: B, rebased past A's old head, holds master and its own commit,
    /// and its old head is kept.
    #[test]
    fn rebases_past_a_base_that_landed_keeping_the_branch_as_it_was() {
        let root = tempfile::tempdir().unwrap();
        let work = repo(root.path());
        pushed(&work, "a", "master", "a");
        pushed(&work, "b", "a", "b");
        let (a, before) = (rev(&work, "a").unwrap(), rev(&work, "b").unwrap());
        lands(&work, "a");
        sh(&work, &["checkout", "-q", "b"]);
        let next = Move {
            branch: "a".into(),
            onto: "origin/master".into(),
            past: a.clone(),
            landed: true,
        };
        let done = rebase(&work, &next).unwrap();
        assert_eq!(done.commits, 1);
        assert_eq!(
            sh(&work, &["rev-parse", "HEAD~1"]),
            rev(&work, "origin/master").unwrap()
        );
        assert!(!has_commit(&work, &a), "a's old head is left behind");
        assert_eq!(rev(&work, &done.backup).unwrap(), before);
        assert!(done.backup.starts_with("refs/zirc/backup/b/"));
    }

    #[test]
    fn refuses_to_rebase_over_uncommitted_changes_and_aborts_a_conflict() {
        let root = tempfile::tempdir().unwrap();
        let work = repo(root.path());
        pushed(&work, "a", "master", "a");
        sh(&work, &["checkout", "-q", "-b", "b"]);
        commit(&work, "a", "b's a");
        let a = rev(&work, "a").unwrap();
        // a lands changed, as a landing that resolved it would: b's change to it conflicts.
        sh(&work, &["checkout", "-q", "master"]);
        commit(&work, "a", "landed a");
        sh(&work, &["push", "-q", "origin", "master"]);
        sh(&work, &["checkout", "-q", "b"]);
        let before = rev(&work, "HEAD").unwrap();
        let next = Move {
            branch: "a".into(),
            onto: "origin/master".into(),
            past: a,
            landed: true,
        };
        std::fs::write(work.join("m"), "changed").unwrap();
        let unclean = rebase(&work, &next).unwrap_err().to_string();
        assert!(unclean.contains("uncommitted"), "{unclean}");
        sh(&work, &["checkout", "-q", "--", "m"]);
        let conflict = rebase(&work, &next).unwrap_err().to_string();
        assert!(conflict.contains("conflicts in a"), "{conflict}");
        assert_eq!(rev(&work, "HEAD").unwrap(), before, "put back as it was");
        assert!(refuse_unclean(&work).is_ok(), "no rebase left under way");
    }

    #[test]
    fn reads_range_diff() {
        let said = [
            "1:  abc1234 = 1:  def5678 The first",
            "2:  bcd2345 < -:  ------- Already there",
            "3:  cde3456 ! 2:  efa6789 Changed by the rebase",
        ]
        .map(str::to_owned);
        let (same, dropped, changed) = compare(&said);
        assert_eq!(same, 1);
        assert_eq!(dropped, ["Already there"]);
        assert_eq!(changed, ["Changed by the rebase"]);
    }
}
