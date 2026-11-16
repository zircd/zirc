//! `zirc playbook`: the hints and gotchas that apply to a path, in one place
//! ([`zirc_core::playbook`]).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Args;
use zirc_core::gotchas;
use zirc_core::hints::{self, Layer};
use zirc_core::playbook::Playbook;
use zirc_proto::PROGRAM;

use crate::gotchas::{here, read};

/// Tells zircd's recorder, best effort and quickly, that `session` was shown `record`: part
/// of the playbook in use ([`zirc_core::ipc::Request::Record`]). A zircd that predates it,
/// or one that's slow, costs nothing.
pub(crate) async fn used<R: zirc_proto::activity::Record>(
    socket: &Path,
    session: &str,
    record: &R,
) {
    let Ok(data) = serde_json::to_value(record) else {
        return;
    };
    let request = zirc_core::ipc::Request::Record {
        session: session.to_owned(),
        kind: R::KIND.to_owned(),
        data,
    };
    let asking = crate::client::ask(socket, request);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), asking).await;
}

// `zirc playbook`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct PlaybookArgs {
    /// The file or directory, relative to where this runs: the gotchas about it, what's in it
    /// or what holds it. The whole repository's without one.
    pub(crate) path: Option<PathBuf>,
    /// Print JSON: every hint with its layer, zirc's own included, and the gotchas, newest
    /// first.
    #[arg(long)]
    pub(crate) json: bool,
}

/// `zirc playbook`.
pub(crate) async fn playbook(
    home: &Path,
    socket: &Path,
    args: PlaybookArgs,
    out: &mut impl Write,
) -> Result<()> {
    let PlaybookArgs { path, json } = args;
    let (repo, paths) = here(socket, path.as_slice()).await?;
    let all = read(socket, &repo).await;
    let place = paths
        .first()
        .filter(|path| !path.is_empty())
        .cloned()
        .unwrap_or_else(|| "the repository".to_owned());
    let found = if paths.is_empty() {
        gotchas::under(&all, &[String::new()])
    } else {
        gotchas::under(&all, &paths)
    };
    let personal = hints::personal_in(home);
    let ours = hints::of_repo(&repo.path).unwrap_or_else(|problem| {
        eprintln!("{PROGRAM}: {problem}");
        Vec::new()
    });
    let defaults = hints::defaults();
    let layers = [
        (Layer::Personal, personal.as_slice()),
        (Layer::Repository, ours.as_slice()),
        (Layer::Default, defaults.as_slice()),
    ];
    let playbook = Playbook::new(&layers, found);
    if json {
        writeln!(out, "{}", serde_json::to_string(&playbook)?)?;
    } else {
        write!(out, "{}", playbook.text(&place))?;
    }
    Ok(())
}
