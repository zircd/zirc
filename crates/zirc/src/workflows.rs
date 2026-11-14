//! `zirc workflow` and `zirc step`: the organisation's workflows ([`zirc_proto::workflow`]),
//! which the server runs, each as a nick of its own.
//!
//! A workflow's file is kept in the repository as `.zirc/workflows/<name>.toml`, where these
//! commands find it by name: its nick and channel say where a run is started and where a
//! step's worker says what they did.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::Subcommand;
use zirc_core::ipc::{Request, Response};
use zirc_proto::Event;
use zirc_proto::run::{Act, StepAction};
use zirc_proto::workflow::{CHANNEL, Deploy, HOST, Workflow};

use crate::args::session_or_user;
use crate::client::{ask, here};
use crate::post::print_channel;

/// Where a repository keeps its workflows' files.
pub(crate) const DIR: &str = ".zirc/workflows";

// `zirc workflow`'s commands: their doc comments are their --help.
#[derive(Debug, Subcommand)]
pub(crate) enum WorkflowCommand {
    /// Check a workflow's file, and say what it does or what's wrong with it.
    Check {
        /// The file, such as .zirc/workflows/master.toml.
        file: PathBuf,
    },
    /// Send a workflow's file to the server, which runs it as the nick it names.
    ///
    /// It goes to #workflows, where zirc-workflows answers whether it runs. A file of the
    /// same name replaces the one before; its runs carry on.
    Deploy {
        /// The file, such as .zirc/workflows/master.toml.
        file: PathBuf,
        /// Who's sending it. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Start a run of a workflow, with its inputs as name=value.
    Run {
        /// The workflow's name, as its file in .zirc/workflows/ is called.
        name: String,
        /// Its inputs, such as owner=cc-1a2b3c.
        inputs: Vec<String>,
        /// Who's starting it. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
}

// `zirc step`'s commands: their doc comments are their --help.
#[derive(Debug, Subcommand)]
pub(crate) enum StepCommand {
    /// Take on a step a workflow gave you.
    Claim {
        /// The run, such as master-3.
        run: String,
        /// The step, such as fix.
        step: String,
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Say a step is done, with each of its outputs as name=value.
    Done {
        /// The run, such as master-3.
        run: String,
        /// The step, such as fix.
        step: String,
        /// Its outputs, such as cause="a typo".
        outputs: Vec<String>,
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Say you couldn't do a step, which fails its run.
    Fail {
        /// The run, such as master-3.
        run: String,
        /// The step, such as fix.
        step: String,
        /// Why, in a few words.
        #[arg(long)]
        why: String,
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Answer an approval, yes or no: only from a person's own account.
    Answer {
        /// The run, such as spend-2.
        run: String,
        /// The step, such as approve.
        step: String,
        /// yes or no.
        #[arg(value_parser = ["yes", "no"])]
        answer: String,
        /// Who's answering. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Cancel a run.
    Cancel {
        /// The run, such as master-3.
        run: String,
        /// Who's cancelling it. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
}

// `zirc role`'s commands: their doc comments are their --help.
#[derive(Debug, Subcommand)]
pub(crate) enum RoleCommand {
    /// Join a role, to be given its steps while this session is live.
    Join {
        /// The role, such as fixers.
        role: String,
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Leave a role.
    Leave {
        /// The role, such as fixers.
        role: String,
        /// Your session. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
}

/// `zirc role`.
pub(crate) async fn role(socket: &Path, command: RoleCommand, out: &mut impl Write) -> Result<()> {
    let (role, session, join) = match command {
        RoleCommand::Join { role, session } => (role, session, true),
        RoleCommand::Leave { role, session } => (role, session, false),
    };
    let request = Request::Role {
        session: session_or_user(session),
        role,
        join,
    };
    let Response::Roles { roles } = ask(socket, request).await? else {
        return Err(crate::client::unexpected());
    };
    if roles.is_empty() {
        writeln!(out, "in no roles")?;
    } else {
        writeln!(out, "in {}", roles.join(", "))?;
    }
    Ok(())
}

/// The workflow `path`'s file says, and the file.
fn read(path: &Path) -> Result<(Workflow, String)> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read {}", path.display()))?;
    let workflow = zirc_workflows::parse(&source)
        .map_err(|why| anyhow::anyhow!("{}: {why}", path.display()))?;
    Ok((workflow, source))
}

/// The file of the workflow `name`, in `.zirc/workflows/` of the directory `from` is in or
/// the nearest above it that has one.
fn find(from: &Path, name: &str) -> Result<PathBuf> {
    for dir in from.ancestors() {
        let file = dir.join(DIR).join(format!("{name}.toml"));
        if file.is_file() {
            return Ok(file);
        }
    }
    bail!("no {DIR}/{name}.toml here or above: workflows are found by their file's name")
}

/// The workflow run `run` is of, such as `master` for `master-3`.
fn of_run(run: &str) -> Result<&str> {
    run.rsplit_once('-')
        .filter(|(_, n)| n.parse::<u64>().is_ok())
        .map(|(name, _)| name)
        .with_context(|| {
            format!("{run} isn't a run: a run is <workflow>-<number>, such as master-3")
        })
}

/// `pairs` given as name=value.
fn pairs(pairs: &[String]) -> Result<BTreeMap<String, String>> {
    pairs
        .iter()
        .map(|pair| {
            let (name, value) = pair
                .split_once('=')
                .with_context(|| format!("{pair} isn't name=value"))?;
            Ok((name.trim().to_owned(), value.to_owned()))
        })
        .collect()
}

/// What `zirc workflow check` says of `workflow`: what starts it, and its steps.
fn described(workflow: &Workflow) -> String {
    use std::fmt::Write as _;
    let mut text = format!(
        "`{}` is fine: run by @{} in {}\n",
        workflow.name, workflow.nick, workflow.channel
    );
    for trigger in &workflow.on {
        let _ = match (&trigger.state, trigger.level, &trigger.every) {
            (Some(state), Some(level), _) => {
                writeln!(text, "- starts when {state} is {}", level.word())
            }
            (_, _, Some(every)) => {
                let day = trigger.day.as_deref().map(|d| format!(", on {d}"));
                let at = trigger.at.as_deref().map(|at| format!(", at {at} UTC"));
                writeln!(
                    text,
                    "- starts every {every}{}{}",
                    day.unwrap_or_default(),
                    at.unwrap_or_default()
                )
            }
            _ => Ok(()),
        };
    }
    for step in &workflow.steps {
        let kind = step.kind.word();
        let who = step
            .who
            .as_deref()
            .map(|who| format!(" for {who}"))
            .unwrap_or_default();
        let when = step
            .when
            .as_deref()
            .map(|when| format!(", when {when}"))
            .unwrap_or_default();
        let _ = writeln!(text, "- {}: {kind}{who}{when}", step.id);
    }
    text
}

async fn send(socket: &Path, session: String, channel: &str, event: Event) -> Result<String> {
    let request = Request::Workflow {
        repo: here(None)?,
        session,
        channel: channel.to_owned(),
        event,
    };
    let mut said = Vec::new();
    print_channel(ask(socket, request).await?, "sent to", &mut said)?;
    Ok(String::from_utf8_lossy(&said).into_owned())
}

/// `zirc workflow`.
pub(crate) async fn workflow(
    socket: &Path,
    command: WorkflowCommand,
    out: &mut impl Write,
) -> Result<()> {
    match command {
        WorkflowCommand::Check { file } => {
            let (workflow, _) = read(&file)?;
            write!(out, "{}", described(&workflow))?;
        }
        WorkflowCommand::Deploy { file, session } => {
            let (workflow, source) = read(&file)?;
            let deploy = Deploy {
                name: workflow.name.clone(),
                nick: workflow.nick.clone(),
                source,
                by: None,
            };
            let sent = send(socket, session_or_user(session), CHANNEL, deploy.event()).await?;
            write!(out, "{sent}")?;
            writeln!(
                out,
                "{HOST} answers in {CHANNEL} whether @{} runs it: `zirc catchup --since 5m`",
                workflow.nick
            )?;
        }
        WorkflowCommand::Run {
            name,
            inputs,
            session,
        } => {
            let (workflow, _) = read(&find(&here(None)?, &name)?)?;
            let inputs = pairs(&inputs)?;
            let mut text = format!("@{} run", workflow.nick);
            for (name, value) in &inputs {
                let quote = if value.contains(' ') { "\"" } else { "" };
                text = format!("{text} {name}={quote}{value}{quote}");
            }
            let event = Event::Text { body: text };
            let sent = send(socket, session_or_user(session), &workflow.channel, event).await?;
            write!(out, "{sent}")?;
        }
    }
    Ok(())
}

/// `zirc step`.
pub(crate) async fn step(socket: &Path, command: StepCommand, out: &mut impl Write) -> Result<()> {
    let (run, step, act, session) = match command {
        StepCommand::Claim { run, step, session } => (run, step, Act::Claim, session),
        StepCommand::Done {
            run,
            step,
            outputs,
            session,
        } => {
            let outputs = pairs(&outputs)?;
            (run, step, Act::Done { outputs }, session)
        }
        StepCommand::Fail {
            run,
            step,
            why,
            session,
        } => (run, step, Act::Fail { why }, session),
        StepCommand::Answer {
            run,
            step,
            answer,
            session,
        } => (
            run,
            step,
            Act::Answer {
                yes: answer == "yes",
            },
            session,
        ),
        StepCommand::Cancel { run, session } => (run, String::new(), Act::Cancel, session),
    };
    let (workflow, _) = read(&find(&here(None)?, of_run(&run)?)?)?;
    let action = StepAction { run, step, act };
    let sent = send(
        socket,
        session_or_user(session),
        &workflow.channel,
        action.event(),
    )
    .await?;
    write!(out, "{sent}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_names_its_workflow() {
        assert_eq!(of_run("master-3").unwrap(), "master");
        assert_eq!(of_run("ship-it-12").unwrap(), "ship-it");
        assert!(of_run("master").is_err());
        assert!(of_run("master-x").is_err());
    }

    #[test]
    fn workflows_are_found_by_name_from_below() {
        let dir = tempfile::tempdir().unwrap();
        let workflows = dir.path().join(DIR);
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("master.toml"), "").unwrap();
        let deep = dir.path().join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            find(&deep, "master").unwrap(),
            workflows.join("master.toml")
        );
        assert!(find(&deep, "other").is_err());
    }

    #[test]
    fn check_says_what_a_workflow_does() {
        let workflow = zirc_workflows::parse(
            r##"
name = "spend"
nick = "spend_bot"
channel = "#money"

[[step]]
id = "approve"
kind = "approve"
who = "mattias"
"##,
        )
        .unwrap();
        assert_eq!(
            described(&workflow),
            "`spend` is fine: run by @spend_bot in #money\n- approve: approve for mattias\n"
        );
        assert_eq!(
            pairs(&["a=1".into(), "b=x=y".into()]).unwrap(),
            BTreeMap::from([("a".into(), "1".into()), ("b".into(), "x=y".into())])
        );
        assert!(pairs(&["nope".into()]).is_err());
    }
}
