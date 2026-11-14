//! Arguments several commands take: an organisation, a moment, where a post goes, its words.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use clap::Args;
use zirc_core::ipc::Target;
use zirc_proto::PROGRAM;

use crate::catchup;

/// Which organisation a command is for, on a machine signed in to several. Hidden from
/// `--help`, as every `--org <name>` is: someone in one organisation sees no change.
#[derive(Debug, Default, Args)]
pub(crate) struct OrgArg {
    /// The organisation, on a machine signed in to several. Defaults to the one the watched
    /// repository this is run in belongs to.
    #[arg(long = "org", value_name = "NAME", hide = true)]
    pub(crate) name: Option<String>,
}

/// The moment `text`, given as `flag`, names ([`zirc_core::time::moment`]); a bare number is
/// minutes ago, as zirc has always taken it.
pub(crate) fn moment_arg(text: &str, flag: &str) -> Result<u64> {
    let minutes;
    let text = if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        minutes = format!("{text}m");
        &minutes
    } else {
        text
    };
    zirc_core::time::moment(text, catchup::now()).with_context(|| {
        format!(
            "{flag} {text:?} isn't a time such as 30m, 2h, 3d, 2w, 2026-11-05 or \
             2026-11-05T08:00:00Z"
        )
    })
}

/// How long `text` says, in seconds, for a flag such as `--for` ([`zirc_core::time::duration`]):
/// `90s`, `20m`, `2h`, `3d` or `2w`; a bare number is minutes, as zirc has always taken it.
pub(crate) fn duration_arg(text: &str) -> Result<u64, String> {
    let minutes;
    let said = if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        minutes = format!("{text}m");
        &minutes
    } else {
        text
    };
    zirc_core::time::duration(said)
        .ok_or_else(|| format!("{text}: say how long, such as 90s, 20m, 2h, 3d or 2w"))
}

/// Where a post goes, other than the repository's channel.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum Scope {
    /// The repository's channel, #<repo>, where every session and person working on it is.
    Repo,
    /// Your branch's channel, #<repo>/<branch>, for a pushed branch others work on too.
    Branch,
    /// The organisation's channel, #general.
    Org,
}

/// Which channel a post or event goes to: as the team's configuration says, unless one of
/// these does.
#[derive(Debug, Args)]
pub(crate) struct Destination {
    /// The repository whose channels to post to. Defaults to the current directory.
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Post to the repository's channel, your pushed branch's, or the organisation's
    /// (#general), rather than where posts go by default.
    #[arg(long, value_enum, conflicts_with = "topic")]
    to: Option<Scope>,
    /// Post to a topic you've joined: the repository's, or with --org, the organisation's.
    #[arg(long)]
    topic: Option<String>,
    /// With --topic, the organisation's topic rather than the repository's.
    #[arg(long, requires = "topic")]
    org: bool,
}

impl Destination {
    /// The repository, and which of its channels.
    pub(crate) fn target(self) -> (Option<PathBuf>, Target) {
        let target = match (self.to, self.topic) {
            (_, Some(name)) => Target::Topic {
                name,
                org: self.org,
            },
            (Some(Scope::Repo), None) => Target::Repo,
            (None, None) => Target::Default,
            (Some(Scope::Branch), None) => Target::Branch,
            (Some(Scope::Org), None) => Target::Org,
        };
        (self.repo, target)
    }
}

/// What `text`, a command's words, says: them, a space apart, or read from stdin for `-`,
/// line breaks and all.
pub(crate) fn words(text: &[String]) -> Result<String> {
    if let [dash] = text
        && dash == "-"
    {
        let mut read = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut read)
            .context("couldn't read what to say from stdin")?;
        let read = read.trim_end();
        if read.trim().is_empty() {
            bail!("nothing to say on stdin");
        }
        return Ok(read.to_owned());
    }
    Ok(text.join(" "))
}

/// `--app`'s name as apps show it, or why it can't be one.
pub(crate) fn app_named(app: Option<String>) -> Result<Option<String>> {
    app.map(|name| {
        zirc_proto::app_label(&name).with_context(|| {
            format!("--app {name}: say it with letters, digits or -, such as ci or deploy")
        })
    })
    .transpose()
}

/// The session whose watch `command` wakes, as said. With none it's an error that says how
/// to say one: such a command once went by the user's name, registered what no watch would
/// ever hear, and said it had worked.
pub(crate) fn session_watched(session: Option<String>, command: &str) -> Result<String> {
    session
        .filter(|session| !session.is_empty())
        .with_context(|| {
            format!(
                "{PROGRAM} {command} wakes a session's watch, so it needs to know whose: add \
             --session <id>, the one your `{PROGRAM} watch --session` runs as, or set \
             ZIRC_SESSION"
            )
        })
}

/// `session`, or else the user's name, or `zirc` when the machine says neither.
pub(crate) fn session_or_user(session: Option<String>) -> String {
    session
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "zirc".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_moments_and_bare_minutes() {
        // The moment is counted back from a clock read after this one, so it's at most
        // that long ago, and less by any second that ticks between the two.
        let now = crate::catchup::now();
        let ago = |text: &str| now - super::moment_arg(text, "--since").unwrap();
        assert!((1795..=1800).contains(&ago("30m")));
        assert!(
            (1795..=1800).contains(&ago("30")),
            "a bare number is minutes"
        );
        assert!((7195..=7200).contains(&ago("2h")));
        assert_eq!(
            super::moment_arg("2026-11-05T08:00:00Z", "--until").unwrap(),
            zirc_core::time::seconds_at("2026-11-05T08:00:00Z").unwrap()
        );
        assert!(super::moment_arg("soon", "--since").is_err());
        assert!((1_814_400..1_814_405).contains(&ago("3w")));
        assert!(super::moment_arg("3y", "--since").is_err());
        // How long, for --for: the same units, and a bare number is minutes.
        for (said, seconds) in [("90s", 90), ("20m", 1200), ("15", 900), ("2h", 7200)] {
            assert_eq!(duration_arg(said), Ok(seconds), "{said}");
        }
        assert_eq!(duration_arg("3d"), Ok(259_200));
        assert_eq!(duration_arg("2w"), Ok(1_209_600));
        let err = duration_arg("soon").unwrap_err();
        assert!(err.contains("such as 90s, 20m, 2h, 3d or 2w"), "{err}");
    }

    #[test]
    fn an_integrations_name_is_its_short_label() {
        assert_eq!(app_named(Some("CI".into())).unwrap().as_deref(), Some("ci"));
        assert_eq!(app_named(None).unwrap(), None);
        let err = app_named(Some("!!".into())).unwrap_err().to_string();
        assert!(err.contains("such as ci or deploy"), "{err}");
        assert_eq!(words(&["a".into(), "b".into()]).unwrap(), "a b");
    }

    #[test]
    fn a_command_that_wakes_a_watch_needs_a_session_and_says_how_to_give_one() {
        assert_eq!(
            session_watched(Some("cc-1".into()), "stack").unwrap(),
            "cc-1"
        );
        for none in [None, Some(String::new())] {
            let err = session_watched(none, "wait-for").unwrap_err().to_string();
            assert!(
                err.contains("zirc wait-for wakes a session's watch"),
                "{err}"
            );
            assert!(err.contains("--session <id>"), "{err}");
        }
    }
}
