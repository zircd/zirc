//! The next step of getting started, as a command says it when it ends: one bold `Next:` line,
//! which zirc.dev decides from how the machine's person stands
//! ([`zirc_proto::onboarding::Progress`]), so the terminal, the dashboard and the apps never
//! disagree; and, for an agent's first watch, the hint for that step (`next.<step>`).
//!
//! Best effort, and never in a command's way: with zirc.dev out of reach, or a step this zirc
//! doesn't know, nothing is said.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use zirc_proto::onboarding::{Outcome, PromptSeen, Step, Surface};

use crate::search::Team;

/// How long a command waits for zirc.dev to say what's next.
const WITHIN: Duration = Duration::from_secs(4);

/// The next step for the person whose machine this is, in the organisation `org` names or
/// this repository's, and the account that asked: `None` when there's none, or nobody says.
pub(crate) async fn step(
    home: &Path,
    socket: &Path,
    org: Option<&str>,
) -> Option<(&'static Step, Team)> {
    let asked = async {
        let org = crate::search::one_org(home, socket, org).await.ok()?;
        let team = crate::search::team(&org)?;
        let progress = team.client.steps(&team.account).await.ok()??;
        // By its number in this zirc's own list: its words are declared there.
        let step = zirc_proto::onboarding::step(progress.next?)?;
        Some((step, team))
    };
    tokio::time::timeout(WITHIN, asked).await.ok()?
}

/// `Next: <what to do>`, bold where a terminal reads it.
pub(crate) fn line(step: &Step) -> String {
    let line = format!("{} {}", zirc_proto::onboarding::NEXT_PREFIX, step.next_line);
    if crate::paint::wanted() {
        format!("\x1b[1m{line}\x1b[0m")
    } else {
        line
    }
}

/// Ends a command with the person's next step, after a blank line, and tells zirc.dev the
/// prompt was shown in a terminal.
pub(crate) async fn say(
    home: &Path,
    socket: &Path,
    org: Option<&str>,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let Some((step, team)) = step(home, socket, org).await else {
        return Ok(());
    };
    writeln!(out)?;
    writeln!(out, "{}", line(step))?;
    let seen = PromptSeen {
        prompt: step.id.to_owned(),
        surface: Surface::Terminal,
        outcome: Outcome::Shown,
    };
    let _ = tokio::time::timeout(WITHIN, team.client.prompt(&team.account, &seen)).await;
    Ok(())
}

/// `zirc repo`, which for a person adding a repository, not an agent's session, ends on their
/// next step.
pub(crate) async fn repo(
    home: &Path,
    socket: &Path,
    command: crate::repo::RepoCommand,
    out: &mut (impl Write + Send),
) -> anyhow::Result<()> {
    let person_adds = matches!(
        &command,
        crate::repo::RepoCommand::Add { session: None, .. }
    );
    crate::setup::repo_first(home, socket, command, out).await?;
    if person_adds {
        say(home, socket, None, out).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use zirc_core::paths;

    use super::*;

    fn signed_in(web: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let config = format!(
            "web = \"{web}\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n[account]\nname = \"ada-mbp\"\npassword = \"pw\"\n"
        );
        std::fs::write(paths::config(home.path()), config).unwrap();
        home
    }

    #[tokio::test]
    async fn says_the_step_zirc_dev_says_is_next_and_that_it_was_shown() {
        let progress = serde_json::to_string(&zirc_proto::onboarding::progress(&[1, 2])).unwrap();
        let (web, served) = zirc_test_ergo::web::serve(vec![(200, progress), (204, String::new())]);
        let home = signed_in(&web);
        let mut out = Vec::new();
        say(home.path(), &home.path().join("no-socket"), None, &mut out)
            .await
            .unwrap();
        let out = String::from_utf8(out).unwrap();
        let agent = zirc_proto::onboarding::step(3).unwrap();
        assert_eq!(out, format!("\nNext: {}\n", agent.next_line));
        let requests = served.join().unwrap();
        assert!(
            requests[0].to_lowercase().starts_with("get /api/me/steps "),
            "{}",
            requests[0]
        );
        assert!(
            requests[1]
                .to_lowercase()
                .starts_with("post /api/me/prompts "),
            "{}",
            requests[1]
        );
        assert!(
            requests[1].contains(r#"{"prompt":"agent","surface":"terminal","outcome":"shown"}"#),
            "{}",
            requests[1]
        );
    }

    #[tokio::test]
    async fn says_nothing_when_every_step_is_reached_or_nobody_answers() {
        let done =
            serde_json::to_string(&zirc_proto::onboarding::progress(&[1, 2, 3, 4, 5, 6, 7, 8]))
                .unwrap();
        let (web, _served) = zirc_test_ergo::web::serve(vec![(200, done)]);
        let home = signed_in(&web);
        let mut out = Vec::new();
        say(home.path(), &home.path().join("no-socket"), None, &mut out)
            .await
            .unwrap();
        assert!(out.is_empty());
        let refused = signed_in("http://127.0.0.1:9");
        say(
            refused.path(),
            &refused.path().join("no-socket"),
            None,
            &mut out,
        )
        .await
        .unwrap();
        assert!(out.is_empty());
    }
}
