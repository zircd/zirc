//! `zirc gotcha`: one-line lessons tied to paths, kept in the repository's gotchas room
//! rather than in the repository ([`zirc_core::gotchas`]). With `-m` it keeps one; without,
//! it lists them, as `zirc gotchas` did and still does; with --retire, it retires one.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Args;
use zirc_core::gotchas::{self, Data, Gotcha};
use zirc_core::ipc::{Repo, Request, Response, Target};

use crate::args::{session_or_user, words};
use crate::client::{ask, waits};
use crate::post::print_channel;
use crate::repo::{in_repo, repo_here};

// `zirc gotcha`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct GotchaArgs {
    /// The files or directories it's about, relative to where this runs: the narrowest
    /// that hold what it's about. Without -m, the path whose lessons to list: all of the
    /// repository's without one.
    pub(crate) paths: Vec<PathBuf>,
    /// The lesson to keep, in a line: what goes wrong, and what to do instead. Without it,
    /// the lessons kept are listed, newest first.
    #[arg(short, long, num_args = 1..)]
    pub(crate) message: Vec<String>,
    /// Listing, print JSON: an array, newest first.
    #[arg(long, conflicts_with = "message")]
    pub(crate) json: bool,
    /// Retire the lesson with this id, as a listing shows it ([id 01jbx7ab]), when it no
    /// longer holds: nobody is told it again. Keeping it again later brings it back.
    #[arg(long, value_name = "ID", conflicts_with_all = ["message", "json", "paths"])]
    pub(crate) retire: Option<String>,
    /// Who learnt it: an agent session's id. Defaults to $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
}

// `zirc gotchas`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct GotchasArgs {
    /// Only those about this path, relative to where this runs: about it, what's in it, or
    /// what holds it. All of them without one.
    pub(crate) path: Option<PathBuf>,
    /// Print JSON: an array on one line, newest first.
    #[arg(long)]
    pub(crate) json: bool,
}

/// The watched repository here, its root, and `paths` relative to that root.
pub(crate) async fn here(socket: &Path, paths: &[PathBuf]) -> Result<(Repo, Vec<String>)> {
    let Response::Repos { repos } = ask(socket, Request::RepoList).await? else {
        return Err(crate::client::unexpected());
    };
    let cwd = std::env::current_dir()?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let Some(repo) = repo_here(&repos, &cwd).cloned() else {
        bail!("not in a repository zirc watches: run it in one (`zirc repo here` says)");
    };
    let root = repo
        .path
        .canonicalize()
        .unwrap_or_else(|_| repo.path.clone());
    let mut relative = Vec::new();
    for path in paths {
        let Some(path) = in_repo(&root, &cwd.join(path)) else {
            bail!("{} isn't in this repository", path.display());
        };
        relative.push(path);
    }
    Ok((repo, relative))
}

/// The gotchas room, [`gotchas::CHANNEL`], as a post's target: the organisation's topic of
/// its name.
fn room() -> Target {
    Target::Topic {
        name: gotchas::CHANNEL.trim_start_matches('#').to_owned(),
    }
}

/// The gotchas about `repo` kept in the gotchas room and not retired, oldest first; none when
/// its history can't be read.
pub(crate) async fn read(socket: &Path, repo: &Repo) -> Vec<Gotcha> {
    let read = Request::Read {
        target: gotchas::CHANNEL.to_owned(),
        limit: gotchas::READ,
        since: None,
        until: None,
    };
    match waits(socket, repo.org.as_deref(), read).await {
        Ok(Response::Messages { mut messages }) => {
            messages.retain(|delivery| {
                delivery.message.repo.is_some()
                    && delivery.scope().eq_ignore_ascii_case(&repo.scope)
            });
            gotchas::kept(&messages)
        }
        _ => Vec::new(),
    }
}

/// `zirc gotcha`.
pub(crate) async fn gotcha(socket: &Path, args: GotchaArgs, out: &mut impl Write) -> Result<()> {
    let GotchaArgs {
        paths,
        message,
        json,
        retire,
        session,
    } = args;
    if let Some(id) = retire {
        return retire_one(socket, &id, session, out).await;
    }
    // Nothing to keep: the lessons kept, about the path if one's given.
    if message.is_empty() {
        anyhow::ensure!(
            paths.len() <= 1,
            "say one path to list the lessons about, or -m with the lesson to keep about these"
        );
        let path = paths.into_iter().next();
        return list(socket, GotchasArgs { path, json }, out).await;
    }
    anyhow::ensure!(!paths.is_empty(), "say which paths the lesson is about");
    let text = words(&message)?;
    if text.lines().count() > 1 {
        bail!("a gotcha is one line: say what goes wrong and what to do instead");
    }
    let (repo, paths) = here(socket, &paths).await?;
    let request = Request::Emit {
        repo: repo.path.clone(),
        session: session_or_user(session),
        kind: gotchas::KIND.to_owned(),
        data: serde_json::to_value(Data { paths })?,
        text,
        to: room(),
        app: None,
    };
    print_channel(ask(socket, request).await?, "kept in", out)
}

/// `zirc gotcha --retire <id>`: the event retiring the gotcha kept here with that id.
async fn retire_one(
    socket: &Path,
    id: &str,
    session: Option<String>,
    out: &mut impl Write,
) -> Result<()> {
    let (repo, _) = here(socket, &[]).await?;
    let all = read(socket, &repo).await;
    let gotcha = gotchas::by_id(&all, id).map_err(anyhow::Error::msg)?;
    let request = Request::Emit {
        repo: repo.path.clone(),
        session: session_or_user(session),
        kind: gotchas::RETIRED.to_owned(),
        data: serde_json::to_value(gotchas::Retired::of(gotcha))?,
        text: gotchas::retired_text(gotcha),
        to: room(),
        app: None,
    };
    print_channel(ask(socket, request).await?, "retired in", out)
}

/// `zirc gotchas`.
pub(crate) async fn list(socket: &Path, args: GotchasArgs, out: &mut impl Write) -> Result<()> {
    let GotchasArgs { path, json } = args;
    let (repo, paths) = here(socket, path.as_slice()).await?;
    let all = read(socket, &repo).await;
    let found: Vec<&Gotcha> = gotchas::about(&all, &paths);
    if json {
        writeln!(out, "{}", serde_json::to_string(&found)?)?;
        return Ok(());
    }
    if found.is_empty() {
        writeln!(out, "no gotchas there yet")?;
    }
    for gotcha in found {
        writeln!(out, "{}", gotchas::line(gotcha))?;
    }
    Ok(())
}

/// The gotchas under `paths`, relative to where this runs, as `zirc intend` says them to
/// `session`, telling the recorder of each it says; empty outside a watched repository or
/// when there are none.
pub(crate) async fn under_here(socket: &Path, session: &str, paths: &[PathBuf]) -> String {
    let Ok((repo, paths)) = here(socket, paths).await else {
        return String::new();
    };
    let all = read(socket, &repo).await;
    let found = gotchas::under(&all, &paths);
    for gotcha in found.iter().take(gotchas::SHOWN) {
        if let Some(id) = &gotcha.id {
            let told = zirc_proto::activity::GotchaTold {
                gotcha: id.clone(),
                how: zirc_proto::activity::Told::Intent,
            };
            crate::playbook::used(socket, session, &told).await;
        }
    }
    gotchas::text(&found)
}
