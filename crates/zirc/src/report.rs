//! `zirc report <role>`: something for a role's holder to do, such as a new warning in CI's
//! logs or a flaky test ([`zirc_proto::role_report`]), sent through zircd to `#ops`, where
//! the tenant's sidecar keeps one open item per kind and key and wakes the holder for a new
//! one. The holder closes one with `--done` or `--baselined`; a landing closes it with a
//! `Fixes: report <id>` trailer.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use clap::Args;
use zirc_core::ipc::{Request, Response};
use zirc_proto::role_report::{self, Closing, End, RoleReport, Subject};

use crate::args::{OrgArg, session_or_user};
use crate::client::{ask_in, here, org_here};

/// What `zirc report` takes.
// The doc comments on the arguments are --help text, where backticks would show.
#[derive(Debug, Args)]
pub(super) struct ReportArgs {
    /// The coworker it's for, by its nick, such as john.
    #[arg(value_name = "COWORKER")]
    role: String,
    /// What it's about.
    #[arg(long, value_parser = subjects())]
    kind: Subject,
    /// What makes it the same as an earlier one: a warning's line, a test, a workload.
    /// Reported again while it's open, it adds nothing.
    #[arg(long)]
    key: String,
    /// What it says, in a line. Defaults to the key.
    #[arg(long)]
    summary: Option<String>,
    /// Where to read more, such as the build's log.
    #[arg(long)]
    link: Option<String>,
    /// More of what's known, such as a container's exit code.
    #[arg(long)]
    details: Option<String>,
    /// The CI build it was seen in.
    #[arg(long)]
    build: Option<String>,
    /// Close the open item instead: it's done.
    #[arg(long, conflicts_with = "baselined")]
    done: bool,
    /// Close the open item instead: it can't be fixed here and its key is in the baseline.
    #[arg(long)]
    baselined: bool,
    /// Who's saying it. Defaults to $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    session: Option<String>,
    #[command(flatten)]
    org: OrgArg,
}

/// The kinds, for clap: its --help lists them as zirc-proto has them, so a kind added there
/// is said here too.
fn subjects() -> impl clap::builder::TypedValueParser<Value = Subject> {
    use clap::builder::TypedValueParser as _;
    clap::builder::PossibleValuesParser::new(Subject::ALL.map(Subject::as_str))
        .try_map(|word: String| Subject::named(&word).ok_or_else(role_report::kinds_said))
}

/// `zirc report`.
pub(super) async fn report(socket: &Path, args: ReportArgs, out: &mut impl Write) -> Result<()> {
    let org = org_here(socket, args.org).await?;
    let session = session_or_user(args.session);
    let end = if args.done {
        Some(End::Done)
    } else if args.baselined {
        Some(End::Baselined)
    } else {
        None
    };
    let report = RoleReport {
        summary: args.summary.unwrap_or_default(),
        link: args.link,
        details: args.details,
        build: args.build,
        ..RoleReport::new(&args.role, args.kind, &args.key)
    }
    .checked()?;
    let id = report.id();
    let request = match end {
        Some(end) => Request::CloseReport {
            repo: here(None)?,
            session,
            closing: Closing {
                id: id.clone(),
                role: report.role.clone(),
                end,
            },
        },
        None => Request::Report {
            repo: here(None)?,
            session,
            report: report.clone(),
        },
    };
    let Response::Channel { channel, .. } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    match end {
        Some(end) => writeln!(
            out,
            "{} [{id}] closed, {}, said in {channel}",
            report.role,
            end.words()
        )?,
        None => writeln!(
            out,
            "reported to {} [{id}] in {channel}: {}; a landing closes it with the trailer {}",
            report.role,
            report.words(),
            role_report::fixes_trailer(&id)
        )?,
    }
    Ok(())
}
