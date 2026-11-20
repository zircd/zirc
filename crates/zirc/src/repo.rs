//! `zirc repo`: the repositories zircd watches, and which one a path is in.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Subcommand;
use zirc_core::ipc::{Repo, Request, Response};
use zirc_proto::PROGRAM;

use crate::client::{NOT_WATCHING, ask_in, here, several, shown, watched_in};
use crate::status::shared_warnings;

#[derive(Debug, Subcommand)]
pub(crate) enum RepoCommand {
    /// Start watching the repository at PATH, or the current directory.
    Add {
        /// Anywhere in the repository.
        path: Option<PathBuf>,
        /// Who works here, such as an agent session's id: its commits and pushes are
        /// announced as theirs. Defaults to `$ZIRC_SESSION`.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
        /// The organisation it belongs to, on a machine signed in to several: its work is
        /// told to that organisation alone. Defaults to the one another clone of it belongs
        /// to.
        #[arg(long, value_name = "NAME", hide = true)]
        org: Option<String>,
    },
    /// Stop watching the repository at PATH, or the current directory.
    Remove {
        /// Anywhere in the repository.
        path: Option<PathBuf>,
        /// Only take this session out of it, leaving it watched for the rest.
        #[arg(long)]
        session: Option<String>,
    },
    /// Say whether this machine watches the repository here, or at PATH.
    ///
    /// And in which channel: an agent's first check, which fails when it isn't. With
    /// `--session`, a watched one is also added for that session, as `zirc repo add
    /// --session` does: one command to start a session with.
    Here {
        /// Anywhere in the repository, or in a worktree of it.
        path: Option<PathBuf>,
        /// Who works here, such as an agent session's id: when it's watched, its commits
        /// and pushes here are announced as theirs from now on.
        #[arg(long)]
        session: Option<String>,
    },
    /// List the repositories being watched and their channels.
    List {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// `zirc repo add`, `remove` and `list`.
pub(crate) async fn repo(socket: &Path, command: RepoCommand, out: &mut impl Write) -> Result<()> {
    let (request, json, org) = match command {
        RepoCommand::Add { path, session, org } => (
            Request::RepoAdd {
                path: here(path)?,
                session,
            },
            false,
            org,
        ),
        RepoCommand::Remove { path, session } => (
            Request::RepoRemove {
                path: here(path)?,
                session,
            },
            false,
            None,
        ),
        RepoCommand::List { json } => (Request::RepoList, json, None),
        RepoCommand::Here { path, session } => {
            let path = here(path)?;
            let org = here_watched(socket, &path, out).await?;
            let Some(session) = session else {
                return Ok(());
            };
            (
                Request::RepoAdd {
                    path,
                    session: Some(session),
                },
                false,
                org,
            )
        }
    };
    let adding = match &request {
        Request::RepoAdd { session, .. } => session.clone(),
        _ => None,
    };
    let added = match &request {
        // As zircd names it, symlinks resolved, such as macOS's /tmp in /private/tmp.
        Request::RepoAdd { path, .. } => Some(path.canonicalize().unwrap_or_else(|_| path.clone())),
        _ => None,
    };
    let Response::Repos { repos } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    // The repository the path is in: the deepest watched one that contains it.
    if let Some(repo) = added.and_then(|path| {
        repos
            .iter()
            .filter(|repo| path.starts_with(&repo.path))
            .max_by_key(|repo| repo.path.components().count())
    }) {
        writeln!(
            out,
            "watching {} as {}: see who's working on what there with `zirc who`, or \
             follow it with `zirc watch`",
            shown(&repo.path),
            watched_in(
                repo,
                several(&repos).then_some(repo.org.as_deref()).flatten()
            )
        )?;
        for line in shared_warnings(repo, adding.as_deref()) {
            writeln!(out, "{line}")?;
        }
        // A person adding it, not an agent: where its agents will talk, and, until the
        // repository's instructions say so, how to have Claude Code use zirc at all.
        if adding.is_none() {
            for line in for_agents(&repo.path) {
                writeln!(out, "{line}")?;
            }
            writeln!(out)?;
            writeln!(out, "{TRY}")?;
        }
    } else if json {
        writeln!(out, "{}", serde_json::to_string(&repos)?)?;
    } else if repos.is_empty() {
        writeln!(out, "{NOT_WATCHING}")?;
    } else {
        let several = several(&repos);
        for repo in &repos {
            let org = several.then_some(repo.org.as_deref()).flatten();
            write!(out, "{} {}", watched_in(repo, org), shown(&repo.path))?;
            // Each session working there.
            let sessions: Vec<&str> = repo.sessions.iter().map(|it| it.session.as_str()).collect();
            if sessions.is_empty() {
                writeln!(out)?;
            } else {
                writeln!(out, ": {}", sessions.join("; "))?;
            }
        }
    }
    Ok(())
}

/// What `zirc repo add` ends on for a person: the first thing worth seeing, in a minute.
pub(crate) const TRY: &str = "Try it: start two Claude Code sessions here and ask both to change the \
     same file. The second hears about the first before it starts.";

/// What `zirc repo add` tells a person about the repository at `root`: where its agents
/// talk, that Claude Code's sessions there use zirc by themselves, through the zirc skill,
/// and, while its `AGENTS.md` doesn't mention zirc, how other agents can.
pub(crate) fn for_agents(root: &Path) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Agents working here talk in {}, with every repository's: Zirc's apps join it for \
             you.",
            zirc_proto::AGENTS_CHANNEL
        ),
        format!(
            "Claude Code sessions here use {PROGRAM} by themselves, through the {PROGRAM} skill."
        ),
    ];
    let told = std::fs::read_to_string(root.join("AGENTS.md"))
        .is_ok_and(|text| text.to_lowercase().contains(PROGRAM));
    if !told {
        lines.push(format!("Other agents: {PROGRAM} agent-guide >> AGENTS.md"));
    }
    lines
}

/// `zirc repo here`: the watched repository `path` is in, directly or as a worktree of
/// one, and its channel; an error when there's none, so an agent's first check can tell.
/// What it answers is its organisation, on a machine signed in to several.
async fn here_watched(socket: &Path, path: &Path, out: &mut impl Write) -> Result<Option<String>> {
    let Response::Repos { repos } = ask_in(socket, None, Request::RepoList).await? else {
        return Err(crate::client::unexpected());
    };
    let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let main = main_checkout(&path);
    let found = repo_here(&repos, &path).or_else(|| repo_here(&repos, main.as_deref()?));
    let Some(repo) = found else {
        bail!(
            "{} isn't in a repository this machine watches: a person starts watching one \
             with `{PROGRAM} repo add`",
            shown(&path)
        );
    };
    let org = several(&repos).then_some(repo.org.as_deref()).flatten();
    writeln!(
        out,
        "{} is watched as {}",
        shown(&repo.path),
        watched_in(repo, org)
    )?;
    Ok(org.map(str::to_owned))
}

/// The main checkout of the Git working tree at `path`, which a linked worktree shares
/// its repository with: `None` outside Git.
pub(crate) fn main_checkout(path: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    let common = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    (output.status.success() && common.ends_with(".git"))
        .then(|| common.parent().map(Path::to_owned))
        .flatten()
}

/// `path` relative to the repository at `root`, `/`-separated, with `.` and `..` taken as
/// written: `None` outside it. The root itself is `.`.
pub(crate) fn in_repo(root: &Path, path: &Path) -> Option<String> {
    use std::path::Component;
    let mut plain = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                plain.pop();
            }
            Component::CurDir => {}
            part => plain.push(part),
        }
    }
    let inside = plain.strip_prefix(root).ok()?;
    let parts: Vec<String> = inside
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

/// The watched repository `cwd` is in: the innermost, where one's inside another.
pub(crate) fn repo_here<'a>(repos: &'a [Repo], cwd: &Path) -> Option<&'a Repo> {
    repos
        .iter()
        .filter(|repo| cwd.starts_with(&repo.path))
        .max_by_key(|repo| repo.path.as_os_str().len())
}

/// What `zirc watch --session` says when no repository was added with that session: what
/// the checkout at `cwd` is watched for instead, if it is, and the fix.
pub(crate) fn unregistered(session: &str, repos: &[Repo], cwd: &Path) -> String {
    let watched = repo_here(repos, cwd).map_or_else(String::new, |repo| {
        let sessions: Vec<&str> = repo.sessions.iter().map(|it| it.session.as_str()).collect();
        match sessions.as_slice() {
            [] => format!("{} is watched, but for no session; ", shown(&repo.path)),
            sessions => format!(
                "{} is watched for {}, not {session}; ",
                shown(&repo.path),
                sessions.join(", ")
            ),
        }
    });
    format!(
        "{PROGRAM}: no repository is watched for {session}, so this hears only what names it; \
         {watched}run `zirc repo add --session {session}` where it works"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_core::ipc;

    #[test]
    fn finds_the_innermost_watched_repository() {
        let repo = |path: &str, scope: &str| Repo {
            path: path.into(),
            scope: scope.into(),
            sessions: Vec::new(),
            org: None,
            shared: Vec::new(),
        };
        let repos = [repo("/src/api", "#api"), repo("/src/api/vendor/ui", "#ui")];
        let here = |cwd: &str| repo_here(&repos, Path::new(cwd)).map(|repo| &*repo.scope);
        assert_eq!(here("/src/api/crates"), Some("#api"));
        assert_eq!(here("/src/api/vendor/ui/src"), Some("#ui"));
        assert_eq!(here("/src/apiary"), None);
        assert_eq!(here("/elsewhere"), None);
    }

    #[test]
    fn says_what_a_checkout_is_watched_for_when_its_not_the_session() {
        let repo = Repo {
            path: "/src/api".into(),
            scope: "#api".into(),
            sessions: vec![
                ipc::RepoSession {
                    session: "cc-1".into(),
                },
                ipc::RepoSession {
                    session: "cc-2".into(),
                },
            ],
            org: None,
            shared: Vec::new(),
        };
        assert_eq!(
            unregistered("cc-3", &[repo], Path::new("/src/api/crates")),
            "zirc: no repository is watched for cc-3, so this hears only what names it; \
             /src/api is watched for cc-1, cc-2, not cc-3; run `zirc repo add \
             --session cc-3` where it works"
        );
    }

    #[test]
    fn tells_a_person_where_agents_talk_and_that_claude_code_needs_nothing() {
        let root = tempfile::tempdir().unwrap();
        let lines = for_agents(root.path());
        assert_eq!(
            lines[0],
            "Agents working here talk in #agents, with every repository's: Zirc's apps join it \
             for you."
        );
        assert!(lines[1].starts_with("Claude Code sessions here use zirc by themselves"));
        assert_eq!(lines[2], "Other agents: zirc agent-guide >> AGENTS.md");

        // Once AGENTS.md mentions zirc, other agents need no telling.
        std::fs::write(root.path().join("AGENTS.md"), "Follow zirc's guide.\n").unwrap();
        assert_eq!(for_agents(root.path()).len(), 2);
    }

    #[test]
    fn a_linked_worktree_is_found_by_its_main_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().canonicalize().unwrap().join("main");
        let git = |args: &[&str]| {
            let done = std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(args)
                .output()
                .unwrap();
            assert!(done.status.success(), "{done:?}");
        };
        std::fs::create_dir(&main).unwrap();
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@e",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ]);
        let linked = main.with_file_name("linked");
        git(&["worktree", "add", "-q", linked.to_str().unwrap()]);
        assert_eq!(main_checkout(&linked).as_deref(), Some(main.as_path()));
        assert_eq!(
            main_checkout(&main.join(".")).as_deref(),
            Some(main.as_path())
        );
        assert_eq!(main_checkout(dir.path()), None);
    }

    #[test]
    fn a_path_in_a_repository_is_said_from_its_root() {
        let root = Path::new("/work/api");
        assert_eq!(
            in_repo(root, Path::new("/work/api/src/../crates/./x.rs")).as_deref(),
            Some("crates/x.rs")
        );
        assert_eq!(in_repo(root, Path::new("/work/api/")).as_deref(), Some("."));
        assert_eq!(in_repo(root, Path::new("/work/api/../web")), None);
    }
}
