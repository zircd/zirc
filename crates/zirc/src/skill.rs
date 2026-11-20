//! The zirc skill for Claude Code, and the guides `zirc agent-guide` and `zirc reference` print.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use clap::Subcommand;
use zirc_proto::PROGRAM;

use crate::client::shown;

/// What coding agents should know about zirc, for their CLAUDE.md or AGENTS.md.
pub(crate) const AGENT_GUIDE: &str = include_str!("agent-guide.md");
pub(crate) const REFERENCE: &str = include_str!("reference.md");

#[derive(Debug, Subcommand)]
pub(crate) enum SkillCommand {
    /// Write the zirc skill to ~/.claude/skills/zirc, replacing an older one.
    Install {
        /// Claude Code's skills directory. ~/.claude/skills unless set.
        #[arg(long, hide = true)]
        skills: Option<PathBuf>,
    },
    /// Remove the zirc skill.
    Uninstall {
        /// Claude Code's skills directory. ~/.claude/skills unless set.
        #[arg(long, hide = true)]
        skills: Option<PathBuf>,
    },
}

/// The agent guide as a Claude Code skill: the same text, under a header that says when to
/// use it and which zirc it describes.
pub(crate) fn skill() -> String {
    format!(
        "---\n\
         name: zirc\n\
         version: {version}\n\
         description: \"Coordinate with the people and other agent sessions working on the same \
         repository, through zirc. Use at the start of every session in a Git repository, to \
         start `zirc watch`, which says whether this machine's zirc watches it, or as its \
         CLAUDE.md or AGENTS.md says the team uses zirc; when `zirc watch` prints a line (an \
         overlap, an intent, a push or conflict to update for, or a message or post that \
         concerns the session); before editing any file for a task, and again whenever the task \
         reaches a file its intent doesn't name (`zirc intend`); and when you learn \
         something the hard way about some paths that the next session changing them should \
         know (`zirc gotcha`).\"\n\
         ---\n\n{AGENT_GUIDE}",
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// The zirc that an installed skill's `text` says it describes: its front matter's `version:`.
pub(crate) fn skill_version(text: &str) -> Option<&str> {
    let front = text.strip_prefix("---\n")?.split("\n---").next()?;
    front
        .lines()
        .find_map(|line| line.strip_prefix("version:"))
        .map(str::trim)
}

/// `version`'s numbers, to compare: `0.1.27` as `[0, 1, 27]`; `None` for anything else.
pub(crate) fn version_numbers(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|part| part.parse().ok()).collect()
}

/// Rewrites the zirc skill in `skills` when one is installed and describes an older zirc
/// than this one, or says no version, as the first skills didn't: whether it did. Never
/// installs one that isn't there, and never writes over a newer or unreadable version.
pub(crate) fn refresh_skill(skills: &Path) -> bool {
    let path = skills.join(PROGRAM).join("SKILL.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let older = match skill_version(&text) {
        None => true,
        Some(installed) => version_numbers(installed)
            .zip(version_numbers(env!("CARGO_PKG_VERSION")))
            .is_some_and(|(installed, mine)| installed < mine),
    };
    older && std::fs::write(&path, skill()).is_ok()
}

/// Claude Code's skills directory: the one asked for, or `skills` in its config directory,
/// `$CLAUDE_CONFIG_DIR` or `~/.claude`.
pub(crate) fn skills_dir(asked: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = asked {
        return Ok(dir);
    }
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(
        || -> Result<PathBuf> {
            let home = std::env::var_os("HOME").context("HOME isn't set")?;
            Ok(PathBuf::from(home).join(".claude"))
        },
        |dir| Ok(PathBuf::from(dir)),
    )?;
    Ok(config.join("skills"))
}

/// Writes the zirc skill to Claude Code's skills directory, replacing an older one, and
/// answers where.
pub(crate) fn install_skill(skills: Option<PathBuf>) -> Result<PathBuf> {
    let dir = skills_dir(skills)?.join("zirc");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("SKILL.md");
    std::fs::write(&path, skill())?;
    Ok(path)
}

/// `zirc skill`.
pub(crate) fn install_or_remove(command: SkillCommand, out: &mut impl Write) -> Result<()> {
    match command {
        SkillCommand::Install { skills } => {
            let path = install_skill(skills)?;
            writeln!(
                out,
                "installed the {PROGRAM} skill for Claude Code: {}",
                shown(&path)
            )?;
            Ok(())
        }
        SkillCommand::Uninstall { skills } => {
            let dir = skills_dir(skills)?.join("zirc");
            if dir.join("SKILL.md").exists() {
                std::fs::remove_dir_all(&dir)?;
                writeln!(out, "removed {}", shown(&dir))?;
            } else {
                writeln!(out, "the {PROGRAM} skill isn't installed")?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use zirc_core::ipc::{Direct, Overlap};

    use crate::lines::{direct_line, overlap_line};
    use crate::{Cli, run};

    #[test]
    fn the_agent_guide_matches_what_the_cli_prints() {
        // The guide quotes the CLI's commands and output; keep them in step.
        for quoted in [
            "zirc status",
            "zirc daemon start",
            "zirc repo add",
            "zirc watch",
            "zirc who",
            "zirc post --session",
            "zirc login",
            "zirc join --session <your session> <topic>",
            "--topic <topic>",
            "zirc repo add --session",
            "zirc watch --once --session <your session>",
            "zirc thinking --session <your session>",
            "zirc request --session <your session>",
            "zirc gotcha --session <your session>",
            "zirc reference",
            "dm from <nick>",
            "zirc ask --session <your session> --to",
            "zirc reply --session <your session> <id>",
            "zirc agents queries",
        ] {
            // As read, with the lines' breaks and indents as spaces.
            let guide = AGENT_GUIDE.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(guide.contains(quoted), "the guide doesn't mention {quoted}");
        }
        let query = direct_line(&Direct {
            from: "mattias-mini".into(),
            from_session: Some("cc-2".into()),
            session: None,
            text: "which branch?".into(),
            query: Some("q1".into()),
            answer: None,
            note: false,
            colleague: false,
            why: None,
            id: None,
            org: None,
        });
        assert_eq!(query, "query from mattias-mini/cc-2 [q1]: which branch?");
        assert!(AGENT_GUIDE.contains("query from <nick>/<session> [<id>]: <text>"));
        let answer = direct_line(&Direct {
            query: None,
            answer: Some("q1".into()),
            note: false,
            colleague: false,
            text: "main".into(),
            from: "mattias-mbp".into(),
            from_session: Some("cc-1".into()),
            session: Some("cc-2".into()),
            why: None,
            id: None,
            org: None,
        });
        assert_eq!(answer, "answer from mattias-mbp/cc-1 [q1]: main");
        assert!(AGENT_GUIDE.contains("answer from <nick>/<session> [<id>]: <text>"));
        let note = direct_line(&Direct {
            query: None,
            answer: None,
            note: true,
            colleague: false,
            text: "landed the login page".into(),
            from: "mattias-mbp".into(),
            from_session: Some("cc-1".into()),
            session: Some("cc-2".into()),
            why: None,
            id: None,
            org: None,
        });
        assert_eq!(note, "note from mattias-mbp/cc-1: landed the login page");
        assert!(AGENT_GUIDE.contains("note from <nick>/<session>: <text>"));
        let overlap = overlap_line(&Overlap {
            channel: "#c".into(),
            nick: "n".into(),
            paths: vec!["p".into()],
            why: None,
        });
        assert_eq!(overlap, "#c overlap: n is also changing p");
        assert!(AGENT_GUIDE.contains("overlap: <nick> is also changing <paths>"));
        assert!(AGENT_GUIDE.contains(
            "overlap: worktrees <path> [<branch>] and <path> [<branch>] are both changing"
        ));
        assert!(
            AGENT_GUIDE.starts_with("## Zirc"),
            "it's appended under a heading of its own"
        );
    }

    #[tokio::test]
    async fn installs_and_removes_the_skill() {
        let skills = tempfile::tempdir().unwrap();
        let run = |action: &'static str| {
            let dir = skills.path().to_str().unwrap().to_owned();
            async move {
                let cli = Cli::try_parse_from(["zirc", "skill", action, "--skills", &dir]).unwrap();
                let mut out = Vec::new();
                run(cli, &mut out).await.unwrap();
                String::from_utf8(out).unwrap()
            }
        };
        assert!(run("install").await.starts_with("installed "));
        let installed = std::fs::read_to_string(skills.path().join("zirc/SKILL.md")).unwrap();
        // Claude Code reads the name and description from the header; the body is the guide.
        assert!(installed.starts_with("---\nname: zirc\nversion: "));
        assert!(installed.contains("\ndescription: \"Coordinate with"));
        assert!(installed.ends_with(AGENT_GUIDE));

        assert!(run("uninstall").await.starts_with("removed "));
        assert!(!skills.path().join("zirc").exists());
        assert_eq!(run("uninstall").await, "the zirc skill isn't installed\n");
    }

    #[test]
    fn a_newer_zirc_brings_an_installed_skill_up_to_it_and_installs_none() {
        let skills = tempfile::tempdir().unwrap();
        let path = skills.path().join("zirc/SKILL.md");
        // None installed stays none.
        assert!(!refresh_skill(skills.path()));
        assert!(!path.exists());

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let installed = |version: &str| {
            std::fs::write(
                &path,
                format!("---\nname: zirc\nversion: {version}\n---\n\nrun `zirc read`\n"),
            )
            .unwrap();
        };
        // An older one is rewritten with this zirc's guide.
        installed("0.0.1");
        assert!(refresh_skill(skills.path()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), skill());
        assert_eq!(skill_version(&skill()), Some(env!("CARGO_PKG_VERSION")));
        // The same, a newer one, and one whose version can't be read are left as they are.
        for version in [env!("CARGO_PKG_VERSION"), "999.0.0", "dev"] {
            installed(version);
            assert!(!refresh_skill(skills.path()), "{version}");
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("zirc read")
            );
        }
        // One from before skills said their version is older than any.
        std::fs::write(&path, "---\nname: zirc\n---\n\nold\n").unwrap();
        assert!(refresh_skill(skills.path()));
        assert_eq!(version_numbers("0.1.27"), Some(vec![0, 1, 27]));
        assert_eq!(version_numbers("0.1.27-dev"), None);
    }
}
