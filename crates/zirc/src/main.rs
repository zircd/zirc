//! The zirc CLI's entry point. See [`zirc::run`].

use clap::{CommandFactory as _, FromArgMatches as _};
use zirc_proto::PROGRAM;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let matches = zirc::Cli::command().get_matches();
    let cli = zirc::Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit());
    zirc::telemetry::command_started(&matches);
    match zirc::run(cli, &mut std::io::stdout()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // Whatever read what it printed stopped reading, as `zirc status | head` does:
        // nothing went wrong here.
        Err(err) if zirc::stopped_reading(&err) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{PROGRAM}: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
