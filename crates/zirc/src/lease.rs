//! `zirc lease`: one session at a time on something a team's machines share, such as an
//! emulator, a simulator or a heavy local build, the others queued in order. zircd agrees
//! leases with every machine in the organisation: see [`zirc_core::leases`].

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use zirc_core::ipc::{Request, Response};
use zirc_core::leases::{Holder, Lease, SETTLE};

use crate::args::{OrgArg, duration_arg, session_or_user};
use crate::client::{ask_in, org_here};
use crate::sessions::until;

impl LeaseArgs {
    /// Whether it lists the leases, for a person to read: no name, no subcommand, no JSON.
    pub(super) fn lists(&self) -> bool {
        self.lease.is_none() && self.command.is_none() && !self.json
    }
}

/// How often `--wait` asks whether the lease is the session's yet.
const POLL: Duration = Duration::from_secs(1);

/// What `zirc lease` takes.
// The doc comments on the arguments are --help text, where backticks would show.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(super) struct LeaseArgs {
    #[command(subcommand)]
    command: Option<LeaseCommand>,
    /// What to lease, such as emulator, simulator or mac-ci: one word your team agrees on.
    /// Without one, shows who holds and who's waiting for each lease.
    #[arg(value_name = "NAME")]
    lease: Option<String>,
    /// Who's asking, such as an agent session's id. Defaults to $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    session: Option<String>,
    /// How long to hold it once it's yours, such as 20m or 1h; 20m unless said. Asking
    /// again while you hold it holds it that long from now, but while others wait, 30m in a
    /// row at most (or the --for you first asked, if longer): then you're queued behind them.
    #[arg(long = "for", value_name = "TIME", value_parser = duration_arg)]
    seconds: Option<u64>,
    /// Wait until it's yours before returning.
    #[arg(long)]
    wait: bool,
    /// Print JSON instead of text.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    org: OrgArg,
}

#[derive(Debug, Subcommand)]
enum LeaseCommand {
    /// Give a lease back, or stop waiting for it.
    Release {
        /// The lease.
        #[arg(value_name = "NAME")]
        lease: String,
        /// Who's giving it back. Defaults to $ZIRC_SESSION, then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
        #[command(flatten)]
        org: OrgArg,
    },
}

/// Seconds since the Unix epoch.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `nick/session`.
fn who(holder: &Holder) -> String {
    format!("{}/{}", holder.nick, holder.session)
}

/// Whether `holder` is `session` on this machine.
fn is(holder: &Holder, session: &str) -> bool {
    holder.mine && holder.session == session
}

/// Who holds `lease`: its `holders`, or from a zircd before leases had slots, its `holder`.
fn holders(lease: &Lease) -> &[Holder] {
    if lease.holders.is_empty() {
        lease.holder.as_slice()
    } else {
        &lease.holders
    }
}

/// Whether `session` holds `lease`.
fn holds(lease: &Lease, session: &str) -> bool {
    holders(lease).iter().any(|it| is(it, session))
}

/// Whether `session` holds `lease`, waits for it, or waits for room to ask.
fn asked(lease: &Lease, session: &str) -> bool {
    lease.held.iter().any(|held| held.session == session)
        || holds(lease, session)
        || lease.waiting.iter().any(|it| is(it, session))
}

/// Who holds `lease` and for how long, as `alpha/cc-1 for 20m, beta/cc-2 for 5m`.
fn held_by(lease: &Lease, now: u64) -> Option<String> {
    let all: Vec<String> = holders(lease)
        .iter()
        .map(|holder| format!("{} for {}", who(holder), until(now, holder.until)))
        .collect();
    (!all.is_empty()).then(|| all.join(", "))
}

/// What `lease` is to `session`, in a line.
fn said(lease: &Lease, session: &str, now: u64) -> String {
    let name = &lease.name;
    if let Some(held) = lease.held.iter().find(|held| held.session == session) {
        return format!(
            "lease {name} waits for this machine to have room ({}): it's asked for then, and \
             zirc watch says when it's yours",
            held.why
        );
    }
    if let Some(holder) = holders(lease).iter().find(|it| is(it, session)) {
        format!(
            "lease {name} is yours for {}: zirc lease release {name} --session {} when \
             you're done",
            until(now, holder.until),
            holder.session
        )
    } else {
        let held = held_by(lease, now).map_or_else(
            || "nobody holds it yet".to_owned(),
            |held| format!("{held} holding it"),
        );
        let ahead = lease
            .waiting
            .iter()
            .position(|it| is(it, session))
            .unwrap_or(lease.waiting.len());
        format!(
            "waiting for lease {name}: {held}, {ahead} ahead of you; zirc watch says when \
                 it's yours"
        )
    }
}

/// The lease `name` as zircd says.
async fn lease_of(socket: &Path, org: Option<&str>, request: Request, name: &str) -> Result<Lease> {
    let Response::Leases { leases } = ask_in(socket, org, request).await? else {
        return Err(crate::client::unexpected());
    };
    Ok(leases
        .into_iter()
        .find(|lease| lease.name == name)
        .unwrap_or_else(|| Lease {
            name: name.to_owned(),
            holder: None,
            holders: Vec::new(),
            waiting: Vec::new(),
            held: Vec::new(),
        }))
}

/// `zirc lease`.
pub(super) async fn lease(socket: &Path, args: LeaseArgs, out: &mut impl Write) -> Result<()> {
    if let Some(LeaseCommand::Release {
        lease: name,
        session,
        org,
    }) = args.command
    {
        let org = org_here(socket, org).await?;
        let session = session_or_user(session);
        let request = Request::LeaseRelease {
            name: name.clone(),
            session,
        };
        lease_of(socket, org.as_deref(), request, &name).await?;
        writeln!(out, "gave back lease {name}")?;
        return Ok(());
    }
    let Some(name) = args.lease else {
        return leases(socket, args.json, args.org, out).await;
    };
    let session = session_or_user(args.session);
    let org = org_here(socket, args.org).await?;
    let request = Request::Lease {
        name: name.clone(),
        session: session.clone(),
        seconds: args.seconds,
        pid: Some(std::process::id()),
        wait: args.wait,
    };
    let mut lease = lease_of(socket, org.as_deref(), request, &name).await?;
    // A lease nobody else wants is the session's once it's settled: worth a moment's wait.
    let settled = tokio::time::Instant::now() + Duration::from_millis(SETTLE) + POLL;
    let mut told = false;
    loop {
        let yours = holds(&lease, &session);
        if yours || (!args.wait && tokio::time::Instant::now() >= settled) {
            break;
        }
        // Given back while waiting, as when its watch was gone or zircd restarted without
        // it: nothing will make it the session's, and `--wait` once waited for ever.
        if !asked(&lease, &session) {
            bail!("{session} no longer waits for lease {name}: zirc lease {name} to ask again");
        }
        if args.wait && !told && tokio::time::Instant::now() >= settled {
            eprintln!("{}", said(&lease, &session, now()));
            told = true;
        }
        tokio::time::sleep(if args.wait { POLL } else { POLL / 4 }).await;
        lease = lease_of(socket, org.as_deref(), Request::Leases, &name).await?;
    }
    if args.json {
        writeln!(out, "{}", serde_json::to_string(&lease)?)?;
    } else {
        writeln!(out, "{}", said(&lease, &session, now()))?;
    }
    Ok(())
}

/// `zirc leases`.
pub(super) async fn leases(
    socket: &Path,
    json: bool,
    org: OrgArg,
    out: &mut impl Write,
) -> Result<()> {
    let org = org_here(socket, org).await?;
    let Response::Leases { leases } = ask_in(socket, org.as_deref(), Request::Leases).await? else {
        return Err(crate::client::unexpected());
    };
    if json {
        writeln!(out, "{}", serde_json::to_string(&leases)?)?;
        return Ok(());
    }
    if leases.is_empty() {
        writeln!(out, "no leases held or waited for")?;
    }
    let now = now();
    for lease in &leases {
        let held = held_by(lease, now).unwrap_or_else(|| "free".to_owned());
        let waiting: Vec<String> = lease.waiting.iter().map(who).collect();
        if waiting.is_empty() {
            writeln!(out, "{}: {held}", lease.name)?;
        } else {
            writeln!(out, "{}: {held}; then {}", lease.name, waiting.join(", "))?;
        }
        for held in &lease.held {
            writeln!(
                out,
                "{}: {} waits for this machine to have room: {}",
                lease.name, held.session, held.why
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_whose_it_is_and_whos_ahead() {
        let holder = |session: &str, mine: bool| Holder {
            nick: if mine { "me" } else { "other" }.into(),
            session: session.into(),
            until: 1_200,
            mine,
        };
        let lease = Lease {
            name: "emulator".into(),
            holder: None,
            holders: vec![holder("cc-1", true)],
            waiting: vec![holder("cc-9", false), holder("cc-2", true)],
            held: Vec::new(),
        };
        assert_eq!(
            said(&lease, "cc-1", 0),
            "lease emulator is yours for 20m: zirc lease release emulator --session cc-1 when \
             you're done"
        );
        let short = Lease {
            held: vec![zirc_core::leases::Held {
                session: "cc-7".into(),
                why: "load 52 is over 40".into(),
            }],
            ..lease.clone()
        };
        assert_eq!(
            said(&short, "cc-7", 0),
            "lease emulator waits for this machine to have room (load 52 is over 40): it's \
             asked for then, and zirc watch says when it's yours"
        );
        // Two holding a build at once, and the second's told it's theirs.
        let two = Lease {
            holders: vec![holder("cc-1", true), holder("cc-3", false)],
            ..lease.clone()
        };
        assert_eq!(
            held_by(&two, 0).as_deref(),
            Some("me/cc-1 for 20m, other/cc-3 for 20m")
        );
        // From a zircd before slots, its one holder.
        let old = Lease {
            holder: Some(holder("cc-1", true)),
            holders: Vec::new(),
            ..lease.clone()
        };
        assert!(holds(&old, "cc-1") && !holds(&old, "cc-3"));
        assert!(asked(&lease, "cc-1") && asked(&lease, "cc-2") && asked(&short, "cc-7"));
        // Given back while waiting: --wait stops instead of waiting for ever.
        assert!(!asked(&lease, "cc-9") && !asked(&lease, "cc-5"));
        assert_eq!(
            said(&lease, "cc-2", 0),
            "waiting for lease emulator: me/cc-1 for 20m holding it, 1 ahead of you; zirc \
             watch says when it's yours"
        );
    }
}
