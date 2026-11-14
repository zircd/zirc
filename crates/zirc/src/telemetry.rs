//! What the CLI tells analytics: each command's name, and whether an agent ran it. See
//! [`zirc_core::telemetry`].

use std::io::IsTerminal as _;

use clap::ArgMatches;
use zirc_core::telemetry::Telemetry;
use zirc_proto::telemetry::{Client, CommandRun, Event};

/// The command `matches` ran, by name only, such as `repo add`, and whether it ran with
/// `--session`, as an agent's commands do.
#[must_use]
pub fn command_run(matches: &ArgMatches) -> CommandRun {
    let mut names = Vec::new();
    let mut deepest = matches;
    while let Some((name, sub)) = deepest.subcommand() {
        names.push(name);
        deepest = sub;
    }
    CommandRun {
        command: names.join(" "),
        agent: matches!(deepest.try_get_raw("session"), Ok(Some(_))),
    }
}

/// Records the command `matches` ran, and, at a terminal, says once that analytics is on.
/// The TUI says so itself, where it can be seen.
pub fn command_started(matches: &ArgMatches) {
    let telemetry = Telemetry::load(&zirc_core::paths::home(), Client::Cli);
    let run = command_run(matches);
    if run.command != "tui" {
        let interactive = std::io::stdout().is_terminal() && std::io::stderr().is_terminal();
        telemetry.notice(interactive, &mut std::io::stderr());
    }
    telemetry.record(&Event::CommandRun(run));
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    fn run(args: &[&str]) -> CommandRun {
        let matches = crate::Cli::command()
            .try_get_matches_from(std::iter::once("zirc").chain(args.iter().copied()))
            .unwrap();
        command_run(&matches)
    }

    #[test]
    fn names_the_command_without_its_arguments() {
        let posted = run(&["post", "--session", "cc-1", "taking: the secret plan"]);
        assert_eq!(posted.command, "post");
        assert!(posted.agent);
        let added = run(&["repo", "add", "/Users/ada/secret-repo"]);
        assert_eq!(added.command, "repo add");
        assert!(!added.agent);
        assert_eq!(run(&["who"]).command, "who");
    }
}
