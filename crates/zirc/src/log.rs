//! `zirc log`: what's been said on the team's server, from its log on zirc.dev, for an
//! organisation's owner's machine. The log has the messages people send only while the owners
//! have debug logging on, and keeps 30 days.

use std::io::Write;
use std::path::Path;
use zirc_proto::PROGRAM;

use anyhow::{Context as _, Result, bail};
use clap::Args;
use zirc_core::login::LogQuery;
use zirc_proto::login::LogLine;

/// The most lines it reads, a page at a time, before it stops and says so.
const MOST: usize = 10_000;

#[derive(Debug, Args)]
pub struct LogArgs {
    /// Text the lines must have, such as a nick, a channel or a word.
    text: Vec<String>,
    /// How far back: minutes, hours, days, weeks or months, such as 30m, 12h, 3d, 2w or 1mo.
    /// The log keeps 30 days.
    #[arg(long, default_value = "12h")]
    since: String,
    /// Every line of the server's log: connections, joins and the server's own, not only the
    /// messages people send.
    #[arg(long)]
    all: bool,
    /// One JSON object a line, oldest first, for scripts and agents: its time, its text and,
    /// for a message, who sent it where.
    #[arg(long)]
    json: bool,
    /// The organisation, on a machine signed in to several.
    #[arg(long, value_name = "NAME", hide = true)]
    org: Option<String>,
}

/// `zirc log`: one organisation's ([`crate::search::one_org`]).
pub async fn log(
    home: &Path,
    socket: &Path,
    args: LogArgs,
    out: &mut (impl Write + Send),
) -> Result<()> {
    let org = crate::search::one_org(home, socket, args.org.as_deref()).await?;
    let Some(crate::search::Team {
        account, client, ..
    }) = crate::search::team(&org)
    else {
        bail!(
            "{} wasn't signed in through zirc.dev, which keeps its log: run `zirc login`",
            org.name()
        );
    };
    let mut query = LogQuery {
        since: args.since,
        text: (!args.text.is_empty()).then(|| args.text.join(" ")),
        before: None,
        all: args.all,
    };
    // Newest page first; each page oldest first.
    let mut pages: Vec<Vec<LogLine>> = Vec::new();
    let mut read = 0;
    loop {
        let page = client
            .log(&account, &query)
            .await?
            .context("zirc.dev doesn't take this machine's account any more: run `zirc login`")?;
        if pages.is_empty() && page.capped {
            eprintln!("{PROGRAM}: the log keeps 30 days: from {}", page.from);
        }
        read += page.lines.len();
        pages.push(page.lines);
        match page.older {
            Some(older) if read < MOST => query.before = Some(older),
            Some(_) => {
                eprintln!(
                    "{PROGRAM}: stopped at the newest {MOST} lines: ask for less with --since or text"
                );
                break;
            }
            None => break,
        }
    }
    if read == 0 && !args.all {
        eprintln!(
            "{PROGRAM}: no messages logged since {}: the log has them only while the organisation's owners have debug logging on, on its Log tab at zirc.dev",
            query.since
        );
    }
    for line in pages.into_iter().rev().flatten() {
        if args.json {
            writeln!(out, "{}", serde_json::to_string(&line)?)?;
        } else {
            writeln!(out, "{}  {}", when(&line.time), line.text)?;
        }
    }
    Ok(())
}

/// A line's time for people: `2026-11-01 05:30:11`, in UTC as the log keeps it.
fn when(time: &str) -> String {
    time.get(..19).unwrap_or(time).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use zirc_core::paths;

    use super::*;
    use crate::{Cli, run};

    /// Runs `zirc log <args>` as a machine set up through the web app `web`.
    async fn zirc_log(web: &str, args: &[&str]) -> Result<String, String> {
        let home = tempfile::tempdir().unwrap();
        let config = format!(
            "web = \"{web}\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n[account]\nname = \"ada-mbp\"\npassword = \"pw\"\n"
        );
        std::fs::write(paths::config(home.path()), config).unwrap();
        let cli = Cli::try_parse_from([&["zirc", "log"], args].concat()).unwrap();
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

    fn page(lines: &[(&str, &str)], older: Option<&str>) -> String {
        let lines: Vec<_> = lines
            .iter()
            .map(|(time, text)| serde_json::json!({"time": time, "text": text}))
            .collect();
        serde_json::json!({"lines": lines, "from": "2026-10-31T17:00:00.000Z", "capped": false, "older": older})
            .to_string()
    }

    #[tokio::test]
    async fn reads_every_page_and_prints_the_oldest_first() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (
                200,
                page(
                    &[
                        ("2026-11-01T04:00:00.000Z", "bob → #web: third"),
                        ("2026-11-01T05:00:00.000Z", "ada → #web: fourth"),
                    ],
                    Some("2026-11-01T04:00:00.000Z"),
                ),
            ),
            (
                200,
                page(
                    &[
                        ("2026-11-01T02:00:00.000Z", "ada → #web: first"),
                        ("2026-11-01T03:00:00.000Z", "ada → bob: second"),
                    ],
                    None,
                ),
            ),
        ]);
        let out = zirc_log(&web, &["login", "bug", "--since", "1d"])
            .await
            .unwrap();
        assert_eq!(
            out,
            "2026-11-01 02:00:00  ada → #web: first\n\
             2026-11-01 03:00:00  ada → bob: second\n\
             2026-11-01 04:00:00  bob → #web: third\n\
             2026-11-01 05:00:00  ada → #web: fourth\n"
        );
        let requests = served.join().unwrap();
        let first = |i: usize| requests[i].lines().next().unwrap().to_owned();
        assert_eq!(first(0), "GET /api/cli/log?since=1d&q=login+bug HTTP/1.1");
        assert_eq!(
            first(1),
            "GET /api/cli/log?since=1d&q=login+bug&before=2026-11-01T04%3A00%3A00.000Z HTTP/1.1"
        );
        assert!(requests[0].to_lowercase().contains("authorization: basic"));
    }

    #[tokio::test]
    async fn prints_json_lines_for_agents_over_the_last_12_hours() {
        let line = r##"{"time":"2026-11-01T05:00:00.000Z","text":"ada → #web: hi","message":{"nick":"ada","to":"#web","kind":"message","text":"hi"}}"##;
        let answer = format!(r#"{{"lines":[{line}],"from":"x","capped":false}}"#);
        let (web, served) = zirc_test_ergo::web::serve(vec![(200, answer)]);
        let out = zirc_log(&web, &["--json", "--all"]).await.unwrap();
        assert_eq!(out, format!("{line}\n"));
        let requests = served.join().unwrap();
        assert!(requests[0].starts_with("GET /api/cli/log?since=12h&all=1 "));
    }

    #[tokio::test]
    async fn says_what_the_web_app_refused() {
        let (web, _served) = zirc_test_ergo::web::serve(vec![(
            403,
            r#"{"error":"Only the organisation's owners can read its log."}"#.into(),
        )]);
        let err = zirc_log(&web, &[]).await.unwrap_err();
        assert!(
            err.contains("Only the organisation's owners can read its log."),
            "{err}"
        );
        let (web, _served) = zirc_test_ergo::web::serve(vec![(401, "{}".into())]);
        let err = zirc_log(&web, &[]).await.unwrap_err();
        assert!(err.contains("run `zirc login`"), "{err}");
    }
}
