//! `zirc react`: an emoji on a message, as people's apps put one, from an agent session.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;
use zirc_core::emoji::{self, Newest};
use zirc_core::ipc::{Request, Response};

use crate::args::{OrgArg, session_or_user};
use crate::client::{ask, ask_in, waits};
use crate::lines::{SHORT_ID, one_line};
use crate::repo::repo_here;

/// The newest emoji the apps draw, as zirc-tui's picker offers them.
pub(crate) const NEWEST: Newest = (15, 0);

#[expect(
    clippy::doc_markdown,
    reason = "the doc comments are --help text, where backticks around :white_check_mark: would print"
)]
#[derive(Debug, Args)]
pub struct ReactArgs {
    /// The message's id, or its start as zirc watch shows it ([id 01jbx7ab]), then the
    /// emoji: itself or its shortcode, such as :eyes:, :+1: or :white_check_mark:. With
    /// --last, only the emoji.
    #[arg(required = true, num_args = 1..=2)]
    args: Vec<String>,
    /// React to the latest message from this nick or session instead of naming one.
    #[arg(long, value_name = "WHO")]
    last: Option<String>,
    /// Where the message was said, a channel or a nick: needed for one zircd hasn't heard
    /// lately, such as one zirc search found.
    #[arg(long = "in", value_name = "CONVERSATION")]
    to: Option<String>,
    /// Take the reaction back.
    #[arg(long)]
    remove: bool,
    /// Who's reacting, such as an agent session's id. Defaults to $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    session: Option<String>,
    #[command(flatten)]
    org: OrgArg,
}

/// How many of a conversation's newest messages `zirc react` offers when it's given none.
const OFFERED: usize = 8;

/// Lists what was said lately where `args` point, `--in` or the repository's channel here,
/// each with the command that reacts to it with `typed`: `zirc react :eyes:` alone once
/// only said what was missing.
async fn offer(socket: &Path, args: &ReactArgs, typed: &str, out: &mut impl Write) -> Result<()> {
    let org = args.org.name.as_deref();
    let target = if let Some(to) = &args.to {
        to.clone()
    } else {
        let Response::Repos { repos } = ask(socket, Request::RepoList).await? else {
            return Err(crate::client::unexpected());
        };
        let cwd = std::env::current_dir()?;
        let cwd = cwd.canonicalize().unwrap_or(cwd);
        let Some(repo) = repo_here(&repos, &cwd) else {
            bail!("say which message and the emoji: zirc react <id> {typed}");
        };
        repo.channel.clone()
    };
    let read = Request::Read {
        target: target.clone(),
        limit: OFFERED,
        since: None,
        until: None,
    };
    let Response::Messages { messages } = waits(socket, org, read).await? else {
        return Err(crate::client::unexpected());
    };
    let said: Vec<String> = messages
        .iter()
        .filter_map(|delivery| {
            let id = delivery.id.as_deref()?;
            let id = id.get(..SHORT_ID).unwrap_or(id);
            let text = one_line(&crate::lines::said(delivery));
            let text: String = text.chars().take(OFFER_WIDTH).collect();
            Some(format!("  zirc react {id} {typed}   {text}"))
        })
        .collect();
    if said.is_empty() {
        bail!("nothing was said in {target} lately to react to: zirc react <id> {typed}");
    }
    writeln!(out, "which message in {target}? Run one of:")?;
    for line in said {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// How much of each message [`offer`] shows, in characters.
const OFFER_WIDTH: usize = 70;

/// Runs `zirc react`.
pub async fn react(socket: &Path, args: ReactArgs, out: &mut impl Write) -> Result<()> {
    let (message, typed) = match (&args.last, args.args.as_slice()) {
        (Some(_), [typed]) => (None, typed),
        (None, [id, typed]) => (Some(id.clone()), typed),
        (Some(_), _) => bail!("with --last, say only the emoji"),
        // Only the emoji: what was said here lately, to pick from.
        (None, [typed]) if emoji::find(typed, NEWEST).is_some() => {
            return offer(socket, &args, typed, out).await;
        }
        (None, _) => bail!("say which message and the emoji: zirc react <id> :eyes:"),
    };
    let Some(emoji) = emoji::find(typed, NEWEST) else {
        bail!("{typed} isn't an emoji or an emoji's shortcode, such as :eyes: or :+1:");
    };
    let request = Request::React {
        repo: std::env::current_dir()?,
        session: session_or_user(args.session),
        message,
        last: args.last,
        to: args.to,
        emoji: emoji.to_owned(),
        remove: args.remove,
    };
    let Response::Channel { channel, .. } =
        ask_in(socket, args.org.name.as_deref(), request).await?
    else {
        return Err(crate::client::unexpected());
    };
    let done = if args.remove { "took back" } else { "reacted" };
    writeln!(out, "{done} {emoji} in {channel}")?;
    Ok(())
}
