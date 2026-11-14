//! `zirc ci`: an organisation's CI tokens, made, listed and revoked from an owner's or admin's
//! machine. CI can't sign in, as there's nobody at a browser: it sends a token instead, with
//! what it has to say, to zirc.dev ([`zirc_proto::login::CI_PATH`]), which posts it in the
//! organisation's channels as `zirc-ci`. Text that names a session wakes that session's
//! `zirc watch`, so CI says which session a build is for.

use std::io::Write;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use clap::{Args, Subcommand};
use zirc_proto::login::{CI_PATH, CI_TOKEN_NAME};

/// What zirc.dev says of an account it no longer takes.
const SIGNED_OUT: &str = "zirc.dev doesn't take this machine's account any more: run `zirc login`";

#[derive(Debug, Subcommand)]
pub enum CiCommand {
    /// Make a token for your CI to say how builds went, and print it once.
    ///
    /// For an organisation's owners' and admins' machines. Keep it as a secret in your CI,
    /// which sends it to zirc.dev with what it has to say: zirc ci token prints the command.
    Token(TokenArgs),
    /// List the organisation's CI tokens, with who made each and when it last posted.
    Tokens(OrgArgs),
    /// Revoke a CI token by its name: CI holding it is refused from then on.
    Revoke(RevokeArgs),
}

#[derive(Debug, Args)]
pub struct OrgArgs {
    /// The organisation, on a machine signed in to several.
    #[arg(long, value_name = "NAME")]
    org: Option<String>,
}

#[derive(Debug, Args)]
pub struct TokenArgs {
    /// What to call it among the organisation's tokens, such as github: lowercase letters,
    /// digits and dashes.
    #[arg(long, default_value = CI_TOKEN_NAME)]
    name: String,
    /// Print only the token, for a script to keep.
    #[arg(long)]
    quiet: bool,
    #[command(flatten)]
    org: OrgArgs,
}

#[derive(Debug, Args)]
pub struct RevokeArgs {
    /// The token's name, as zirc ci tokens lists it.
    #[arg(default_value = CI_TOKEN_NAME)]
    name: String,
    #[command(flatten)]
    org: OrgArgs,
}

/// The organisation's account at zirc.dev, for a command that manages its CI tokens.
async fn team(home: &Path, socket: &Path, org: &OrgArgs) -> Result<crate::search::Team> {
    let org = crate::search::one_org(home, socket, org.org.as_deref()).await?;
    let Some(team) = crate::search::team(&org) else {
        bail!(
            "{} wasn't signed in through zirc.dev, which keeps its CI tokens: run `zirc login`",
            org.name()
        );
    };
    Ok(team)
}

/// Runs `zirc ci`.
pub async fn ci(
    home: &Path,
    socket: &Path,
    command: CiCommand,
    out: &mut (impl Write + Send),
) -> Result<()> {
    match command {
        CiCommand::Token(args) => {
            let team = team(home, socket, &args.org).await?;
            let made = team
                .client
                .ci_token(&team.account, &args.name)
                .await?
                .context(SIGNED_OUT)?;
            if args.quiet {
                writeln!(out, "{}", made.token)?;
                return Ok(());
            }
            writeln!(out, "{}", made.token)?;
            writeln!(out)?;
            writeln!(
                out,
                "That is {}'s CI token {}, shown this once. Keep it as a secret in your CI, as ZIRC_TOKEN.",
                made.organisation, made.name
            )?;
            writeln!(
                out,
                "Then have a build say how it went, naming the session it's for:"
            )?;
            writeln!(out)?;
            writeln!(
                out,
                "  curl -fsS {}{CI_PATH} -H \"Authorization: Bearer $ZIRC_TOKEN\" \\",
                team.client.web()
            )?;
            writeln!(
                out,
                "    -d '{{\"channel\":\"#<repository>\",\"session\":\"<session>\",\"state\":\"failed\",\"text\":\"build 7 failed\"}}'"
            )?;
            writeln!(out)?;
            writeln!(
                out,
                "state is pending, running, passed, failed, cancelled or warning. zirc ci revoke {} ends it.",
                made.name
            )?;
        }
        CiCommand::Tokens(org) => {
            let team = team(home, socket, &org).await?;
            let listed = team
                .client
                .ci_tokens(&team.account)
                .await?
                .context(SIGNED_OUT)?;
            if listed.tokens.is_empty() {
                writeln!(
                    out,
                    "{} has no CI tokens: zirc ci token makes one",
                    team.name
                )?;
            }
            for token in listed.tokens {
                let by = token.by.map(|by| format!(" by {by}")).unwrap_or_default();
                let used = token.used.map_or_else(
                    || "never used".to_owned(),
                    |at| format!("last used {}", day(&at)),
                );
                writeln!(out, "{}  made {}{by}, {used}", token.name, day(&token.made))?;
            }
        }
        CiCommand::Revoke(args) => {
            let team = team(home, socket, &args.org).await?;
            team.client
                .revoke_ci_token(&team.account, &args.name)
                .await?
                .context(SIGNED_OUT)?;
            writeln!(out, "revoked {}'s CI token {}", team.name, args.name)?;
        }
    }
    Ok(())
}

/// A time's day, for people: `2026-11-12`.
fn day(time: &str) -> &str {
    time.get(..10).unwrap_or(time)
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use zirc_core::paths;

    use super::*;
    use crate::{Cli, run};

    /// Runs `zirc ci <args>` as a machine set up through the web app `web`.
    async fn zirc_ci(web: &str, args: &[&str]) -> Result<String, String> {
        let home = tempfile::tempdir().unwrap();
        let config = format!(
            "web = \"{web}\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n[account]\nname = \"ada-mbp\"\npassword = \"pw\"\n"
        );
        std::fs::write(paths::config(home.path()), config).unwrap();
        let cli = Cli::try_parse_from([&["zirc", "ci"], args].concat()).unwrap();
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
    async fn makes_a_token_with_the_machines_account_and_says_how_ci_uses_it() {
        let made = r#"{"name":"github","token":"zirc_ci_abc","organisation":"acme"}"#;
        let (web, served) = zirc_test_ergo::web::serve(vec![(200, made.to_owned())]);
        let out = zirc_ci(&web, &["token", "--name", "github"]).await.unwrap();
        assert!(out.starts_with("zirc_ci_abc\n"), "{out}");
        assert!(out.contains("acme's CI token github"), "{out}");
        assert!(
            out.contains(&format!(
                "curl -fsS {web}/ci -H \"Authorization: Bearer $ZIRC_TOKEN\""
            )),
            "{out}"
        );
        let requests = served.join().unwrap();
        let request = requests[0].to_lowercase();
        assert!(request.starts_with("post /api/cli/ci-token "), "{request}");
        assert!(request.contains("authorization: basic"), "{request}");
        assert!(request.contains(r#"{"name":"github"}"#), "{request}");
    }

    #[tokio::test]
    async fn prints_only_the_token_when_asked_quietly() {
        let made = r#"{"name":"ci","token":"zirc_ci_abc","organisation":"acme"}"#;
        let (web, _served) = zirc_test_ergo::web::serve(vec![(200, made.to_owned())]);
        assert_eq!(
            zirc_ci(&web, &["token", "--quiet"]).await.unwrap(),
            "zirc_ci_abc\n"
        );
    }

    #[tokio::test]
    async fn lists_tokens_and_revokes_one_by_name() {
        let listed = r#"{"tokens":[{"name":"ci","by":"ada","made":"2026-11-12T09:00:00.000Z","used":"2026-11-12T10:30:00.000Z"},{"name":"github","made":"2026-11-12T09:05:00.000Z"}]}"#;
        let (web, served) =
            zirc_test_ergo::web::serve(vec![(200, listed.to_owned()), (200, "{}".to_owned())]);
        assert_eq!(
            zirc_ci(&web, &["tokens"]).await.unwrap(),
            "ci  made 2026-11-12 by ada, last used 2026-11-12\ngithub  made 2026-11-12, never used\n"
        );
        assert_eq!(
            zirc_ci(&web, &["revoke", "github"]).await.unwrap(),
            "revoked acme's CI token github\n"
        );
        let requests = served.join().unwrap();
        assert!(
            requests[1]
                .to_lowercase()
                .starts_with("post /api/cli/ci-token/revoke ")
        );
    }

    #[tokio::test]
    async fn says_what_the_web_app_refused() {
        let (web, _served) = zirc_test_ergo::web::serve(vec![
            (
                403,
                r#"{"error":"Only the organisation's owners and admins manage its CI tokens."}"#
                    .to_owned(),
            ),
            (
                401,
                r#"{"error":"Not a machine's credentials: run zirc login."}"#.to_owned(),
            ),
        ]);
        let refused = zirc_ci(&web, &["token"]).await.unwrap_err();
        assert!(refused.contains("owners and admins"), "{refused}");
        let signed_out = zirc_ci(&web, &["tokens"]).await.unwrap_err();
        assert!(signed_out.contains("run `zirc login`"), "{signed_out}");
    }
}
