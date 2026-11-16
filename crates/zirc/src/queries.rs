//! Agents asking each other: `zirc note`, `ask`, `reply`, and who may query this machine's.

use std::io::{IsTerminal as _, Write};
use std::path::Path;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use zirc_core::agents;
use zirc_core::ipc::{Request, Response};

use crate::args::{OrgArg, session_or_user, words};
use crate::client::{ask, ask_in, org_here};
use crate::post::{print_channel, print_posted};

#[derive(Debug, Subcommand)]
pub(crate) enum AgentsCommand {
    /// Say, or set, who may query this machine's agents with `zirc ask`.
    ///
    /// off, mine (this machine's other sessions, and your own account and machines, by
    /// default) or anyone, which lets any member's agents try to steer yours.
    Queries {
        /// Who may.
        who: Option<Who>,
        /// Let anyone, knowing that their agents could then try to steer yours.
        #[arg(long)]
        yes: bool,
    },
}

/// Who may query this machine's agents, as `zirc agents queries` takes it.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum Who {
    /// No one.
    Off,
    /// This machine's other sessions, and your own account and machines.
    Mine,
    /// Anyone signed in to the server: their agents could then try to steer yours.
    Anyone,
}

impl Who {
    pub(crate) fn queries(self) -> agents::Queries {
        match self {
            Self::Off => agents::Queries::Off,
            Self::Mine => agents::Queries::Mine,
            Self::Anyone => agents::Queries::Anyone,
        }
    }
}

// `zirc note`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct NoteArgs {
    /// The session it's for: its id, its name, or <machine>/<session>.
    pub(crate) to: String,
    /// What to say, or - to read it from stdin.
    #[arg(required = true, num_args = 1..)]
    pub(crate) text: Vec<String>,
    /// Who's writing: an agent session's id. Defaults to $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc ask`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct AskArgs {
    /// The question.
    #[arg(required = true, num_args = 1..)]
    pub(crate) text: Vec<String>,
    /// Whom: <machine>, <machine>/<session>, or a person's nick for their machines'.
    #[arg(long)]
    pub(crate) to: String,
    /// Who's asking: an agent session's id. Defaults to $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc reply`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct ReplyArgs {
    /// The query's id, as `query from … [<id>]` showed it.
    pub(crate) id: String,
    /// The answer.
    #[arg(required = true, num_args = 1..)]
    pub(crate) text: Vec<String>,
    /// Who's answering: an agent session's id. Defaults to $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    /// The organisation the query came from, on a machine signed in to several.
    /// Defaults to whichever heard it.
    #[arg(long, value_name = "NAME", hide = true)]
    pub(crate) org: Option<String>,
}

/// `zirc note`.
pub(crate) async fn note(socket: &Path, args: NoteArgs, out: &mut impl Write) -> Result<()> {
    let NoteArgs {
        to,
        text,
        session,
        org,
    } = args;
    let org = org_here(socket, org).await?;
    let request = Request::Note {
        session: session_or_user(session),
        to,
        text: words(&text)?,
    };
    print_posted(ask_in(socket, org.as_deref(), request).await?, out)
}

/// `zirc ask`.
pub(crate) async fn query(socket: &Path, args: AskArgs, out: &mut impl Write) -> Result<()> {
    let AskArgs {
        text,
        to,
        session,
        org,
    } = args;
    let org = org_here(socket, org).await?;
    let request = Request::Ask {
        session: session_or_user(session),
        to,
        text: words(&text)?,
    };
    let Response::Asked { id, to } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    writeln!(out, "asked {} [{id}]", to.join(", "))?;
    Ok(())
}

/// `zirc reply`.
pub(crate) async fn reply(socket: &Path, args: ReplyArgs, out: &mut impl Write) -> Result<()> {
    let ReplyArgs {
        id,
        text,
        session,
        org,
    } = args;
    let request = Request::Reply {
        session: session_or_user(session),
        query: id,
        text: words(&text)?,
    };
    print_channel(
        ask_in(socket, org.as_deref(), request).await?,
        "answered",
        out,
    )
}

/// `zirc agents`.
pub(crate) async fn agents(
    socket: &Path,
    command: AgentsCommand,
    out: &mut impl Write,
) -> Result<()> {
    let AgentsCommand::Queries { who, yes } = command;
    let set = who.map(Who::queries);
    if set == Some(agents::Queries::Anyone) {
        // A person's choice, never an agent's: an agent talked into it by a query
        // runs without a terminal.
        if !std::io::stdin().is_terminal() {
            bail!(
                "only a person at a terminal can let anyone query this machine's \
                 agents: run it yourself"
            );
        }
        if !yes {
            bail!(
                "{}\nTo let anyone query them all the same: zirc agents queries anyone \
                 --yes",
                agents::ANYONE_WARNING
            );
        }
    }
    let Response::Queries {
        queries,
        colleagues_kept_out,
    } = ask(socket, Request::Queries { set }).await?
    else {
        return Err(crate::client::unexpected());
    };
    writeln!(out, "{}", agents::said(queries, colleagues_kept_out))?;
    Ok(())
}
