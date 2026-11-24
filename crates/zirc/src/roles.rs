//! `zirc role` and `zirc roles`: a role's one seat taken, left and spoken from
//! ([`zirc_proto::roles`]), and how the organisation's roles stand.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use clap::Subcommand;
use zirc_core::ipc::{Request, Response};
use zirc_proto::PROGRAM;
use zirc_proto::roles::{Holder, Seat, Standing};

use crate::args::session_or_user;
use crate::client::{ask, unexpected};

// `zirc role`'s commands: their doc comments are their --help.
#[derive(Debug, Subcommand)]
pub(crate) enum RoleCommand {
    /// Take a role's one seat, such as the fixer, in a checkout of its repository.
    ///
    /// Prints the role's brief: what it does is your job from now on. You hold it while your
    /// watch runs; leaving it, or your session ending, frees it. Refused while another
    /// session holds it.
    Take {
        /// The role, such as fixer.
        role: String,
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Leave the role you hold.
    Leave {
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Say something as the role you hold: to someone directly, or in #agents.
    Say {
        /// Whom to, by nick: someone who wrote to the role. Without it, #agents.
        #[arg(long)]
        to: Option<String>,
        /// The message it answers, by the id your watch showed.
        #[arg(long)]
        reply_to: Option<String>,
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
        /// What to say.
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Say who held a role last, to ask them how things stood: `zirc ask --to` them.
    Last {
        /// The role, such as fixer.
        role: String,
    },
}

/// `zirc role`.
pub(crate) async fn role(socket: &Path, command: RoleCommand, out: &mut impl Write) -> Result<()> {
    match command {
        RoleCommand::Take { role, session } => {
            let (repo, _) = crate::gotchas::here(socket, &[]).await?;
            let request = Request::RoleTake {
                role,
                session: session_or_user(session),
                repo: Some(repo.scope.trim_start_matches('#').to_owned()),
            };
            let Response::RoleTaken { role, seat } = ask(socket, request).await? else {
                return Err(unexpected());
            };
            writeln!(out, "you're the {} in {}", role.name, role.repository)?;
            writeln!(out, "  ↳ brief: {}", role.brief.trim())?;
            writeln!(out, "  ↳ wakes on: {}", role.wakes_on().join("; "))?;
            if let Some(last) = &seat.last {
                writeln!(out, "  ↳ {}", last_words(&role.name, last))?;
            }
            Ok(())
        }
        RoleCommand::Leave { session } => {
            let request = Request::RoleLeave {
                session: session_or_user(session),
            };
            let Response::Done = ask(socket, request).await? else {
                return Err(unexpected());
            };
            Ok(writeln!(out, "left the role")?)
        }
        RoleCommand::Say {
            to,
            reply_to,
            session,
            text,
        } => {
            let request = Request::RoleSay {
                session: session_or_user(session),
                to: to.clone(),
                reply_to,
                text: text.join(" "),
            };
            let Response::Done = ask(socket, request).await? else {
                return Err(unexpected());
            };
            let to = to.as_deref().unwrap_or(zirc_proto::AGENTS_CHANNEL);
            Ok(writeln!(out, "said to {to}")?)
        }
        RoleCommand::Last { role } => {
            let standing = standing(socket).await?;
            let seat = seat_of(&standing, &role)?;
            match &seat.last {
                Some(last) => writeln!(out, "{}", last_words(&role, last))?,
                None => writeln!(out, "nobody has held the {role} before")?,
            }
            Ok(())
        }
    }
}

/// `zirc roles`, or with `avatars`, `zirc avatars`: the pictures `--avatar` takes.
pub(crate) async fn list(socket: &Path, avatars: bool, out: &mut impl Write) -> Result<()> {
    if avatars {
        return Ok(write!(out, "{}", crate::avatars_text())?);
    }
    roles(socket, out).await
}

/// `zirc roles`: each role, who holds it, and what wakes it.
async fn roles(socket: &Path, out: &mut impl Write) -> Result<()> {
    let standing = standing(socket).await?;
    if standing.listing.roles.is_empty() {
        writeln!(
            out,
            "no roles: your organisation's admins add them on zirc.dev"
        )?;
        return Ok(());
    }
    for seat in &standing.seats {
        let held = seat.status(
            seat.holder
                .as_ref()
                .map_or("", |holder| holder.machine.as_str()),
        );
        writeln!(out, "{} ({}): {held}", seat.role, seat.repository)?;
        writeln!(out, "  wakes on: {}", seat.wakes_on.join("; "))?;
        if seat.holder.is_none() {
            writeln!(
                out,
                "  take it: {}",
                zirc_proto::roles::take_command(&seat.role)
            )?;
        }
    }
    Ok(())
}

/// How the roles stand, from the organisation's server.
async fn standing(socket: &Path) -> Result<Standing> {
    let Response::RoleStanding(standing) = ask(socket, Request::RoleStanding).await? else {
        return Err(unexpected());
    };
    Ok(standing)
}

fn seat_of<'a>(standing: &'a Standing, role: &str) -> Result<&'a Seat> {
    standing
        .seats
        .iter()
        .find(|seat| seat.role == role)
        .ok_or_else(|| anyhow::anyhow!("there's no {role} role: `{PROGRAM} roles` lists them"))
}

/// Who held `role` last, and how to ask them.
fn last_words(role: &str, last: &Holder) -> String {
    format!(
        "{} on {} held the {role} last: ask it with {PROGRAM} ask --to {}/{} \"…\"",
        last.who_with_tool(),
        last.machine,
        last.machine,
        last.session.session
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_to_ask_the_last_holder() {
        let last = Holder {
            machine: "henrik-mbp".into(),
            since: 1,
            session: zirc_proto::session::SessionCard {
                session: "cc-1".into(),
                name: Some("main-fix".into()),
                title: String::new(),
                tool: None,
            },
        };
        assert_eq!(
            last_words("fixer", &last),
            "main-fix on henrik-mbp held the fixer last: ask it with zirc ask --to \
             henrik-mbp/cc-1 \"…\""
        );
    }
}
