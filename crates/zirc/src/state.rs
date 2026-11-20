//! `zirc state`: how something a team shares stands now, such as its default branch broken
//! and who's fixing it ([`zirc_proto::state`]). Anyone sets one, a team's CI scripts
//! included; everyone reads it before relying on it.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use zirc_core::ipc::{Request, Response};
use zirc_core::states::{self, LOOK_BACK, Said};
use zirc_proto::state::{HOLD, Level};

use crate::args::{OrgArg, session_or_user};
use crate::client::{ask, ask_in, here, org_here, waits};
use crate::lines::one_line;

/// What `zirc state` takes.
// The doc comments on the arguments are --help text, where backticks would show.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(super) struct StateArgs {
    #[command(subcommand)]
    command: Option<StateCommand>,
    /// Only this one, such as master, shown however it stands. Without, every state that
    /// isn't ok.
    #[arg(value_name = "NAME")]
    state_name: Option<String>,
    /// Those that are ok too.
    #[arg(long)]
    all: bool,
    /// Fail when one shown is broken: a one-off check in a CI script. To wait for one to
    /// be fine, zirc wait state <NAME> ok in the background, never a loop on this.
    #[arg(long)]
    check: bool,
    /// How many of #ops's newest messages to look through.
    #[arg(short = 'n', long, default_value_t = LOOK_BACK)]
    limit: usize,
    /// Print JSON instead of text.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    org: OrgArg,
}

#[derive(Debug, Subcommand)]
enum StateCommand {
    /// Say how something stands, such as master broken and who's on it.
    ///
    /// Such as: zirc state set master broken --why "//web:e2e fails since build 1008"
    /// --owner cc-2b76ff. It goes to #ops, as decisions do, for the repository this is run in, and wakes the
    /// owner's watch alone. The newest said of a name is how it stands.
    Set {
        /// What it's the state of, such as master or staging.
        #[arg(value_name = "NAME")]
        name: String,
        /// ok, warning or broken.
        #[arg(value_name = "LEVEL", value_parser = level)]
        level: Level,
        /// Why, in a few words, such as which check fails since when.
        #[arg(long)]
        why: Option<String>,
        /// Who's on it: a session or a person, whose watch it wakes.
        #[arg(long)]
        owner: Option<String>,
        /// Where to read more, such as a failed build's log.
        #[arg(long)]
        link: Option<String>,
        /// When it ends by itself, as how long from now (90m, 2h) or a time (2026-11-14T15:00:00Z):
        /// it reads as fine from then, and this machine says so. A hold on landing, landings
        /// broken, must say it.
        #[arg(long, value_name = "WHEN")]
        until: Option<String>,
        /// Who's saying it. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
        /// The organisation, on a machine signed in to several.
        #[arg(long = "org", value_name = "NAME", hide = true)]
        org: Option<String>,
    },
    /// Say you're on it: the state said again as it stands, with you as its owner.
    ///
    /// Such as: zirc state take master --session cc-2b76ff. Refused while an owner it names
    /// has a live watch, as two sessions fixing one break collide: ask them first.
    Take {
        /// What it's the state of, such as master.
        #[arg(value_name = "NAME")]
        name: String,
        /// Who's taking it. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
        /// The organisation, on a machine signed in to several.
        #[arg(long = "org", value_name = "NAME", hide = true)]
        org: Option<String>,
    },
    /// Say it's fine again: the same as zirc state set <NAME> ok.
    Clear {
        /// What it's the state of, such as master.
        #[arg(value_name = "NAME")]
        name: String,
        /// Why it's fine, such as the fix that landed.
        #[arg(long)]
        why: Option<String>,
        /// Who's saying it. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
        /// The organisation, on a machine signed in to several.
        #[arg(long = "org", value_name = "NAME", hide = true)]
        org: Option<String>,
    },
}

/// The level `word` names, for clap.
pub(crate) fn level(word: &str) -> Result<Level, String> {
    Level::of_word(word).ok_or_else(|| format!("{word}: say ok, warning or broken"))
}

/// `zirc state`.
pub(super) async fn state(socket: &Path, args: StateArgs, out: &mut impl Write) -> Result<()> {
    match args.command {
        Some(StateCommand::Set {
            name,
            level,
            why,
            owner,
            link,
            until,
            session,
            org,
        }) => {
            let until = until_of(&name, level, until.as_deref(), crate::catchup::now())?;
            let saying = Saying {
                name,
                level,
                why,
                owner,
                link,
                until,
                session,
                org,
            };
            set(socket, saying, out).await
        }
        Some(StateCommand::Take { name, session, org }) => {
            take(socket, name, session, org, out).await
        }
        Some(StateCommand::Clear {
            name,
            why,
            session,
            org,
        }) => {
            let saying = Saying {
                name,
                level: Level::Ok,
                why,
                owner: None,
                link: None,
                until: None,
                session,
                org,
            };
            set(socket, saying, out).await
        }
        None => show(socket, args, out).await,
    }
}

/// When a state said now, at `now`, ends by itself, as a time tag, from `--until`'s `when`:
/// how long from now, or a time. A hold on landing ([`HOLD`]) that isn't fine must say it.
fn until_of(name: &str, level: Level, when: Option<&str>, now: u64) -> Result<Option<String>> {
    let Some(when) = when else {
        if name == HOLD && level != Level::Ok {
            bail!(
                "{HOLD} {} is a hold on every landing: say when it ends, with --until 2h or --until 2026-11-14T15:00:00Z",
                level.word()
            );
        }
        return Ok(None);
    };
    let at = zirc_core::time::duration(when)
        .map(|seconds| now.saturating_add(seconds))
        .or_else(|| zirc_core::time::seconds_at(when))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--until {when}: say how long from now (90m, 2h) or a time (2026-11-14T15:00:00Z)"
            )
        })?;
    if at <= now {
        bail!("--until {when} has passed");
    }
    Ok(Some(zirc_core::time::tag(at)))
}

/// How something stands, as `zirc state set` and `zirc state clear` say it.
struct Saying {
    name: String,
    level: Level,
    why: Option<String>,
    owner: Option<String>,
    link: Option<String>,
    until: Option<String>,
    session: Option<String>,
    org: Option<String>,
}

async fn set(socket: &Path, saying: Saying, out: &mut impl Write) -> Result<()> {
    let Saying {
        name,
        level,
        why,
        owner,
        link,
        until,
        session,
        org,
    } = saying;
    let org = org_here(socket, OrgArg { name: org }).await?;
    let request = Request::SetState {
        repo: here(None)?,
        session: session_or_user(session),
        name: name.clone(),
        level,
        why,
        owner,
        link,
        until,
    };
    let Response::Channel { channel, .. } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    writeln!(out, "{name} is {}, said in {channel}", level.word())?;
    Ok(())
}

/// `zirc state take`: `name` said again as it stands, with `session` as its owner, unless
/// someone else on it is live.
async fn take(
    socket: &Path,
    name: String,
    session: Option<String>,
    org: Option<String>,
    out: &mut impl Write,
) -> Result<()> {
    let session = session_or_user(session);
    let org_name = org_here(socket, OrgArg { name: org.clone() }).await?;
    let all = read(socket, org_name.as_deref(), LOOK_BACK).await?;
    let repo = repo_here(socket).await;
    let Some(said) = all.into_iter().find(|said| {
        said.state.name == name && repo.as_ref().is_none_or(|repo| &said.state.repo == repo)
    }) else {
        bail!(
            "no state {name} said in {}: nothing to take",
            states::CHANNEL
        );
    };
    if said.state.level == Level::Ok {
        bail!("{name} is ok: nothing to take");
    }
    let (live, _) = live_sessions(socket, org_name.as_deref()).await;
    if let Some(owner) = said
        .state
        .owners()
        .into_iter()
        .find(|owner| *owner != session && live.iter().any(|l| l == owner))
    {
        bail!(
            "{owner} is on {name}, and its watch is live: ask it first (zirc ask --session {session} --to <its machine>/{owner} \"…\")"
        );
    }
    let state = said.state;
    let saying = Saying {
        name,
        level: state.level,
        why: state.why,
        owner: Some(session.clone()),
        link: state.link,
        until: state.until,
        session: Some(session),
        org,
    };
    set(socket, saying, out).await
}

/// The organisation's live sessions and those gone, as zircd knows them
/// ([`Request::LiveSessions`]): none of either from a zircd that predates it.
async fn live_sessions(socket: &Path, org: Option<&str>) -> (Vec<String>, Vec<String>) {
    match ask_in(socket, org, Request::LiveSessions).await {
        Ok(Response::LiveSessions { live, gone }) => (live, gone),
        _ => (Vec::new(), Vec::new()),
    }
}

/// The states in #ops's newest `limit` messages, the newest of each name, as they stand now:
/// one that has ended by itself is fine ([`states::in_force`]).
pub(super) async fn read(socket: &Path, org: Option<&str>, limit: usize) -> Result<Vec<Said>> {
    let request = Request::Read {
        target: states::CHANNEL.to_owned(),
        limit,
        since: None,
        until: None,
    };
    let Response::Messages { messages } = waits(socket, org, request).await? else {
        return Err(crate::client::unexpected());
    };
    let now = zirc_core::time::tag(crate::catchup::now());
    Ok(states::current(&messages)
        .into_iter()
        .map(|said| states::in_force(said, &now))
        .collect())
}

/// The channel of the watched repository this is run in, if it's in one.
pub(super) async fn repo_here(socket: &Path) -> Option<String> {
    let cwd = here(None).ok()?;
    let Ok(Response::Repos { repos }) = ask(socket, Request::RepoList).await else {
        return None;
    };
    repos
        .into_iter()
        .filter(|repo| cwd.starts_with(&repo.path))
        .max_by_key(|repo| repo.path.as_os_str().len())
        .map(|repo| repo.scope)
}

/// A state in a line: `2026-11-05T12:00:00Z #monorepo master is broken: …, said by ci`, and
/// for one that isn't fine, how long it has stood and how far its branch has moved since, as
/// this checkout knows it, and whether its owner has gone ([`states::standing`]):
/// `(broken 37h, master 14 commits on, owner gone)`.
pub(super) fn line(said: &Said, now: u64, gone: &[String]) -> String {
    let mut line = format!(
        "{} {}, said by {}",
        said.time.as_deref().unwrap_or("?"),
        one_line(&said.state.text()),
        said.by
    );
    let branch = states::branch_of(&said.state.name);
    let moved = said
        .time
        .as_deref()
        .and_then(|time| commits_since(branch, time));
    if let Some(standing) =
        states::standing(said, now, moved.map(|commits| (branch, commits)), gone)
    {
        line = format!("{line} ({standing})");
    }
    line
}

/// How many commits `origin/<branch>` took since `time`, as this checkout last fetched it:
/// nothing when it's no branch here, or git can't say.
fn commits_since(branch: &str, time: &str) -> Option<usize> {
    let at = zirc_core::time::seconds_at(time)?;
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("--since={at}")])
        .arg(format!("refs/remotes/origin/{branch}"))
        .arg("--")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(())?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

async fn show(socket: &Path, args: StateArgs, out: &mut impl Write) -> Result<()> {
    let org = org_here(socket, args.org).await?;
    let all = read(socket, org.as_deref(), args.limit).await?;
    // In a watched repository, its states alone: another's master isn't this one's.
    let repo = repo_here(socket).await;
    let now = crate::catchup::now();
    let (_, gone) = live_sessions(socket, org.as_deref()).await;
    let shown: Vec<&Said> = all
        .iter()
        .filter(|said| repo.as_ref().is_none_or(|repo| &said.state.repo == repo))
        .filter(|said| match &args.state_name {
            Some(name) => &said.state.name == name,
            None => args.all || said.state.level != Level::Ok,
        })
        .collect();
    for said in &shown {
        if args.json {
            writeln!(out, "{}", serde_json::to_string(&json(said))?)?;
        } else {
            writeln!(out, "{}", line(said, now, &gone))?;
            if said.state.level != Level::Ok {
                let wait = states::wait_hint(&said.state.name);
                writeln!(out, "  \u{21b3} {wait}")?;
            }
        }
    }
    if shown.is_empty() && !args.json {
        match &args.state_name {
            Some(name) => writeln!(out, "no state {name} said in {}", states::CHANNEL)?,
            None => writeln!(out, "nothing is broken or in warning")?,
        }
    }
    if args.check
        && let Some(broken) = shown.iter().find(|said| said.state.level == Level::Broken)
    {
        bail!("{}", one_line(&broken.state.text()));
    }
    Ok(())
}

/// A state as `--json` prints it: the state's fields, with who said it and when.
fn json(said: &Said) -> serde_json::Value {
    let mut value = serde_json::to_value(&said.state).unwrap_or_default();
    if let Some(map) = value.as_object_mut() {
        map.insert("by".into(), said.by.clone().into());
        if let Some(time) = &said.time {
            map.insert("time".into(), time.clone().into());
        }
    }
    value
}

/// The states that aren't ok, as `zirc status` says them: a line each. Nothing when #ops
/// can't be read, as when zircd isn't connected: status says that itself.
pub(super) async fn troubled_lines(socket: &Path) -> Vec<String> {
    let Ok(all) = read(socket, None, LOOK_BACK).await else {
        return Vec::new();
    };
    let now = crate::catchup::now();
    let (_, gone) = live_sessions(socket, None).await;
    states::troubled(&all)
        .into_iter()
        .map(|said| format!("state: {}", line(said, now, &gone)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_794_668_400;

    #[test]
    fn a_hold_says_when_it_ends_and_ends_ahead() {
        assert!(until_of(HOLD, Level::Broken, None, NOW).is_err());
        assert_eq!(until_of(HOLD, Level::Ok, None, NOW).unwrap(), None);
        assert_eq!(until_of("master", Level::Broken, None, NOW).unwrap(), None);
        assert_eq!(
            until_of(HOLD, Level::Broken, Some("2h"), NOW).unwrap(),
            Some(zirc_core::time::tag(NOW + 7200))
        );
        assert_eq!(
            until_of(HOLD, Level::Broken, Some("2026-11-14T15:00:00Z"), 0).unwrap(),
            Some("2026-11-14T15:00:00.000Z".into())
        );
        assert!(until_of(HOLD, Level::Broken, Some("2026-11-14T15:00:00Z"), NOW * 2).is_err());
        assert!(until_of(HOLD, Level::Broken, Some("later"), NOW).is_err());
    }
}
