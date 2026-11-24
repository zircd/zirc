//! `zirc ci connect` and `zirc ci test-failure`: a repository's GitHub Actions told to say
//! how its builds went, in one command, and a test failure to see an agent wake.
//!
//! `connect` finds the workflows on the default branch, makes the organisation a CI token and
//! keeps it as the repository's secret ([`SECRET_NAME`]) with `gh`, adds the step that tells
//! Zirc ([`zirc_proto::ci::actions_step`]) to the end of each job on a branch of its own, opens
//! a pull request, and waits until a run is heard: what proves it works. It changes the
//! person's repository and its secrets, so it says what it will do and asks first.
//!
//! `test-failure` makes a real run fail on purpose (the user's choice, 24 Nov 2026: "Real run,
//! by re-run"): it names the branch in the repository variable [`TEST_VARIABLE`], re-runs the
//! branch's last run, whose step then says it failed, as a test, and clears the variable once
//! that's heard.

use std::io::{IsTerminal as _, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::Args;
use zirc_core::ipc::{self, Heard, Request};
use zirc_proto::Event;
use zirc_proto::ci::{
    CONNECT_BRANCH, CONNECT_COMMAND, LANDING_KIND, SECRET_NAME, SECRET_ONLY_COMMAND, STEP_NAME,
    TEST_FAILURE_COMMAND, TEST_VARIABLE,
};

use crate::ci::OrgArgs;

/// How long either command waits to hear a run.
const HEARD_WITHIN: Duration = Duration::from_mins(15);

#[derive(Debug, Args)]
pub struct ConnectArgs {
    /// Only set the secret again, with a new token: the fix when CI's posts are refused.
    #[arg(long)]
    secret_only: bool,
    /// Don't ask before changing the repository and its secrets.
    #[arg(long, short)]
    yes: bool,
    /// Anywhere in the repository; the current directory otherwise.
    #[arg(long, value_name = "PATH")]
    path: Option<PathBuf>,
    #[command(flatten)]
    org: OrgArgs,
}

#[derive(Debug, Args)]
pub struct TestFailureArgs {
    /// The session to wake: it waits on the branch from now on. Defaults to `$ZIRC_SESSION`.
    #[arg(long, env = "ZIRC_SESSION")]
    session: Option<String>,
    /// The branch whose last run to re-run; the one checked out otherwise.
    #[arg(long)]
    branch: Option<String>,
    /// Anywhere in the repository; the current directory otherwise.
    #[arg(long, value_name = "PATH")]
    path: Option<PathBuf>,
}

/// What `git` and `gh` are asked, so tests can answer for them.
pub(crate) trait Tools: Send {
    /// Runs `program` with `args` in `dir`, `input` on its stdin, and gives what it printed.
    ///
    /// # Errors
    ///
    /// When it can't be run or fails: with what it said.
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        dir: &Path,
        input: Option<&str>,
    ) -> Result<String>;
}

/// The real `git` and `gh`.
pub(crate) struct Real;

impl Tools for Real {
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        dir: &Path,
        input: Option<&str>,
    ) -> Result<String> {
        use std::process::{Command, Stdio};
        let mut child = Command::new(program)
            .args(args)
            .current_dir(dir)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| match program {
                "gh" => "GitHub's gh isn't installed: https://cli.github.com, then gh auth login"
                    .to_owned(),
                _ => format!("couldn't run {program}"),
            })?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(input.as_bytes())?;
        }
        let done = child.wait_with_output()?;
        if !done.status.success() {
            // Never the arguments' values: one may be a token's.
            bail!(
                "{program} {} failed: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&done.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&done.stdout).trim().to_owned())
    }
}

/// How many spaces `line` starts with.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Whether `line` says nothing: blank, or a comment.
fn silent(line: &str) -> bool {
    let line = line.trim_start();
    line.is_empty() || line.starts_with('#')
}

/// `workflow`, a GitHub Actions workflow's text, with `step` (as
/// [`zirc_proto::ci::actions_step`] gives it, its first line six spaces in) added to the end
/// of each job that has steps of its own and no such step yet, and those jobs' names. A job
/// that only calls another workflow (`uses:`) has no steps to add to. Read by indentation, as
/// people write workflows, without parsing YAML: nothing else in the file changes.
#[must_use]
pub(crate) fn with_step(workflow: &str, step: &str) -> (String, Vec<String>) {
    let lines: Vec<&str> = workflow.lines().collect();
    let Some(jobs_at) = lines.iter().position(|line| line.trim_end() == "jobs:") else {
        return (workflow.to_owned(), Vec::new());
    };
    // The jobs' own indentation: the first key under `jobs:`.
    let Some(job_indent) = lines[jobs_at + 1..]
        .iter()
        .find(|line| !silent(line))
        .map(|line| indent(line))
        .filter(|at| *at > 0)
    else {
        return (workflow.to_owned(), Vec::new());
    };
    let step_indent = indent(step);
    // Where to add the step, last first, so earlier places stay where they are.
    let mut adds: Vec<(usize, usize)> = Vec::new();
    let mut jobs = Vec::new();
    let mut at = jobs_at + 1;
    while at < lines.len() {
        let line = lines[at];
        if !silent(line) && indent(line) < job_indent {
            break;
        }
        if silent(line) || indent(line) != job_indent {
            at += 1;
            continue;
        }
        let name = line.trim().trim_end_matches(':').to_owned();
        let end = (at + 1..lines.len())
            .find(|&next| !silent(lines[next]) && indent(lines[next]) <= job_indent)
            .unwrap_or(lines.len());
        if let Some((after, items_at)) = steps_end(&lines[at + 1..end])
            && !lines[at..end].iter().any(|line| has_step(line))
        {
            adds.push((at + 1 + after, items_at));
            jobs.push(name);
        }
        at = end;
    }
    let mut out: Vec<String> = lines.iter().map(|&line| line.to_owned()).collect();
    for (after, items_at) in adds.into_iter().rev() {
        let added: Vec<String> = step
            .lines()
            .map(|line| {
                if line.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{}{}",
                        " ".repeat(items_at),
                        &line[step_indent.min(indent(line))..]
                    )
                }
            })
            .collect();
        for (n, line) in added.into_iter().enumerate() {
            out.insert(after + 1 + n, line);
        }
    }
    let mut text = out.join("\n");
    if workflow.ends_with('\n') {
        text.push('\n');
    }
    (text, jobs)
}

/// Whether `line` names the step that tells Zirc.
fn has_step(line: &str) -> bool {
    let line = line.trim().trim_start_matches("- ");
    line.strip_prefix("name:")
        .is_some_and(|name| name.trim().trim_matches(['"', '\'']) == STEP_NAME)
}

/// In a job's lines after its name: the last line of its steps, and how far in its steps'
/// items are; `None` when it has no steps written as a list under `steps:`.
fn steps_end(job: &[&str]) -> Option<(usize, usize)> {
    let steps = job
        .iter()
        .position(|line| line.trim_end().trim_start() == "steps:")?;
    let key_indent = indent(job[steps]);
    let first = (steps + 1..job.len()).find(|&at| !silent(job[at]))?;
    let items_at = indent(job[first]);
    if !job[first].trim_start().starts_with("- ") || items_at < key_indent {
        return None;
    }
    let mut last = first;
    for (at, line) in job.iter().enumerate().skip(first + 1) {
        if silent(line) {
            continue;
        }
        // Another key of the job ends its steps; an item at the key's own depth is still one.
        if indent(line) < items_at
            || (indent(line) == key_indent && !line.trim_start().starts_with("- "))
        {
            break;
        }
        last = at;
    }
    Some((last, items_at))
}

/// A workflow to change: its path in the repository, its new text and the jobs it changes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub path: String,
    pub text: String,
    pub jobs: Vec<String>,
}

/// The repository as GitHub knows it: `owner/name` and its default branch.
struct Remote {
    slug: String,
    default: String,
}

fn remote(tools: &mut dyn Tools, root: &Path) -> Result<Remote> {
    let told = tools
        .run(
            "gh",
            &[
                "repo",
                "view",
                "--json",
                "nameWithOwner,defaultBranchRef",
                "-q",
                ".nameWithOwner + \" \" + .defaultBranchRef.name",
            ],
            root,
            None,
        )
        .context(
            "gh couldn't say which GitHub repository this is: gh auth login, in a clone of one",
        )?;
    let (slug, default) = told
        .split_once(' ')
        .context("gh didn't name the repository and its default branch")?;
    Ok(Remote {
        slug: slug.to_owned(),
        default: default.to_owned(),
    })
}

/// The workflows on `branch` of origin, as fetched, that would change, each with the step added.
fn changes(
    tools: &mut dyn Tools,
    root: &Path,
    branch: &str,
    step: &str,
) -> Result<(Vec<Change>, usize)> {
    tools.run("git", &["fetch", "--quiet", "origin", branch], root, None)?;
    let tree = format!("origin/{branch}");
    let listed = tools
        .run(
            "git",
            &["ls-tree", "--name-only", &tree, ".github/workflows/"],
            root,
            None,
        )
        .unwrap_or_default();
    let mut found = 0;
    let mut changes = Vec::new();
    for path in listed.lines().filter(|path| is_workflow(path)) {
        found += 1;
        let mut text = tools.run("git", &["show", &format!("{tree}:{path}")], root, None)?;
        text.push('\n');
        let (text, jobs) = with_step(&text, step);
        if !jobs.is_empty() {
            changes.push(Change {
                path: path.to_owned(),
                text,
                jobs,
            });
        }
    }
    Ok((changes, found))
}

/// Whether `path` is a workflow's: YAML, by its extension.
fn is_workflow(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"))
}

/// A name for the repository's CI token among the organisation's: `github-<repository>`,
/// lowercase letters, digits and dashes, 32 at most.
fn token_name(slug: &str) -> String {
    let repository = slug.rsplit('/').next().unwrap_or(slug);
    let name: String = format!("github-{repository}")
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(32)
        .collect();
    name.trim_end_matches('-').to_owned()
}

/// Asks `question` on the terminal, unless `yes` says not to: refuses where nobody can answer.
fn confirm(yes: bool, question: &str, out: &mut impl Write) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("nobody to ask here: run it again with --yes to go ahead");
    }
    write!(out, "{question} [y/N] ")?;
    out.flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// The repository's root, from `path` or here.
fn root(tools: &mut dyn Tools, path: Option<PathBuf>) -> Result<PathBuf> {
    let here = crate::client::here(path)?;
    let root = tools
        .run("git", &["rev-parse", "--show-toplevel"], &here, None)
        .context("not in a Git repository")?;
    Ok(PathBuf::from(root))
}

/// What a run said of one job: its build, its job and how it went.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Said {
    build: String,
    job: String,
    state: String,
    test: bool,
}

/// What `heard` says of a run on `branch` that has ended, if it's CI's line on one.
fn said(heard: &Heard, branch: &str) -> Option<Said> {
    let Heard::Message(delivery) = heard else {
        return None;
    };
    let Event::Custom { kind, data, .. } = &delivery.message.event else {
        return None;
    };
    let ci = zirc_proto::ci::Data::of(data);
    let state = ci.state.clone()?;
    if kind != LANDING_KIND
        || ci.branch.as_deref() != Some(branch)
        || !zirc_proto::ci::speaks_for_ci(delivery.account.as_deref())
        || !matches!(state.as_str(), "passed" | "failed" | "cancelled")
    {
        return None;
    }
    Some(Said {
        build: ci.build.unwrap_or_default(),
        job: data
            .get("job")
            .and_then(|job| job.as_str())
            .unwrap_or_default()
            .to_owned(),
        state,
        test: ci.test,
    })
}

/// Waits until `enough` of what CI says of runs on `branch` is heard, and gives it; `None`
/// when [`HEARD_WITHIN`] passes first.
async fn hear(
    watching: &mut ipc::Watching,
    branch: &str,
    enough: impl Fn(&[Said]) -> bool,
) -> Result<Option<Vec<Said>>> {
    let limit = tokio::time::sleep(HEARD_WITHIN);
    tokio::pin!(limit);
    let mut all = Vec::new();
    loop {
        let next = tokio::select! {
            next = watching.next() => next?,
            () = &mut limit => return Ok(None),
        };
        let Some(heard) = next else {
            bail!("zircd stopped: zirc daemon start, then run it again");
        };
        if let Some(said) = said(&heard, branch) {
            all.push(said);
            if enough(&all) {
                return Ok(Some(all));
            }
        }
    }
}

/// `test passed, lint passed`.
fn outcome(said: &[Said]) -> String {
    let jobs: Vec<String> = said
        .iter()
        .map(|said| format!("{} {}", said.job, said.state).trim().to_owned())
        .collect();
    jobs.join(", ")
}

/// Bold, where `out` is a terminal's.
fn bold(text: &str) -> String {
    if crate::paint::wanted() {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Makes the organisation a new CI token for the repository and keeps it as its secret.
async fn set_secret(
    tools: &mut dyn Tools,
    home: &Path,
    socket: &Path,
    org: &OrgArgs,
    root: &Path,
    slug: &str,
    out: &mut (impl Write + Send),
) -> Result<()> {
    let team = crate::ci::team(home, socket, org).await?;
    let name = token_name(slug);
    // One of that name is this repository's from before: the new one takes its place.
    let _ = team.client.revoke_ci_token(&team.account, &name).await;
    let made = team
        .client
        .ci_token(&team.account, &name)
        .await?
        .context(crate::ci::SIGNED_OUT)?;
    tools
        .run(
            "gh",
            &["secret", "set", SECRET_NAME, "--repo", slug],
            root,
            Some(&made.token),
        )
        .with_context(|| format!("couldn't set the secret: it takes admin access to {slug}"))?;
    writeln!(out, "✓ Set the secret {SECRET_NAME} on {slug}, with gh")?;
    Ok(())
}

/// Runs `zirc ci connect`.
pub async fn connect(
    home: &Path,
    socket: &Path,
    args: ConnectArgs,
    tools: &mut dyn Tools,
    out: &mut (impl Write + Send),
) -> Result<()> {
    let root = root(tools, args.path)?;
    let remote = remote(tools, &root)?;
    if args.secret_only {
        let plan = format!(
            "This makes a new CI token and sets it as the secret {SECRET_NAME} on {}.",
            remote.slug
        );
        writeln!(out, "{plan}")?;
        if !confirm(args.yes, "Go ahead?", out)? {
            bail!("nothing changed");
        }
        return set_secret(tools, home, socket, &args.org, &root, &remote.slug, out).await;
    }
    let team = crate::ci::team(home, socket, &args.org).await?;
    let repository = remote
        .slug
        .rsplit('/')
        .next()
        .unwrap_or(&remote.slug)
        .to_owned();
    let step = zirc_proto::ci::actions_step(team.client.web(), &repository);
    let (changes, found) = changes(tools, &root, &remote.default, &step)?;
    if found == 0 {
        bail!(
            "no GitHub Actions workflows in .github/workflows on {}: for any other CI, \
             `zirc ci token` prints a token and the one command a build runs",
            remote.default
        );
    }
    if changes.is_empty() {
        writeln!(
            out,
            "Every job on {} tells Zirc already. If its runs aren't heard: {SECRET_ONLY_COMMAND}",
            remote.default
        )?;
        return Ok(());
    }
    for change in &changes {
        writeln!(
            out,
            "Found GitHub Actions: {} (jobs: {})",
            change.path,
            change.jobs.join(", ")
        )?;
    }
    writeln!(out, "This will:")?;
    writeln!(
        out,
        "  1. make {} a CI token and set it as the secret {SECRET_NAME} on {}, with gh",
        team.name, remote.slug
    )?;
    writeln!(
        out,
        "  2. add the step \"{STEP_NAME}\" to the end of each job, on a new branch {CONNECT_BRANCH}"
    )?;
    writeln!(
        out,
        "  3. push it and open a pull request, then wait for its run"
    )?;
    if !confirm(args.yes, "Go ahead?", out)? {
        bail!("nothing changed");
    }
    // Following before anything runs, so no run is missed.
    let mut watching = ipc::watch(socket, None).await?;
    set_secret(tools, home, socket, &args.org, &root, &remote.slug, out).await?;
    writeln!(
        out,
        "✓ Added the step \"{STEP_NAME}\" to the end of each job"
    )?;
    let pull = push(tools, &root, &remote, &changes)?;
    writeln!(
        out,
        "✓ Pushed {CONNECT_BRANCH} and opened a pull request: {pull}"
    )?;
    writeln!(out, "Waiting for its run…")?;
    let jobs: usize = changes.iter().map(|change| change.jobs.len()).sum();
    let Some(said) = hear(&mut watching, CONNECT_BRANCH, |said| said.len() >= jobs).await? else {
        bail!(
            "no run heard in {} minutes. If its workflows don't run on pull requests, merge \
             {pull}: the next run is heard. If a run ended unheard, {SECRET_ONLY_COMMAND} sets \
             the secret again",
            HEARD_WITHIN.as_secs() / 60
        );
    };
    let build = said
        .first()
        .map(|said| said.build.clone())
        .unwrap_or_default();
    writeln!(out, "✓ Heard run {build}: {}", outcome(&said))?;
    writeln!(out)?;
    writeln!(out, "{}", bold("CI is connected."))?;
    Ok(())
}

/// Commits `changes` on [`CONNECT_BRANCH`], from the default branch, in a worktree of its
/// own so the person's checkout is left as it is, pushes it and opens a pull request: its
/// address.
fn push(tools: &mut dyn Tools, root: &Path, remote: &Remote, changes: &[Change]) -> Result<String> {
    // A directory of its own, which git makes and `worktree remove` takes away.
    let tree = std::env::temp_dir().join(format!("{CONNECT_BRANCH}-{}", std::process::id()));
    let at = tree.to_string_lossy().into_owned();
    let from = format!("origin/{}", remote.default);
    tools.run(
        "git",
        &["worktree", "add", "--quiet", "--detach", &at, &from],
        root,
        None,
    )?;
    let done = (|| {
        let mut add = vec!["add", "--"];
        for change in changes {
            std::fs::write(tree.join(&change.path), &change.text)
                .with_context(|| format!("couldn't write {}", change.path))?;
            add.push(&change.path);
        }
        tools.run("git", &add, &tree, None)?;
        let message = format!("Tell Zirc how each job went ({CONNECT_COMMAND})");
        tools.run("git", &["commit", "--quiet", "-m", &message], &tree, None)?;
        let to = format!("HEAD:refs/heads/{CONNECT_BRANCH}");
        tools.run(
            "git",
            &["push", "--quiet", "--force", "origin", &to],
            &tree,
            None,
        )?;
        let body = format!(
            "Each job ends with a step that tells Zirc how it went, so agents wake when a build \
             of their branch fails. Made by `{CONNECT_COMMAND}`; the token is the repository's \
             secret {SECRET_NAME}."
        );
        tools.run(
            "gh",
            &[
                "pr",
                "create",
                "--repo",
                &remote.slug,
                "--head",
                CONNECT_BRANCH,
                "--base",
                &remote.default,
                "--title",
                "Tell Zirc how CI went",
                "--body",
                &body,
            ],
            &tree,
            None,
        )
    })();
    let _ = tools.run("git", &["worktree", "remove", "--force", &at], root, None);
    done
}

/// The branch a test failure is for, `asked` or the one checked out, and its last run's id.
fn last_run(
    tools: &mut dyn Tools,
    root: &Path,
    remote: &Remote,
    asked: Option<String>,
) -> Result<(String, String)> {
    let branch = match asked {
        Some(branch) => branch,
        None => tools.run("git", &["rev-parse", "--abbrev-ref", "HEAD"], root, None)?,
    };
    if branch == remote.default || branch == "HEAD" {
        bail!("a test failure is for a branch of your own, not {branch}: --branch <branch>");
    }
    let run = tools.run(
        "gh",
        &[
            "run",
            "list",
            "--repo",
            &remote.slug,
            "--branch",
            &branch,
            "--limit",
            "1",
            "--json",
            "databaseId",
            "-q",
            ".[0].databaseId",
        ],
        root,
        None,
    )?;
    if run.is_empty() {
        bail!(
            "{branch} has no run on {} yet: push it, let CI run once, then send the test",
            remote.slug
        );
    }
    Ok((branch, run))
}

/// Runs `zirc ci test-failure`.
pub async fn test_failure(
    socket: &Path,
    args: TestFailureArgs,
    tools: &mut dyn Tools,
    out: &mut (impl Write + Send),
) -> Result<()> {
    let root = root(tools, args.path)?;
    let remote = remote(tools, &root)?;
    let (branch, run) = last_run(tools, &root, &remote, args.branch)?;
    // From now on the session wakes on what CI says of the branch, this failure first.
    if let Some(session) = &args.session {
        let wait = Request::WaitFor {
            session: session.clone(),
            on: vec![branch.clone()],
            states: Vec::new(),
            reactions: Vec::new(),
            drop: false,
        };
        crate::client::ask(socket, wait).await?;
    }
    let mut watching = ipc::watch(socket, None).await?;
    tools
        .run(
            "gh",
            &[
                "variable",
                "set",
                TEST_VARIABLE,
                "--repo",
                &remote.slug,
                "--body",
                &branch,
            ],
            &root,
            None,
        )
        .with_context(|| {
            format!(
                "couldn't set {TEST_VARIABLE}: it takes admin access to {}",
                remote.slug
            )
        })?;
    let heard = async {
        tools.run(
            "gh",
            &["run", "rerun", &run, "--repo", &remote.slug],
            &root,
            None,
        )?;
        writeln!(
            out,
            "✓ Sent: CI is running {branch}'s run {run} again, to fail as a test"
        )?;
        hear(&mut watching, &branch, |said| {
            said.iter().any(|said| said.test)
        })
        .await
    }
    .await;
    // Whatever happened, the next real run isn't a test.
    let cleared = tools.run(
        "gh",
        &["variable", "delete", TEST_VARIABLE, "--repo", &remote.slug],
        &root,
        None,
    );
    let Some(said) = heard? else {
        bail!(
            "no test failure heard in {} minutes. {branch}'s workflows must have the step \
             \"{STEP_NAME}\" ({CONNECT_COMMAND}'s pull request, merged into it); if they do and \
             its runs aren't heard, {SECRET_ONLY_COMMAND}",
            HEARD_WITHIN.as_secs() / 60
        );
    };
    cleared.with_context(|| {
        format!("couldn't clear {TEST_VARIABLE}: gh variable delete {TEST_VARIABLE}")
    })?;
    let build = said
        .iter()
        .find(|said| said.test)
        .map(|said| said.build.clone())
        .unwrap_or_default();
    writeln!(
        out,
        "✓ Heard run {build} fail, as the test it is: nothing to fix"
    )?;
    match &args.session {
        Some(session) => writeln!(out, "{session}'s watch woke on it.")?,
        None => writeln!(
            out,
            "To wake an agent with it, name its session: {TEST_FAILURE_COMMAND} --session <session>"
        )?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: &str =
        "      - name: Tell Zirc\n        if: always()\n        run: |\n          echo hi\n";

    #[test]
    fn adds_the_step_to_the_end_of_each_job_with_steps() {
        let workflow = "\
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: make test

  lint:
    runs-on: ubuntu-latest
    steps:
      - run: make lint
    timeout-minutes: 5
  deploy:
    uses: ./.github/workflows/deploy.yml
";
        let (text, jobs) = with_step(workflow, STEP);
        assert_eq!(jobs, ["test", "lint"]);
        assert_eq!(
            text,
            "\
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: make test
      - name: Tell Zirc
        if: always()
        run: |
          echo hi

  lint:
    runs-on: ubuntu-latest
    steps:
      - run: make lint
      - name: Tell Zirc
        if: always()
        run: |
          echo hi
    timeout-minutes: 5
  deploy:
    uses: ./.github/workflows/deploy.yml
"
        );
        // Said again, it changes nothing.
        assert_eq!(with_step(&text, STEP), (text, Vec::new()));
    }

    #[test]
    fn keeps_a_workflows_own_indentation() {
        // Four spaces a level, items at their key's depth, and a multi-line last step.
        let workflow = "\
jobs:
    build:
        steps:
        - name: Build
          run: |
              make

              make check
";
        let (text, jobs) = with_step(workflow, STEP);
        assert_eq!(jobs, ["build"]);
        assert!(
            text.ends_with("              make check\n        - name: Tell Zirc\n          if: always()\n          run: |\n            echo hi\n"),
            "{text}"
        );
        assert_eq!(with_step("name: x\n", STEP).1, Vec::<String>::new());
        assert_eq!(
            with_step("jobs:\n  a:\n    steps: []\n", STEP).1,
            Vec::<String>::new()
        );
    }

    #[test]
    fn names_a_token_for_the_repository() {
        assert_eq!(token_name("zirc/server"), "github-server");
        assert_eq!(token_name("Acme/My.Repo_2"), "github-my-repo-2");
        assert!(token_name("a/a-very-long-repository-name-that-goes-on").len() <= 32);
    }

    /// `git` and `gh` answering from a script, and what they were asked.
    struct Fake {
        asked: Vec<String>,
        inputs: Vec<String>,
    }

    impl Tools for Fake {
        fn run(
            &mut self,
            program: &str,
            args: &[&str],
            _: &Path,
            input: Option<&str>,
        ) -> Result<String> {
            self.asked.push(format!("{program} {}", args.join(" ")));
            self.inputs.extend(input.map(str::to_owned));
            Ok(match (program, args[0], args.get(1).copied()) {
                ("git", "rev-parse", _) => "/src/server",
                ("gh", "repo", _) => "zirc/server main",
                ("git", "ls-tree", _) => ".github/workflows/ci.yml\n.github/workflows/notes.md",
                ("git", "show", _) => "jobs:\n  test:\n    steps:\n      - run: make test",
                _ => "",
            }
            .to_owned())
        }
    }

    #[test]
    fn finds_the_default_branchs_workflows_and_what_would_change() {
        let mut fake = Fake {
            asked: Vec::new(),
            inputs: Vec::new(),
        };
        let remote = remote(&mut fake, Path::new("/src/server")).unwrap();
        assert_eq!(
            (remote.slug.as_str(), remote.default.as_str()),
            ("zirc/server", "main")
        );
        let (changes, found) = changes(&mut fake, Path::new("/src/server"), "main", STEP).unwrap();
        assert_eq!(found, 1, "only the workflows");
        assert_eq!(changes.len(), 1);
        assert_eq!(
            (changes[0].path.as_str(), changes[0].jobs.as_slice()),
            (".github/workflows/ci.yml", &["test".to_owned()][..])
        );
        assert!(
            changes[0].text.ends_with("          echo hi\n"),
            "{}",
            changes[0].text
        );
        assert!(
            fake.asked
                .contains(&"git show origin/main:.github/workflows/ci.yml".to_owned()),
            "{:?}",
            fake.asked
        );
    }

    #[test]
    fn hears_only_cis_word_on_the_branchs_ended_runs() {
        let line = |account: &str, data: serde_json::Value| {
            let message = zirc_proto::Message::new(
                zirc_proto::NO_SESSION,
                Event::Custom {
                    kind: LANDING_KIND.into(),
                    data,
                    text: "x".into(),
                },
            );
            let mut delivery = ipc::Delivery::new("#agents", "zirc-ci", message);
            delivery.account = Some(account.to_owned());
            Heard::Message(delivery)
        };
        let data = |state: &str, branch: &str| serde_json::json!({"state": state, "branch": branch, "build": "9915", "job": "test", "test": true});
        let heard = said(&line("zirc-ci", data("failed", "tls")), "tls").unwrap();
        assert_eq!(
            (heard.build.as_str(), heard.job.as_str(), heard.test),
            ("9915", "test", true)
        );
        assert_eq!(outcome(&[heard]), "test failed");
        assert_eq!(said(&line("zirc-ci", data("running", "tls")), "tls"), None);
        assert_eq!(said(&line("zirc-ci", data("failed", "other")), "tls"), None);
        // A machine saying so isn't CI.
        assert_eq!(said(&line("ada-mbp", data("failed", "tls")), "tls"), None);
    }
}
