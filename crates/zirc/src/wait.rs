//! `zirc wait`: a command that ends when what it waits for happened, or failed. A wait is a
//! process, so the tool that started it shows it, its ending is the wake, and stopping it
//! is how to stop waiting. It reads the repository's channels back first, so one started
//! after the fact ends at once, then follows zircd as `zirc watch` does, across its restarts.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::{Args, Subcommand};
use zirc_core::ipc::{self, Delivery, Heard, Request, Response};
use zirc_core::states;
use zirc_core::waits::{Condition, Outcome, Want};
use zirc_proto::state::Level;

use crate::args::{OrgArg, duration_arg};
use crate::catchup::now;
use crate::client::{ask, ask_in, here, not_running};
use crate::daemon::socket_id;
use crate::lines::{line, one_line};
use crate::repo::repo_here;
use crate::watch::{ONCE_REWATCH_WAIT, orphaned, rewatch};

/// How many of a channel's newest messages a wait reads back.
const READ_BACK: usize = 500;

#[derive(Debug, Subcommand)]
pub(crate) enum WaitCommand {
    /// Wait until a landing lands: a branch, a build by its number, or a session's.
    ///
    /// Such as zirc wait landed api-auth, or zirc wait landed 1299. Prints CI's line and
    /// ends; ends as failed when CI says the build failed or the branch conflicts.
    Landed(LandedArgs),
    /// Wait until a landing is deployed to production.
    ///
    /// Name it by its branch, its build's number or the commit it landed as, which the line
    /// saying it landed gives: zirc wait deployed 9426072. It ends once every app the
    /// landing deploys is out, web's step and the system apps' alike; --app controller ends
    /// it on that one. Prints CI's line and ends; ends as failed when CI says the deploy
    /// failed.
    Deployed(DeployedArgs),
    /// Wait until a state your team shares, such as master, stands as you say.
    ///
    /// Such as zirc wait state master ok, to land once a broken master is fine: it ends at
    /// once when it stands so already. Without a level, it ends when the state is next said
    /// at another level than it stands at now.
    State(StateArgs),
}

// `zirc wait landed`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct LandedArgs {
    /// A branch, a build by its number, or a session.
    pub(crate) on: String,
    #[command(flatten)]
    pub(crate) how: HowArgs,
}

// `zirc wait deployed`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct DeployedArgs {
    /// The landing: its branch, its build by its number, or its commit.
    pub(crate) on: String,
    /// Only a deploy with this app in it, such as controller.
    #[arg(long, value_name = "APP")]
    pub(crate) app: Option<String>,
    #[command(flatten)]
    pub(crate) how: HowArgs,
}

// `zirc wait state`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct StateArgs {
    /// What it's the state of, such as master.
    #[arg(value_name = "NAME")]
    pub(crate) state_name: String,
    /// How it must stand: ok, warning or broken.
    #[arg(value_name = "LEVEL", value_parser = crate::state::level)]
    pub(crate) level: Option<Level>,
    #[command(flatten)]
    pub(crate) how: HowArgs,
}

// What every `zirc wait` takes: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct HowArgs {
    /// Give up after this long, such as 45m or 3h, as failed: 3h unless said.
    #[arg(long = "for", value_name = "TIME", default_value = "3h", value_parser = duration_arg)]
    pub(crate) seconds: u64,
    /// How far back to look for a landing or a deploy having happened already, such as 3h.
    #[arg(long, value_name = "TIME", default_value = "3h", value_parser = duration_arg)]
    pub(crate) back: u64,
    /// The repository whose CI says it. Defaults to the current directory.
    #[arg(long)]
    pub(crate) repo: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// `zirc wait`.
pub(crate) async fn wait(socket: &Path, command: WaitCommand, out: &mut impl Write) -> Result<()> {
    let (how, asked) = match command {
        WaitCommand::Landed(args) => (args.how, Asked::Ci(Condition::landed(&args.on))),
        WaitCommand::Deployed(args) => (
            args.how,
            Asked::Ci(Condition::deployed(&args.on, args.app.as_deref())),
        ),
        WaitCommand::State(args) => (args.how, Asked::State(args.state_name, args.level)),
    };
    let Response::Repos { repos } = ask(socket, Request::RepoList).await? else {
        return Err(crate::client::unexpected());
    };
    let cwd = here(how.repo)?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let repo = repo_here(&repos, &cwd)
        .context("run it in a repository zirc watches: CI says these in its channels")?;
    let org = how.org.name.or_else(|| repo.org.clone());
    let mut place = Place {
        socket,
        org: org.as_deref(),
        channel: &repo.channel,
    };
    let mut condition = match asked {
        Asked::Ci(condition) => condition,
        Asked::State(name, level) => {
            if let Some(problem) = zirc_proto::state::check_name(&name) {
                bail!("{problem}");
            }
            // States are said in the organisation's #ops, each naming its repository.
            place.channel = states::CHANNEL;
            let want = if let Some(level) = level {
                Want::At(level)
            } else {
                let any = Condition::stands(Some(&repo.channel), &name, Want::Not(None));
                Want::Not(place.stands(&any).await?.map(|(level, _)| level))
            };
            Condition::stands(Some(&repo.channel), &name, want)
        }
    };
    // Following before reading back, so nothing said in between is missed.
    let mut watching = ipc::watch(socket, None)
        .await
        .map_err(|err| not_running(socket, err))?;
    let mut watched = socket_id(socket);
    let since = now().saturating_sub(how.back);
    if let Some(ended) = place.read_back(&mut condition, since).await? {
        return ended.say(&condition, out);
    }
    eprintln!(
        "waiting for {}: this ends when CI says it has, or that it failed, or in {} minutes",
        condition.describe(),
        how.seconds / 60
    );
    let limit = tokio::time::sleep(Duration::from_secs(how.seconds));
    tokio::pin!(limit);
    loop {
        let next = tokio::select! {
            next = watching.next() => next,
            () = &mut limit => bail!(
                "waited {} minutes for {}: CI said nothing of it",
                how.seconds / 60,
                condition.describe()
            ),
            // Whatever started the wait is gone, and nobody reads it.
            () = orphaned() => return Ok(()),
        };
        match next {
            Ok(Some(Heard::Message(delivery))) => {
                if let Some(ended) = place.judge(&mut condition, delivery) {
                    return ended.say(&condition, out);
                }
            }
            Ok(Some(_)) => {}
            // zircd stopped, as for an update: follow the new one, and read back what was
            // said meanwhile.
            Ok(None) | Err(ipc::Error::Io(_)) => {
                watching = rewatch(socket, watched, None, ONCE_REWATCH_WAIT).await?;
                watched = socket_id(socket);
                if let Some(ended) = place.read_back(&mut condition, since).await? {
                    return ended.say(&condition, out);
                }
            }
            Err(ipc::Error::Daemon(message)) if message == ipc::RESTARTING => {
                watching = rewatch(socket, watched, None, ONCE_REWATCH_WAIT).await?;
                watched = socket_id(socket);
            }
            Err(err) => return Err(err.into()),
        }
    }
}

/// Where a wait listens: a repository's channel and the ones under it, such as its
/// landings, or for a state the organisation's `#ops`, in one organisation.
struct Place<'a> {
    socket: &'a Path,
    org: Option<&'a str>,
    channel: &'a str,
}

/// How a wait ended, and the line that ended it.
struct Ended {
    outcome: Outcome,
    /// None for a state never said, which is fine.
    delivery: Option<Delivery>,
}

/// What a wait was asked for: something CI says, or a state by its name and the level it
/// must stand at.
enum Asked {
    Ci(Condition),
    State(String, Option<Level>),
}

impl Place<'_> {
    /// Whether `channel` is the repository's or one under it.
    fn has(&self, channel: &str) -> bool {
        channel
            .strip_prefix(self.channel)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }

    /// How `delivery` ends the wait for `condition`, if it's said here and does.
    fn judge(&self, condition: &mut Condition, delivery: Delivery) -> Option<Ended> {
        let elsewhere = matches!((&delivery.org, self.org), (Some(its), Some(ours)) if its != ours);
        if elsewhere || !self.has(&delivery.channel) {
            return None;
        }
        let outcome = condition.judge(&delivery.message.event)?;
        Some(Ended {
            outcome,
            delivery: Some(delivery),
        })
    }

    /// How the state `condition` waits on stands, as the newest message here saying it
    /// has it, with that message: none when it was never said, or isn't a wait on a state.
    /// One that has ended by itself, as a hold whose time has passed, is fine.
    async fn stands(&self, condition: &Condition) -> Result<Option<(Level, Delivery)>> {
        let request = Request::Read {
            target: self.channel.to_owned(),
            limit: states::LOOK_BACK,
            since: None,
            until: None,
        };
        let Response::Messages { messages } = ask_in(self.socket, self.org, request).await? else {
            return Err(crate::client::unexpected());
        };
        let now = zirc_core::time::tag(now());
        Ok(messages.into_iter().rev().find_map(|delivery| {
            let state = condition.state_in(&delivery.message.event)?;
            let level = if state.ended(&now) {
                Level::Ok
            } else {
                state.level
            };
            Some((level, delivery))
        }))
    }

    /// How what was said here since `since` ends the wait for `condition`, if it does:
    /// every channel's messages, oldest first; for a state, how it stands now, whenever
    /// that was said.
    async fn read_back(&self, condition: &mut Condition, since: u64) -> Result<Option<Ended>> {
        if matches!(condition, Condition::Stands { .. }) {
            let stands = self.stands(condition).await?;
            let level = stands.as_ref().map(|(level, _)| *level);
            return Ok(condition.met_at(level).then_some(Ended {
                outcome: Outcome::Met,
                delivery: stands.map(|(_, delivery)| delivery),
            }));
        }
        let Response::Status(status) = ask(self.socket, Request::Status).await? else {
            return Err(crate::client::unexpected());
        };
        let joined = match self.org {
            Some(org) => status
                .orgs
                .iter()
                .find(|it| it.name == org)
                .map_or(&status.channels, |it| &it.channels),
            None => &status.channels,
        };
        let mut said = Vec::new();
        for channel in joined.iter().filter(|channel| self.has(channel)) {
            let request = Request::Read {
                target: channel.clone(),
                limit: READ_BACK,
                since: Some(since),
                until: None,
            };
            let Response::Messages { messages } = ask_in(self.socket, self.org, request).await?
            else {
                return Err(crate::client::unexpected());
            };
            said.extend(messages);
        }
        // RFC 3339 in UTC, as the server stamps them, sorts as text.
        said.sort_by(|a, b| a.time.cmp(&b.time));
        Ok(said
            .into_iter()
            .find_map(|delivery| self.judge(condition, delivery)))
    }
}

impl Ended {
    /// Prints the line that ended the wait for `condition`, as `zirc watch` prints one;
    /// a failure is the command's too, so `zirc wait … && …` goes on only when it's met.
    fn say(self, condition: &Condition, out: &mut impl Write) -> Result<()> {
        match &self.delivery {
            // On one line, though CI's text may take several.
            Some(delivery) => writeln!(out, "{}", one_line(&line(delivery)))?,
            None => writeln!(out, "nothing says otherwise: it's fine")?,
        }
        out.flush()?;
        match self.outcome {
            Outcome::Met => Ok(()),
            Outcome::Failed => bail!("waited for {}: CI says it failed", condition.describe()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listens_in_the_repositorys_channel_and_those_under_it() {
        let place = Place {
            socket: Path::new("/nowhere"),
            org: None,
            channel: "#api",
        };
        assert!(place.has("#api"));
        assert!(place.has("#api/landings"));
        assert!(!place.has("#api-docs"));
        assert!(!place.has("#ops"));
    }
}
