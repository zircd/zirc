//! zircd's command line: little to set, since `zirc daemon` runs it, but `--help` says what
//! it is and how it's run.

use std::path::PathBuf;

use clap::{ArgAction, Parser};

/// The end of `zircd --help`: what zircd does, and how it's usually run.
const RUNNING: &str = "\
zircd watches the repositories that zirc repo add names, tells their channels about
edits, commits and pushes, and serves the zirc commands over a socket in zirc's directory.

It's usually run for you:
  zirc daemon install   Run zircd at login, and keep it running
  zirc daemon start     Run zircd until it's stopped
  zirc daemon stop      Stop it; so do SIGINT and SIGTERM";

/// The zirc daemon: this machine's one connection to the team's IRC server.
// The doc comments here are --help text, where backticks would show.
#[derive(Debug, Parser)]
#[command(name = "zircd", version, after_help = RUNNING)]
pub struct Cli {
    /// zirc's directory, with config.toml, the socket and the log. ~/.zirc unless set.
    #[arg(long, env = "ZIRC_HOME", value_name = "DIR")]
    pub home: Option<PathBuf>,
    /// What to log, as a tracing filter such as zircd=debug.
    #[arg(long, env = "ZIRC_LOG", value_name = "FILTER", default_value = "info")]
    pub log: String,
    /// `-?` asks for help too, as `-h` does.
    #[arg(short = '?', action = ArgAction::Help, hide = true)]
    question_mark: Option<bool>,
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use clap::error::ErrorKind;

    use super::Cli;

    #[test]
    fn says_what_it_is_instead_of_starting() {
        for flag in ["--help", "-h", "-?"] {
            let err = Cli::try_parse_from(["zircd", flag]).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::DisplayHelp, "{flag}");
            assert!(err.to_string().contains("zirc daemon install"), "{flag}");
        }
        for flag in ["--version", "-V"] {
            let err = Cli::try_parse_from(["zircd", flag]).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::DisplayVersion, "{flag}");
        }
        let err = Cli::try_parse_from(["zircd", "start"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn runs_with_no_arguments() {
        let cli = Cli::try_parse_from(["zircd", "--home", "/tmp/zirc"]).unwrap();
        assert_eq!(cli.home, Some("/tmp/zirc".into()));
        Cli::try_parse_from(["zircd"]).unwrap();
    }
}
