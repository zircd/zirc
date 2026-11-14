//! `zirc stack`: building on another session's branch before it lands, rather than waiting
//! on it with `zirc wait-for`. zircd keeps the head built on, wakes the session's watch when
//! the branch lands or moves, and this says the rebase to run then: a landing rebases the
//! branch, so its commits come back with new hashes and only `git rebase --onto` past the
//! old head leaves them behind.

use std::io::Write;
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
    /// says what yours is stacked on, and the rebase to run for each that landed or moved.
    pub(crate) branch: Option<String>,
    /// Forget the stack on <BRANCH>.
    #[arg(long, requires = "branch")]
    pub(crate) drop: bool,
    /// Fail while your branch is stacked on one that hasn't landed, or that landed and you
    /// haven't rebased past: for a landing script to stop on.
    #[arg(long, conflicts_with_all = ["branch", "drop"])]
    pub(crate) check: bool,
    /// Saying what yours is stacked on, print JSON: an array on one line.
    #[arg(long, conflicts_with_all = ["drop", "check"])]
    pub(crate) json: bool,
    /// The session whose branch is stacked, whose watch is woken. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
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
        check,
        json,
        session,
        org,
    } = args;
    let session = session_watched(session, "stack")?;
    let org = org_here(socket, org).await?;
    let tip = match (&branch, drop) {
        (Some(branch), false) => Some(tip_of(branch)?),
        _ => None,
    };
    let request = Request::Stack {
        session: session.clone(),
        branch: branch.clone(),
        tip: tip.clone(),
        drop,
    };
    let Response::Stacks { stacks } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    if let (Some(branch), Some(tip)) = (&branch, &tip) {
        writeln!(
            out,
            "stacked on {branch} at {}: your watch wakes when it lands or moves, and `zirc \
             stack` then says the rebase to run. Land yours only after it has.",
            short(tip)
        )?;
        return Ok(());
    }
    if drop {
        writeln!(
            out,
            "not stacked on {} any more",
            branch.unwrap_or_default()
        )?;
        return Ok(());
    }
    if json {
        writeln!(out, "{}", serde_json::to_string(&stacks)?)?;
        return Ok(());
    }
    let mut blocking = Vec::new();
    let mut said = String::new();
    for stack in &stacks {
        match step(stack, has_commit(&stack.tip), &default_branch()) {
            Step::Done => {
                // Rebased past it: nothing's left to say of it.
                let forget = Request::Stack {
                    session: session.clone(),
                    branch: Some(stack.branch.clone()),
                    tip: None,
                    drop: true,
                };
                let _ = ask_in(socket, org.as_deref(), forget).await;
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
    } = stack;
    if *landed {
        if !has_tip {
            return Step::Done;
        }
        return Step::Say {
            line: format!(
                "{branch} landed: rebase onto {default} past the head you built on, `git fetch \
                 origin {default} && git rebase --onto origin/{default} {tip}`, then land"
            ),
            blocks: true,
        };
    }
    let line = match moved {
        Some(moved) => format!(
            "{branch} moved to {}: `git fetch origin {branch} && git rebase --onto \
             origin/{branch} {tip}`, then `zirc stack {branch}` again",
            short(moved)
        ),
        None => format!("{branch} at {}: not landed yet", short(tip)),
    };
    Step::Say { line, blocks: true }
}

/// A commit as people say it.
fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// `branch`'s head on origin, fetched now, which this branch must already hold.
fn tip_of(branch: &str) -> Result<String> {
    let fetched = git(&["fetch", "-q", "origin", branch])?;
    if !fetched.status.success() {
        bail!("couldn't fetch {branch} from origin: push it first, or name a branch that's there");
    }
    let parsed = git(&["rev-parse", "--verify", "-q", &format!("origin/{branch}")])?;
    let tip = String::from_utf8_lossy(&parsed.stdout).trim().to_owned();
    if !parsed.status.success() || tip.is_empty() {
        bail!("origin has no {branch}");
    }
    if !has_commit(&tip) {
        bail!(
            "your branch doesn't hold {branch}'s head yet: `git rebase origin/{branch}` first, \
             then stack again"
        );
    }
    Ok(tip)
}

/// Whether HEAD holds `commit`.
fn has_commit(commit: &str) -> bool {
    git(&["merge-base", "--is-ancestor", commit, "HEAD"]).is_ok_and(|out| out.status.success())
}

/// The repository's default branch, as origin's HEAD says; `master` when it doesn't.
fn default_branch() -> String {
    git(&["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"])
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

/// Runs git here.
fn git(args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
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
        }
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
        assert!(line.starts_with("api-auth moved to 9876543: "));
        assert!(line.contains("git rebase --onto origin/api-auth abcdef0123"));
        assert!(blocks);
        let Step::Say { line, blocks } = step(&stack(true, None), true, "main") else {
            panic!("a landing is said");
        };
        assert!(line.contains("git fetch origin main && git rebase --onto origin/main abcdef0123"));
        assert!(blocks);
        assert_eq!(step(&stack(true, None), false, "main"), Step::Done);
    }
}
