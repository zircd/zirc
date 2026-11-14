//! `zirc state`: how something a team shares stands now, such as its default branch broken
//! and who's fixing it ([`zirc_proto::state`]). Anyone sets one, a team's CI scripts
//! included; everyone reads it before relying on it.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use zirc_core::ipc::{Request, Response};
use zirc_core::states::{self, LOOK_BACK, Said};
use zirc_proto::state::Level;

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
        /// Who's saying it. Defaults to $ZIRC_SESSION, then $USER.
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
            session,
            org,
        }) => {
            let saying = Saying {
                name,
                level,
                why,
                owner,
                link,
                session,
                org,
            };
            set(socket, saying, out).await
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
                session,
                org,
            };
            set(socket, saying, out).await
        }
        None => show(socket, args, out).await,
    }
}

/// How something stands, as `zirc state set` and `zirc state clear` say it.
struct Saying {
    name: String,
    level: Level,
    why: Option<String>,
    owner: Option<String>,
    link: Option<String>,
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
    };
    let Response::Channel { channel, .. } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    writeln!(out, "{name} is {}, said in {channel}", level.word())?;
    Ok(())
}

/// The states in #ops's newest `limit` messages, the newest of each name.
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
    Ok(states::current(&messages))
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
        .map(|repo| repo.channel)
}

/// A state in a line: `2026-11-05T12:00:00Z #monorepo master is broken: …, said by ci`.
pub(super) fn line(said: &Said) -> String {
    format!(
        "{} {}, said by {}",
        said.time.as_deref().unwrap_or("?"),
        one_line(&said.state.text()),
        said.by
    )
}

async fn show(socket: &Path, args: StateArgs, out: &mut impl Write) -> Result<()> {
    let org = org_here(socket, args.org).await?;
    let all = read(socket, org.as_deref(), args.limit).await?;
    // In a watched repository, its states alone: another's master isn't this one's.
    let repo = repo_here(socket).await;
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
            writeln!(out, "{}", line(said))?;
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
    states::troubled(&all)
        .into_iter()
        .map(|said| format!("state: {}", line(said)))
        .collect()
}
