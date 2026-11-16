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
}

/// `zirc hints`: the effective hints, or with `check`, what's wrong with a hints file.
pub(crate) fn hints_command(
    home: &Path,
    check: Option<HintsCommand>,
    out: &mut impl Write,
) -> Result<()> {
    if let Some(HintsCommand::Check { file }) = check {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
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
    // The person's, the repository's here, then zirc's own: the first of a kind and
    // conditions is the one that counts, unless a lower one is required.
    let personal = hints::personal_in(home);
    let repo = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            let root = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            hints::of_repo(Path::new(&root)).unwrap_or_else(|problem| {
                eprintln!("{PROGRAM}: {problem}");
                Vec::new()
            })
        })
        .unwrap_or_default();
    let layers = [
        (hints::Layer::Personal, personal),
        (hints::Layer::Repository, repo),
        (hints::Layer::Default, hints::defaults()),
    ];
    for (layer, hint) in layers
        .iter()
        .flat_map(|(layer, hints)| hints.iter().map(move |hint| (*layer, hint)))
    {
        writeln!(out, "{}", hints::line(layer, hint))?;
    }
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
