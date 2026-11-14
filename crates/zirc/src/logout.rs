//! `zirc logout`: signing this machine out, of everything, of one organisation (`zirc
//! logout acme`, which `zirc orgs remove acme` still does too) or of one guest's terminal
//! (`zirc logout acme/web`). What
//! is asked, and what leaves, is zirc-core's ([`zirc_core::signout`]), in the apps' words
//! ([`zirc_core::account_text`]), so every app and the CLI mean the same by it.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use clap::{Args, Subcommand};
use zirc_core::device_key::Software;
use zirc_core::ipc::{Request, Response};
use zirc_core::paths;
use zirc_core::settings::{Organisation, Settings};
use zirc_core::signout::{Plan, Scope, revoke_key};

use crate::client::{ask, shown};
use crate::daemon::stop;
use crate::setup::earlier_setup;
use crate::{guest, service};

/// `zirc logout`'s arguments.
#[derive(Debug, Args)]
pub struct Logout {
    /// The one organisation to take off this machine, as `zirc orgs` names it, such as
    /// acme; or the guest's terminal to end, such as acme/web. Without one, signs this
    /// machine out of everything.
    #[arg(conflicts_with = "all", value_name = "ORG")]
    target: Option<String>,
    /// Sign out of everything: what plain `zirc logout` does.
    #[arg(long)]
    all: bool,
    #[command(flatten)]
    how: How,
}

/// How signing out goes, for every way of doing it.
#[derive(Debug, Args)]
pub struct How {
    /// Don't ask first.
    #[arg(short, long)]
    yes: bool,
    /// Forget this machine's sign-in here only, without telling zirc.dev to revoke its key.
    /// What signing out does anyway when zirc.dev can't be reached.
    #[arg(long)]
    local: bool,
    /// Leave zircd as it is: it stops being signed in when it next starts.
    #[arg(long)]
    no_daemon: bool,
}

/// `zirc orgs`' subcommands.
#[derive(Debug, Subcommand)]
pub enum Orgs {
    /// List the organisations this machine is signed in to.
    List {
        /// Print JSON: an array on one line.
        #[arg(long)]
        json: bool,
    },
    /// Take an organisation off this machine.
    ///
    /// Forgets its server, its key and its connection. The others, your account's other devices and every message stay. The last one also signs
    /// out of zirc.dev, as `zirc logout` does.
    Remove {
        /// The organisation, as `zirc orgs` names it.
        org: String,
        #[command(flatten)]
        how: How,
    },
}

/// `zirc logout`.
pub async fn logout(home: &Path, args: Logout, out: &mut (impl Write + Send)) -> Result<()> {
    let input = &mut std::io::BufReader::new(std::io::stdin());
    match args.target {
        Some(target) if !args.all => {
            let name = guest::org_of(&target);
            sign_out(home, &Scope::One(name), Kind::One, &args.how, input, out).await
        }
        _ => {
            sign_out(
                home,
                &Scope::Everything,
                Kind::Everything,
                &args.how,
                input,
                out,
            )
            .await
        }
    }
}

/// `zirc orgs`.
pub async fn orgs(home: &Path, command: Option<Orgs>, out: &mut (impl Write + Send)) -> Result<()> {
    match command.unwrap_or(Orgs::List { json: false }) {
        Orgs::List { json } => list(home, json, out),
        Orgs::Remove { org, how } => {
            let input = &mut std::io::BufReader::new(std::io::stdin());
            sign_out(home, &Scope::One(org), Kind::Own, &how, input, out).await
        }
    }
}

fn list(home: &Path, json: bool, out: &mut impl Write) -> Result<()> {
    let settings = earlier_setup(&paths::config(home), false)?;
    if json {
        let orgs: Vec<serde_json::Value> = settings
            .iter()
            .flat_map(Settings::organisations)
            .map(|org| {
                let guest = org.guest.as_ref().map(zirc_core::guest::of);
                serde_json::json!({ "name": org.name(), "server": org.server.host, "guest": guest })
            })
            .collect();
        writeln!(out, "{}", serde_json::Value::Array(orgs))?;
        return Ok(());
    }
    let Some(settings) = settings else {
        writeln!(
            out,
            "This machine isn't logged in to anything: `zirc login` does."
        )?;
        return Ok(());
    };
    for org in settings.organisations() {
        match &org.guest {
            Some(guest) => writeln!(
                out,
                "{}  guest in {}",
                org.name(),
                zirc_core::guest::of(guest)
            )?,
            None => writeln!(out, "{}  {}", org.name(), org.server.host)?,
        }
    }
    Ok(())
}

/// What the person named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Everything,
    /// One organisation, its own or a guest's terminal, whichever the name is: by `zirc
    /// logout <org>`.
    One,
    /// A guest's terminal: what [`Kind::One`] turns out to be when the name is one's.
    Guest,
    /// An organisation of one's own, by `zirc orgs remove`.
    Own,
}

/// Whether `answer` says yes.
fn yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Signs out of `scope`: says what leaves and who loses their connection, asks (unless
/// `how.yes`, or `kind` is a guest's terminal, which never asked), tells zirc.dev this
/// machine's key is gone (unless `how.local`, or it can't be told: then says so), forgets
/// the sign-in and restarts zircd without it, or stops it when nothing is left.
async fn sign_out(
    home: &Path,
    scope: &Scope,
    kind: Kind,
    how: &How,
    input: &mut (impl BufRead + Send),
    out: &mut (impl Write + Send),
) -> Result<()> {
    let path = paths::config(home);
    let Some(mut settings) = earlier_setup(&path, false)? else {
        if kind == Kind::Everything {
            writeln!(out, "This machine isn't logged in to anything.")?;
            return Ok(());
        }
        bail!(
            "this machine isn't logged in to anything: there's no {}",
            shown(&path)
        );
    };
    let plan = Plan::new(&settings, scope)?;
    let socket = paths::socket(home);
    let mut kind = kind;
    if let Scope::One(name) = scope {
        let guest = plan.leaving[0].guest.is_some();
        kind = match (kind, guest) {
            (Kind::Own, true) => {
                bail!("{name} is a guest's terminal here: `zirc logout {name}` ends it")
            }
            (Kind::One, true) => Kind::Guest,
            (Kind::One, false) => Kind::Own,
            (kind, _) => kind,
        };
    }

    let sheet = plan.sheet();
    if kind != Kind::Guest {
        writeln!(out, "{}", sheet.title)?;
        writeln!(out, "{}", sheet.body)?;
        let agents = running_agents(&socket).await;
        if !agents.is_empty() {
            writeln!(
                out,
                "Agent sessions here lose their connection: {}.",
                agents.join(", ")
            )?;
        }
        if !how.yes {
            write!(out, "{}? [y/N] ", sheet.confirm)?;
            out.flush()?;
            let mut answer = String::new();
            input.read_line(&mut answer)?;
            if !yes(&answer) {
                writeln!(out, "Nothing changed.")?;
                return Ok(());
            }
        }
    }

    let key = Software::machine(home);
    let leaving: Vec<Organisation> = settings
        .organisations()
        .into_iter()
        .filter(|org| plan.leaving.iter().any(|held| held.org == org.name()))
        .collect();
    let unrevoked = revoke_all(&leaving, &key, how.local, out).await?;
    zirc_core::signout::forget(home, &path, &mut settings, &plan)
        .with_context(|| format!("changing {}", shown(&path)))?;
    for held in &plan.leaving {
        match &held.guest {
            Some(public) => writeln!(
                out,
                "Logged out: this machine is no longer your terminal in {public}."
            )?,
            None => writeln!(out, "{}", zirc_core::account_text::signed_out(&held.org))?,
        }
    }
    if plan.ends_zirc_dev() {
        writeln!(out, "Signed out of zirc.dev.")?;
    }
    if !unrevoked.is_empty() {
        writeln!(
            out,
            "This machine's key is forgotten here, but zirc.dev still accepts it for {}: revoke \
             the device on your account page at zirc.dev, or run `zirc login` and \
             `zirc logout` again while online.",
            unrevoked.join(", ")
        )?;
    }

    if !how.no_daemon {
        if plan.leaves_nothing() {
            let _ = stop(&socket, &mut std::io::sink()).await;
            service::uninstall(&mut std::io::sink())?;
            writeln!(out, "zircd is stopped, and no longer runs at login.")?;
        } else if ask(&socket, Request::Restart).await.is_ok() {
            writeln!(out, "zircd is restarting without it.")?;
        }
    }
    Ok(())
}

/// Tells zirc.dev this machine's key is gone for each of `leaving` (unless `local`), saying
/// how it went; the organisations it didn't reach, or wasn't asked to, are the answer.
async fn revoke_all(
    leaving: &[Organisation],
    key: &Software,
    local: bool,
    out: &mut impl Write,
) -> Result<Vec<String>> {
    let mut unrevoked = Vec::new();
    for org in leaving {
        if local {
            if org.account.as_ref().is_some_and(|a| a.device_key) {
                unrevoked.push(org.name());
            }
            continue;
        }
        match revoke_key(org, key).await {
            Ok(true) => writeln!(
                out,
                "Revoked this machine's key for {} at zirc.dev.",
                org.name()
            )?,
            Ok(false) => {}
            Err(why) => {
                writeln!(out, "Couldn't reach zirc.dev for {}: {why}.", org.name())?;
                unrevoked.push(org.name());
            }
        }
    }
    Ok(unrevoked)
}

/// Takes `plan`'s organisations out of `settings` and writes them, or removes the file, and
/// this machine's key with it, when nothing stays.
/// The agent sessions with a watch running on this machine, which lose their connection;
/// none when zircd isn't running.
async fn running_agents(socket: &Path) -> Vec<String> {
    match ask(socket, Request::Status).await {
        Ok(Response::Status(status)) => status.watching.into_iter().flatten().collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
