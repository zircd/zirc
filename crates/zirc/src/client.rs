//! Asking zircd over its socket, and saying why it didn't answer.

use std::path::{Path, PathBuf};
use std::time::Duration;
use zirc_proto::PROGRAM;

use anyhow::{Result, bail};
use zirc_core::ipc::{self, Repo, Request, Response};
use zirc_core::paths;

use crate::args::OrgArg;
use crate::daemon::not_set_up;
use crate::repo::repo_here;

/// How long a command waits for zircd's answer: longer than zircd waits on the server for
/// any (as `zirc search` does), short enough that a stuck zircd doesn't hang a script.
pub(crate) const ANSWER_WAIT: Duration = Duration::from_secs(60);

/// A request that waits on the server, which a zircd from before it can't read: said so.
pub(crate) async fn waits(socket: &Path, org: Option<&str>, request: Request) -> Result<Response> {
    match answer_within(socket, org, &request, ANSWER_WAIT).await? {
        Err(ipc::Error::Daemon(message)) if message.contains("unknown variant") => {
            bail!("{}", ipc::older_zircd())
        }
        answer => answer.map_err(|err| not_running(socket, err)),
    }
}

/// What `zirc who` and `zirc repo list` say before any `zirc repo add`.
pub(crate) const NOT_WATCHING: &str =
    "not watching any repositories: run `zirc repo add` inside one to watch it";

/// `path` for people: under the home directory, as `~/…`.
pub(crate) fn shown(path: &Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| Some(Path::new("~").join(path.strip_prefix(home).ok()?)))
        .unwrap_or_else(|| path.to_owned())
        .display()
        .to_string()
}

/// What a command says when zircd's answer isn't the kind it asked for, which only a zircd
/// of another version gives: so, and what to do about it.
pub(crate) fn unexpected() -> anyhow::Error {
    anyhow::anyhow!(
        "zircd's answer isn't one this {PROGRAM} knows: {}",
        ipc::older_zircd()
    )
}

/// `path` made absolute, or the current directory: zircd runs elsewhere.
pub(crate) fn here(path: Option<PathBuf>) -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(path.map_or_else(|| cwd.clone(), |path| cwd.join(path)))
}

pub(crate) async fn ask(socket: &Path, request: Request) -> Result<Response> {
    ask_in(socket, None, request).await
}

/// zircd's answer to `request` for the organisation `org`: see [`ipc::request_in`].
pub(crate) async fn ask_in(socket: &Path, org: Option<&str>, request: Request) -> Result<Response> {
    answer_within(socket, org, &request, ANSWER_WAIT)
        .await?
        .map_err(|err| not_running(socket, err))
}

/// The organisation a command is for: as `--org` says, or else the one the watched
/// repository it runs in belongs to. `None` leaves it to zircd, which takes the one the
/// request's repository belongs to, or the machine's only one.
pub(crate) async fn org_here(socket: &Path, org: OrgArg) -> Result<Option<String>> {
    if org.name.is_some() {
        return Ok(org.name);
    }
    let Response::Repos { repos } = ask(socket, Request::RepoList).await? else {
        return Err(crate::client::unexpected());
    };
    let cwd = std::env::current_dir()?;
    Ok(repo_here(&repos, &cwd).and_then(|repo| repo.org.clone()))
}

/// Whether the repositories are in more than one organisation, so they're named.
pub(crate) fn several(repos: &[Repo]) -> bool {
    repos
        .iter()
        .filter_map(|repo| repo.org.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        > 1
}

/// `#api`, or `acme #api` when it's one organisation's of several: or any line said in one.
pub(crate) fn channel_in(channel: &str, org: Option<&str>) -> String {
    match org {
        Some(org) => format!("{org} {channel}"),
        None => channel.to_owned(),
    }
}

/// zircd's answer to `request`, or an error once it's taken longer than `wait`, as a zircd
/// busy with something else can: then the request doesn't hang.
pub(crate) async fn answer_within(
    socket: &Path,
    org: Option<&str>,
    request: &Request,
    wait: Duration,
) -> Result<Result<Response, ipc::Error>> {
    tokio::time::timeout(wait, ipc::request_in(socket, org, request))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "zircd didn't answer within {wait:?}: see {}",
                shown(&socket.with_file_name("zircd.log"))
            )
        })
}

/// The usual reason the socket can't be reached, said so: zircd isn't running, or on a
/// machine that isn't set up, there's no zircd to run yet.
pub(crate) fn not_running(socket: &Path, err: ipc::Error) -> anyhow::Error {
    match &err {
        ipc::Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            let set_up = socket
                .parent()
                .is_some_and(|home| paths::config(home).exists());
            if set_up {
                anyhow::anyhow!("zircd isn't running: start it with `zirc daemon start`")
            } else {
                anyhow::anyhow!(not_set_up())
            }
        }
        _ => err.into(),
    }
}

/// The last lines of a log, for an error message.
pub(crate) fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(10)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gives_up_on_a_zircd_that_doesnt_answer() {
        let home = tempfile::Builder::new()
            .prefix("zirc")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = home.path().join("zircd.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        // Takes the connection and never answers.
        let _held = tokio::spawn(async move {
            let held = listener.accept().await;
            std::future::pending::<()>().await;
            drop(held);
        });
        let err = answer_within(&socket, None, &Request::Status, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("zircd didn't answer within 200ms"),
            "{err}"
        );
    }
}
