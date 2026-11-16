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
    /// The session waiting, whose messages end the wait too: a direct message or note to it,
    /// or its person's answer to its request, printed so it can act on it at once, rather than
    /// after the wait. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// `zirc wait`.
pub(crate) async fn wait(
    home: &Path,
    socket: &Path,
    command: WaitCommand,
    out: &mut impl Write,
) -> Result<()> {
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
    // What a watch says under a message, said under one that ends the wait too: printed bare,
    // an agent took its person's "stop" for text injected into a tool's output and kept going
    // (the agent evals' M3, 16 Nov 2026).
    let mut hints = crate::hints::watch_hints(home, how.session.as_deref(), &repos);
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
    let started_by = std::os::unix::process::parent_id();
    loop {
        let next = tokio::select! {
            next = watching.next() => next,
            () = &mut limit => bail!(
                "waited {} minutes for {}: CI said nothing of it",
                how.seconds / 60,
                condition.describe()
            ),
            // Whatever started the wait is gone, and nobody reads it.
            () = orphaned(started_by) => return Ok(()),
        };
        match next {
            Ok(Some(Heard::Message(delivery))) => {
                if let Some(ended) = place.judge(&mut condition, delivery) {
                    return ended.say(&condition, out);
                }
            }
            Ok(Some(heard)) if for_session(&heard, how.session.as_deref()) => {
                return stop_for(&heard, &mut hints, &condition, out);
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

/// Ends a wait for `condition` on `heard`, a message for the waiting session: printed, with
/// the hint a watch gives under it, and the wait ended as not happened.
fn stop_for(
    heard: &Heard,
    hints: &mut zirc_core::hints::Watch,
    condition: &Condition,
    out: &mut impl Write,
) -> Result<()> {
    let said = match heard {
        Heard::Direct(direct) => crate::lines::direct_line(direct),
        Heard::Answered(answered) => crate::decide::answered_line(answered),
        _ => String::new(),
    };
    writeln!(out, "{said}")?;
    if let Some(shown) = hints.new_for(heard) {
        writeln!(out, "{}", crate::hints::hint_line(&shown))?;
    }
    bail!(
        "stopped waiting for {}: a message came for you, above. Act on it, then wait again if \
         you still need to",
        condition.describe()
    );
}

/// Whether `heard` is for `session` itself: a direct message or note to it, or its person's
/// answer to its request. A wait run in the foreground holds its session's turn, so what its
/// person says ends it: a "stop" sent while one waited for its landing was read only once the
/// landing it should have cancelled had landed (the agent evals' M3, 16 Nov 2026).
fn for_session(heard: &Heard, session: Option<&str>) -> bool {
    let Some(session) = session.filter(|s| !s.is_empty()) else {
        return false;
    };
    match heard {
        Heard::Direct(direct) => {
            direct.session.as_deref() == Some(session) && direct.answer.is_none()
        }
        Heard::Answered(answered) => answered.request.session == session,
        _ => false,
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

    /// Whether `delivery` was said here: in this organisation, in the repository's channel
    /// or one under it.
    fn says_here(&self, delivery: &Delivery) -> bool {
        let elsewhere = matches!((&delivery.org, self.org), (Some(its), Some(ours)) if its != ours);
        !elsewhere && self.has(&delivery.channel)
    }

    /// How `delivery` ends the wait for `condition`, if it's said here and does.
    fn judge(&self, condition: &mut Condition, delivery: Delivery) -> Option<Ended> {
        if !self.says_here(&delivery) {
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
    /// every channel's messages, oldest first, as [`Condition::read_back`] reads them; for a
    /// state, how it stands now, whenever that was said.
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
        said.retain(|delivery| self.says_here(delivery));
        let answer = condition.read_back(said.iter().map(|delivery| &delivery.message.event));
        Ok(answer.map(|(at, outcome)| Ended {
            outcome,
            delivery: Some(said.swap_remove(at)),
        }))
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
    fn a_message_for_the_waiting_session_ends_its_wait() {
        let direct = |to: Option<&str>, answer: Option<&str>| {
            Heard::Direct(ipc::Direct {
                from: "alice".into(),
                from_session: None,
                session: to.map(str::to_owned),
                text: "stop the rename".into(),
                query: None,
                answer: answer.map(str::to_owned),
                note: false,
                colleague: false,
                id: None,
                why: None,
                org: None,
            })
        };
        assert!(for_session(&direct(Some("cc-1"), None), Some("cc-1")));
        assert!(
            !for_session(&direct(Some("cc-2"), None), Some("cc-1")),
            "another session's"
        );
        assert!(
            !for_session(&direct(None, None), Some("cc-1")),
            "to the machine"
        );
        assert!(
            !for_session(&direct(Some("cc-1"), Some("q1")), Some("cc-1")),
            "an answer it waits on with zirc ask"
        );
        assert!(
            !for_session(&direct(Some("cc-1"), None), None),
            "no session said"
        );
    }

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
