//! `zirc flag`, `zirc block`, `zirc unblock` and `zirc blocked`: reporting a person or a
//! message to the organisation's owners and to zirc, and the account's own blocklist, which
//! the apps share ([`zirc_proto::report`]). All through zirc.dev as this machine's account,
//! which acts as its person.

use std::io::Write;
use std::path::Path;

use anyhow::{Context as _, Result};
use clap::Args;
use zirc_proto::report::{self, Kind, Reason, Report};

use crate::search::{self, Team, one_org};

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// The account of the person to report, or whose message it is.
    who: String,
    /// Why: harassment, hate, violence, sexual, illegal, spam or other.
    #[arg(long, value_name = "REASON")]
    reason: String,
    /// A message to report instead of the person, by the id `zirc search` shows. Needs --in.
    #[arg(long, value_name = "ID", requires = "channel")]
    message: Option<String>,
    /// The channel the message is in, or for a direct message the other person's nick.
    #[arg(long = "in", value_name = "CHANNEL", requires = "message")]
    channel: Option<String>,
    /// What to add, at most 500 characters.
    #[arg(long)]
    note: Option<String>,
    /// Only this organisation, on a machine signed in to several.
    #[arg(long, value_name = "NAME", hide = true)]
    org: Option<String>,
}

#[derive(Debug, Args)]
pub struct WhoArgs {
    /// The person's account.
    who: String,
    /// Only this organisation, on a machine signed in to several.
    #[arg(long, value_name = "NAME", hide = true)]
    org: Option<String>,
}

#[derive(Debug, Args)]
pub struct BlockedArgs {
    /// Print JSON: an array on one line.
    #[arg(long)]
    json: bool,
    /// Only this organisation, on a machine signed in to several.
    #[arg(long, value_name = "NAME", hide = true)]
    org: Option<String>,
}

/// The one organisation this is about ([`crate::search::one_org`]), through zirc.dev.
async fn team(home: &Path, socket: &Path, org: Option<&str>) -> Result<Team> {
    let org = one_org(home, socket, org).await?;
    search::team(&org).with_context(|| {
        format!(
            "{} wasn't signed in through zirc.dev, which keeps its reports and blocks",
            org.name()
        )
    })
}

/// The report `args` make, or why they don't.
fn report_from(args: &ReportArgs) -> Result<Report> {
    let reason = Reason::from_id(&args.reason).with_context(|| {
        let all: Vec<&str> = Reason::ALL.iter().map(|reason| reason.id()).collect();
        format!("--reason is one of {}", all.join(", "))
    })?;
    let note = report::note(args.note.as_deref()).map_err(anyhow::Error::msg)?;
    Ok(Report {
        kind: if args.message.is_some() {
            Kind::Message
        } else {
            Kind::Person
        },
        reason,
        account: args.who.clone(),
        channel: args.channel.clone(),
        message: args.message.clone(),
        note,
    })
}

/// Runs `zirc flag`.
pub async fn report(
    home: &Path,
    socket: &Path,
    args: &ReportArgs,
    out: &mut impl Write,
) -> Result<()> {
    let report = report_from(args)?;
    let team = team(home, socket, args.org.as_deref()).await?;
    let id = team
        .client
        .report(&team.account, &team.name, &report)
        .await?;
    writeln!(
        out,
        "reported {} to {}'s owners and zirc, who act on reports within 24 hours (report {id})",
        args.who, team.name
    )?;
    Ok(())
}

/// Runs `zirc block` (`blocked` true) or `zirc unblock`.
pub async fn block(
    home: &Path,
    socket: &Path,
    args: &WhoArgs,
    blocked: bool,
    out: &mut impl Write,
) -> Result<()> {
    let team = team(home, socket, args.org.as_deref()).await?;
    team.client
        .set_blocked(&team.account, &team.name, &args.who, blocked)
        .await?;
    if blocked {
        writeln!(
            out,
            "blocked {}: their messages and notifications no longer reach you, in any of your apps",
            args.who
        )?;
    } else {
        writeln!(out, "unblocked {}", args.who)?;
    }
    Ok(())
}

/// Runs `zirc blocked`.
pub async fn blocked(
    home: &Path,
    socket: &Path,
    args: &BlockedArgs,
    out: &mut impl Write,
) -> Result<()> {
    let team = team(home, socket, args.org.as_deref()).await?;
    let list = team.client.blocks(&team.account, &team.name).await?;
    if args.json {
        writeln!(out, "{}", serde_json::to_string(&list.blocked)?)?;
        return Ok(());
    }
    if list.blocked.is_empty() {
        writeln!(out, "nobody blocked in {}", team.name)?;
    }
    for person in list.blocked {
        match person.name {
            Some(name) => writeln!(out, "{} ({name})", person.account)?,
            None => writeln!(out, "{}", person.account)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use crate::{Cli, run};
    use zirc_core::paths;

    /// Runs `zirc <args>` as a machine of ada's signed in through the web app `web`.
    async fn zirc(web: &str, args: &[&str]) -> Result<String, String> {
        let home = tempfile::tempdir().unwrap();
        let config = format!(
            "web = \"{web}\"\nowner = \"ada\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n[account]\nname = \"ada-mbp\"\npassword = \"pw\"\n"
        );
        std::fs::write(paths::config(home.path()), config).unwrap();
        let cli = Cli::try_parse_from([&["zirc"], args].concat()).unwrap();
        let cli = Cli {
            home: Some(home.path().to_owned()),
            ..cli
        };
        let mut out = Vec::new();
        run(cli, &mut out)
            .await
            .map(|()| String::from_utf8(out).unwrap())
            .map_err(|err| err.to_string())
    }

    #[tokio::test]
    async fn reports_a_message_and_a_person() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (201, r#"{"id":"r1"}"#.into()),
            (201, r#"{"id":"r2"}"#.into()),
        ]);
        let said = zirc(
            &web,
            &[
                "flag",
                "mal",
                "--reason",
                "spam",
                "--message",
                "m1",
                "--in",
                "#web",
            ],
        )
        .await
        .unwrap();
        assert!(
            said.contains("reported mal") && said.contains("24 hours") && said.contains("r1"),
            "{said}"
        );
        zirc(
            &web,
            &["flag", "mal", "--reason", "hate", "--note", " rude "],
        )
        .await
        .unwrap();
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("POST /api/organisations/"),
            "{}",
            asked[0]
        );
        assert!(asked[0].contains("/reports "), "{}", asked[0]);
        assert!(asked[0].ends_with(
            r##"{"kind":"message","reason":"spam","account":"mal","channel":"#web","message":"m1"}"##
        ));
        assert!(
            asked[1]
                .ends_with(r#"{"kind":"person","reason":"hate","account":"mal","note":"rude"}"#)
        );
    }

    #[tokio::test]
    async fn refuses_an_unknown_reason_before_asking() {
        let err = zirc("http://127.0.0.1:1", &["flag", "mal", "--reason", "boring"])
            .await
            .unwrap_err();
        assert!(err.contains("--reason is one of harassment"), "{err}");
    }

    #[tokio::test]
    async fn blocks_unblocks_and_lists() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (204, String::new()),
            (
                200,
                r#"{"blocked":[{"account":"mal","name":"Mal"},{"account":"eve"}]}"#.into(),
            ),
            (204, String::new()),
        ]);
        assert!(
            zirc(&web, &["block", "mal"])
                .await
                .unwrap()
                .contains("blocked mal")
        );
        assert_eq!(zirc(&web, &["blocked"]).await.unwrap(), "mal (Mal)\neve\n");
        assert_eq!(
            zirc(&web, &["unblock", "mal"]).await.unwrap(),
            "unblocked mal\n"
        );
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("PUT /api/organisations/"),
            "{}",
            asked[0]
        );
        assert!(asked[0].contains("/blocks/mal "), "{}", asked[0]);
        assert!(
            asked[2].starts_with("DELETE /api/organisations/"),
            "{}",
            asked[2]
        );
    }
}
