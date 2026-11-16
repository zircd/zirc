//! `zirc hints`, and the hints `zirc watch` shows under what wakes a session.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use zirc_core::hints;
use zirc_core::ipc;
use zirc_proto::PROGRAM;

use crate::lines::one_line;

/// The hints for `session`'s watch, as `zirc_core::hints::Watch` reads them, saying on
/// stderr for each repository whose hints file doesn't read that its hints are left out.
pub(crate) fn watch_hints(home: &Path, session: Option<&str>, repos: &[ipc::Repo]) -> hints::Watch {
    let (watch, problems) = hints::Watch::load(home, session, repos);
    for problem in problems {
        eprintln!("{PROGRAM}: {problem}");
    }
    watch
}

/// What `zirc hints` does besides listing them.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum HintsCommand {
    /// Say what's wrong with a hints file, such as .zirc/hints.toml, or that it's fine.
    Check {
        /// The file.
        file: PathBuf,
    },
    /// Add a hint for this repository, or with --org the organisation.
    ///
    /// Add a hint for this repository, or with --org the whole organisation: the server keeps
    /// it, and it holds at once on every machine.
    Add(AddArgs),
    /// Remove a hint the server keeps, by the id `zirc hints` shows.
    ///
    /// Remove a hint the server keeps, by the id `zirc hints` shows ([hint 0123abcd]).
    Remove {
        /// The hint's id, or enough of its start to be the only one.
        id: String,
        /// The agent session asking. Defaults to $ZIRC_SESSION.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Move this repository's .zirc/hints.toml to the server.
    ///
    /// Move a repository's .zirc/hints.toml, as its default branch has it, to the server, where
    /// its hints then hold in its place.
    Import {
        /// Another file of hints to import, in place of the repository's.
        file: Option<PathBuf>,
        /// The agent session asking. Defaults to $ZIRC_SESSION.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
}

// `zirc hints add`'s arguments: their doc comments are its --help.
#[derive(Debug, clap::Args)]
pub(crate) struct AddArgs {
    /// The kind of event it's for, such as overlap or landing.yours.failed: `zirc hints`
    /// lists zirc's own, with theirs.
    #[arg(long)]
    pub(crate) kind: String,
    /// What it asks: act, tell or quiet.
    #[arg(long, default_value = "tell")]
    pub(crate) level: String,
    /// A condition, as `lockfile=true`, `names_me=true`, `branch=api-*`, `channel=#api/*` or
    /// `emoji=👀`; several must all hold.
    #[arg(long = "when", value_name = "CONDITION")]
    pub(crate) when: Vec<String>,
    /// Make it required, so no person's own hint overrides it: a person's alone, from a
    /// terminal of their own.
    #[arg(long)]
    pub(crate) required: bool,
    /// For the whole organisation, rather than this repository.
    #[arg(long)]
    pub(crate) org: bool,
    /// What it says, in a line, with {who}, {branch} and the like filled in.
    #[arg(short, long, num_args = 1.., required = true)]
    pub(crate) message: Vec<String>,
    /// The agent session asking. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
}

/// `zirc hints`: the hints in effect here, or what `command` says to do with them.
pub(crate) async fn hints_command(
    home: &Path,
    socket: &Path,
    command: Option<HintsCommand>,
    out: &mut impl Write,
) -> Result<()> {
    match command {
        None => list(home, socket, out).await,
        Some(HintsCommand::Check { file }) => check_file(&file, out),
        Some(HintsCommand::Add(args)) => add(home, socket, args, out).await,
        Some(HintsCommand::Remove { id, session }) => {
            remove(home, socket, &id, session.as_deref(), out).await
        }
        Some(HintsCommand::Import { file, session }) => {
            import(home, socket, file.as_deref(), session.as_deref(), out).await
        }
    }
}

/// What's wrong with the hints file `file`, or that it's fine.
fn check_file(file: &Path, out: &mut impl Write) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let problems = hints::check(&text, hints::Layer::Repository);
    if problems.is_empty() {
        writeln!(out, "{}: fine", file.display())?;
        return Ok(());
    }
    for problem in &problems {
        writeln!(out, "{}: {problem}", file.display())?;
    }
    bail!("{} problems in {}", problems.len(), file.display());
}

/// The watched repository here, if this runs in one.
async fn repo_here(socket: &Path) -> Option<ipc::Repo> {
    crate::gotchas::here(socket, &[])
        .await
        .ok()
        .map(|(repo, _)| repo)
}

/// The hints in effect here, highest layer first, each in a line; those the server keeps with
/// the id `zirc hints remove` takes. In a layer, the most specific that fits is the one that
/// counts; a higher layer's overrides a lower one's, unless the lower one's is required.
async fn list(home: &Path, socket: &Path, out: &mut impl Write) -> Result<()> {
    let repo = repo_here(socket).await;
    let (team, problem) = match &repo {
        Some(repo) => hints::team(home, repo),
        None => (
            hints::Team {
                organisation: zirc_core::team_hints::for_repo(home, None, "").1,
                ..hints::Team::default()
            },
            None,
        ),
    };
    if let Some(problem) = problem {
        eprintln!("{PROGRAM}: {problem}");
    }
    let tag = |hint: &hints::Hint| zirc_proto::hints::tag(&hint.id());
    for hint in hints::personal_in(home) {
        writeln!(out, "{}", hints::line(hints::Layer::Personal, &hint))?;
    }
    for hint in &team.repository {
        let source = if team.from_file {
            " (from .zirc/hints.toml: zirc hints import moves it to the server)".to_owned()
        } else {
            tag(hint)
        };
        writeln!(
            out,
            "{}{source}",
            hints::line(hints::Layer::Repository, hint)
        )?;
    }
    for hint in &team.organisation {
        writeln!(
            out,
            "{}{}",
            hints::line(hints::Layer::Organisation, hint),
            tag(hint)
        )?;
    }
    for hint in hints::defaults() {
        writeln!(out, "{}", hints::line(hints::Layer::Default, &hint))?;
    }
    Ok(())
}

/// The organisation of `repo`, or this machine's first, with its server, asked as this
/// machine.
fn server_for(
    home: &Path,
    repo: Option<&ipc::Repo>,
) -> Result<(String, zirc_core::team_hints::Server)> {
    let text = std::fs::read_to_string(zirc_core::paths::config(home))
        .context("this machine isn't signed in: zirc login")?;
    let settings = zirc_core::settings::Settings::from_toml(&text)
        .map_err(|err| anyhow::anyhow!("config.toml doesn't read: {err}"))?;
    let orgs = settings.organisations();
    let org = repo
        .and_then(|repo| repo.org.as_deref())
        .and_then(|name| orgs.iter().find(|org| org.name() == name))
        .or_else(|| orgs.first())
        .context("this machine isn't signed in: zirc login")?;
    let server = zirc_core::team_hints::Server::of(home, org).context(
        "this machine has no key of its own on the organisation's server: zirc login again",
    )?;
    Ok((org.name(), server))
}

/// Reads the team's hints again and keeps them, so a change holds here at once: the other
/// machines read them when they're told.
async fn read_again(home: &Path, org: &str, server: &zirc_core::team_hints::Server) {
    if let Ok(listing) = server.list().await {
        let _ = zirc_core::team_hints::keep(home, org, &listing);
    }
}

/// The scope a change is for: the organisation, or the watched repository here.
async fn scope_here(socket: &Path, org: bool) -> Result<(Option<ipc::Repo>, String)> {
    let repo = repo_here(socket).await;
    if org {
        return Ok((repo, zirc_proto::hints::ORG.to_owned()));
    }
    let Some(repo) = repo else {
        bail!(
            "not in a repository zirc watches: run it in one, or say --org for the organisation's"
        );
    };
    let scope = repo.channel.to_lowercase();
    Ok((Some(repo), scope))
}

/// The hint `args` says.
fn hint_of(args: &AddArgs) -> Result<hints::Hint> {
    let level = serde_json::from_value(serde_json::json!(args.level))
        .map_err(|_| anyhow::anyhow!("--level is act, tell or quiet"))?;
    let when = hints::When::parse(&args.when).map_err(|why| anyhow::anyhow!("--when: {why}"))?;
    Ok(hints::Hint {
        kind: args.kind.clone(),
        when,
        level,
        text: crate::args::words(&args.message)?,
        required: args.required,
    })
}

/// `zirc hints add`.
async fn add(home: &Path, socket: &Path, args: AddArgs, out: &mut impl Write) -> Result<()> {
    let hint = hint_of(&args)?;
    let (repo, scope) = scope_here(socket, args.org).await?;
    let (org, server) = server_for(home, repo.as_ref())?;
    let change = server
        .add(&scope, &hint, args.session.as_deref())
        .await
        .map_err(anyhow::Error::msg)?;
    read_again(home, &org, &server).await;
    let stored = &change.hint;
    writeln!(
        out,
        "added for {}: {}{}",
        stored.whose(),
        hints::line(stored.layer(), &stored.hint),
        zirc_proto::hints::tag(&stored.id)
    )?;
    Ok(())
}

/// `zirc hints remove`.
async fn remove(
    home: &Path,
    socket: &Path,
    id: &str,
    session: Option<&str>,
    out: &mut impl Write,
) -> Result<()> {
    let repo = repo_here(socket).await;
    let (org, server) = server_for(home, repo.as_ref())?;
    let listing = server.list().await.map_err(anyhow::Error::msg)?;
    let stored = listing.by_id(id).map_err(anyhow::Error::msg)?;
    let change = server
        .remove(&stored.id, session)
        .await
        .map_err(anyhow::Error::msg)?;
    read_again(home, &org, &server).await;
    writeln!(
        out,
        "removed for {}: {}",
        change.hint.whose(),
        hints::line(change.hint.layer(), &change.hint.hint)
    )?;
    Ok(())
}

/// `zirc hints import`: each of the file's hints added for the repository here, saying how
/// each went; one there already is fine.
async fn import(
    home: &Path,
    socket: &Path,
    file: Option<&Path>,
    session: Option<&str>,
    out: &mut impl Write,
) -> Result<()> {
    let (repo, scope) = scope_here(socket, false).await?;
    let Some(repo) = repo else {
        bail!("not in a repository zirc watches: run it in one");
    };
    let given = match file {
        Some(file) => hints::parse(
            &std::fs::read_to_string(file)
                .with_context(|| format!("reading {}", file.display()))?,
        ),
        None => hints::of_repo(&repo.path),
    }
    .map_err(anyhow::Error::msg)?;
    if given.is_empty() {
        writeln!(
            out,
            "no hints to import: the default branch has no .zirc/hints.toml"
        )?;
        return Ok(());
    }
    let (org, server) = server_for(home, Some(&repo))?;
    let mut failed = 0;
    for hint in &given {
        let line = hints::line(hints::Layer::Repository, hint);
        match server.add(&scope, hint, session).await {
            Ok(_) => writeln!(out, "imported: {line}")?,
            Err(why) if why.contains("there already") => writeln!(out, "there already: {line}")?,
            Err(why) => {
                failed += 1;
                writeln!(out, "not imported ({why}): {line}")?;
            }
        }
    }
    read_again(home, &org, &server).await;
    if failed > 0 {
        bail!("{failed} of {} hints weren't imported", given.len());
    }
    writeln!(
        out,
        "{scope}'s hints are now the server's: remove .zirc/hints.toml, which is no longer read"
    )?;
    Ok(())
}

/// A hint as its line under the event: marked, so nothing anyone sends can pass for one.
pub(crate) fn hint_line(shown: &hints::Shown) -> String {
    format!(
        "  \u{21b3} hint ({}): {}",
        shown.layer.name(),
        one_line(&shown.text)
    )
}
