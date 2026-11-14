//! `zirc status`: zircd's connections, channels, repositories and sessions.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use zirc_core::ipc::{self, Repo, Request, Response};
use zirc_proto::PROGRAM;

use crate::client::{ask, shown};
use crate::sessions::breaker_text;
use crate::{guest, state};

pub(crate) async fn status(
    socket: &Path,
    json: bool,
    session: Option<&str>,
    out: &mut impl Write,
) -> Result<()> {
    let Response::Status(status) = ask(socket, Request::Status).await? else {
        return Err(crate::client::unexpected());
    };
    if json {
        writeln!(out, "{}", serde_json::to_string(&status)?)?;
    } else {
        if status.orgs.is_empty() {
            let state = state(status.connected, status.problem.as_ref());
            match &status.guest {
                Some(of) => writeln!(
                    out,
                    "{}",
                    guest::status_line(
                        status.connected,
                        status.problem.is_some(),
                        of,
                        status.nick.as_deref()
                    )
                )?,
                None => writeln!(out, "{state} {}", status.server)?,
            }
            if let Some(problem) = &status.problem {
                writeln!(out, "{problem}")?;
            }
            if let Some(nick) = status.nick.as_ref().filter(|_| status.guest.is_none()) {
                writeln!(out, "nick {nick}")?;
            }
            writeln!(out, "channels {}", channels(&status.channels))?;
        }
        // Signed in to several organisations: a line for each.
        for org in &status.orgs {
            let state = state(org.connected, org.problem.as_ref());
            let nick = org
                .nick
                .as_ref()
                .map(|nick| format!(" as {nick}"))
                .unwrap_or_default();
            match &org.guest {
                Some(of) => writeln!(
                    out,
                    "{}: {}",
                    org.name,
                    guest::status_line(
                        org.connected,
                        org.problem.is_some(),
                        of,
                        org.nick.as_deref()
                    )
                )?,
                None => writeln!(out, "{}: {state} {}{nick}", org.name, org.server)?,
            }
            if let Some(problem) = &org.problem {
                writeln!(out, "{}: {problem}", org.name)?;
            }
            writeln!(out, "{}: channels {}", org.name, channels(&org.channels))?;
        }
        for line in stuck_channelless(socket, &status).await {
            writeln!(out, "{line}")?;
        }
        if let Some(version) = &status.version {
            writeln!(out, "zircd {version}")?;
        }
        if let Some(busy) = status.busy_ms.and_then(slow) {
            writeln!(out, "{busy}")?;
        }
        if let Some(breaker) = status
            .breaker
            .as_ref()
            .filter(|it| it.paused_machine || !it.paused_sessions.is_empty())
        {
            write!(out, "{}", breaker_text(breaker))?;
        }
        for line in state::troubled_lines(socket).await {
            writeln!(out, "{line}")?;
        }
        if !status.watching.is_empty() {
            writeln!(out, "watching {}", watching(&status.watching))?;
        }
        if let Some(session) = session
            && let Response::Repos { repos } = ask(socket, Request::RepoList).await?
        {
            for line in own(session, &repos) {
                writeln!(out, "{line}")?;
            }
        }
    }
    Ok(())
}

/// How long a connection may be in no channels before it's said: a zircd that's just
/// connected is still joining them, and one the server refused tries again within 10 s.
pub(crate) const JOINING: Duration = Duration::from_secs(12);

/// [`channelless`]'s lines for the connections in no channels that stay so for [`JOINING`]:
/// none for one that's only joining, as just after it connected.
pub(crate) async fn stuck_channelless(socket: &Path, status: &ipc::Status) -> Vec<String> {
    let Ok(Response::Repos { repos }) = ask(socket, Request::RepoList).await else {
        return Vec::new();
    };
    let mut lines = channelless(status, &repos);
    let deadline = tokio::time::Instant::now() + JOINING;
    while !lines.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let Ok(Response::Status(now)) = ask(socket, Request::Status).await else {
            break;
        };
        lines = channelless(&now, &repos);
    }
    lines
}

/// A loud line for each organisation that's connected but in no channels while repositories
/// are watched there: posts fail and watches hear nothing until zircd gets back in.
pub(crate) fn channelless(status: &ipc::Status, repos: &[Repo]) -> Vec<String> {
    let line = |org: Option<&str>| {
        let to = org.map(|org| format!(" to {org}")).unwrap_or_default();
        format!(
            "{PROGRAM}: connected{to}, but in no channels, so posts fail and watches hear nothing; \
             zircd keeps trying (see ~/.zirc/zircd.log), and restarting it may help"
        )
    };
    if status.orgs.is_empty() {
        let stuck = status.connected && status.channels.is_empty() && !repos.is_empty();
        return stuck.then(|| line(None)).into_iter().collect();
    }
    status
        .orgs
        .iter()
        .filter(|org| org.connected && org.channels.is_empty())
        .filter(|org| {
            repos
                .iter()
                .any(|repo| repo.org.as_ref() == Some(&org.name))
        })
        .map(|org| line(Some(&org.name)))
        .collect()
}

/// How a connection is, as `zirc status` says.
pub(crate) fn state(connected: bool, problem: Option<&String>) -> &'static str {
    if connected {
        "connected to"
    } else if problem.is_some() {
        "not connecting to"
    } else {
        "connecting to"
    }
}

/// Channels as `zirc status` lists them.
pub(crate) fn channels(channels: &[String]) -> String {
    if channels.is_empty() {
        "none".to_owned()
    } else {
        channels.join(" ")
    }
}

/// The warnings `repo` carries of a session registered there that another worktree had
/// registered too ([`ipc::Shared`]): `session`'s alone, when it names one.
pub(crate) fn shared_warnings(repo: &Repo, session: Option<&str>) -> Vec<String> {
    repo.shared
        .iter()
        .filter(|shared| session.is_none_or(|session| shared.session == session))
        .map(|shared| ipc::shared_warning(shared, &shown(&repo.path), &shown(&shared.with)))
        .collect()
}

/// Where `session` is watched, a line for each checkout, and a warning when another
/// worktree registered it too lately.
pub(crate) fn own(session: &str, repos: &[Repo]) -> Vec<String> {
    let warnings: Vec<String> = repos
        .iter()
        .flat_map(|repo| shared_warnings(repo, Some(session)))
        .collect();
    let mut lines = own_places(session, repos);
    lines.extend(warnings);
    lines
}

pub(crate) fn own_places(session: &str, repos: &[Repo]) -> Vec<String> {
    let lines: Vec<String> = repos
        .iter()
        .filter_map(|repo| {
            repo.sessions.iter().find(|it| it.session == session)?;
            Some(format!("{session} works in {}", shown(&repo.path)))
        })
        .collect();
    if lines.is_empty() {
        vec![format!(
            "{session} works in no watched repository: zirc repo add --session {session}"
        )]
    } else {
        lines
    }
}

/// The watches open, as `zirc status` lists them: the sessions', then how many watch
/// without one, as a person or a script does.
pub(crate) fn watching(watches: &[Option<String>]) -> String {
    let sessions: Vec<&str> = watches.iter().filter_map(Option::as_deref).collect();
    let others = watches.len() - sessions.len();
    let sessions = (!sessions.is_empty()).then(|| format!("sessions {}", sessions.join(" ")));
    let others = match others {
        0 => None,
        1 => Some("1 watch without a session".to_owned()),
        n => Some(format!("{n} watches without a session")),
    };
    [sessions, others]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", and ")
}

/// How long zircd must have been busy with one thing, in milliseconds, for `zirc status` to
/// say so: under it, nobody waited long enough to notice.
const SLOW_MS: u64 = 1000;

/// How [`slow`]'s line starts: what the machine's load added to `zirc status`, which the
/// home page's terminals leave out (tests/terminals.rs).
pub const BUSY: &str = "zircd was busy ";

/// What `zirc status` says of a zircd that was busy `ms` with one thing lately
/// ([`zirc_core::ipc::Status::busy_ms`]), when that's long enough to have been felt.
fn slow(ms: u64) -> Option<String> {
    (ms >= SLOW_MS).then(|| {
        format!(
            "{BUSY}{}.{}s with one thing in the last minute: commands waited that long",
            ms / 1000,
            ms % 1000 / 100
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_where_a_session_works() {
        let repo = |path: &str, sessions: &[&str]| Repo {
            path: path.into(),
            channel: "#api".into(),
            sessions: sessions
                .iter()
                .map(|session| ipc::RepoSession {
                    session: (*session).into(),
                })
                .collect(),
            org: None,
            shared: Vec::new(),
        };
        let repos = [
            repo("/src/api", &["cc-1", "cc-2"]),
            repo("/src/api-wt", &["cc-1"]),
        ];
        assert_eq!(
            own("cc-1", &repos),
            ["cc-1 works in /src/api", "cc-1 works in /src/api-wt"]
        );
        assert_eq!(
            own("cc-3", &repos),
            ["cc-3 works in no watched repository: zirc repo add --session cc-3"]
        );
        // A second worktree took cc-2's name lately: warned of, for cc-2 alone.
        let mut shared = repo("/src/api-2", &["cc-2"]);
        shared.shared = vec![ipc::Shared {
            session: "cc-2".into(),
            with: "/src/api".into(),
            at: 1,
        }];
        let repos = [repos[0].clone(), shared];
        let said = own("cc-2", &repos);
        assert_eq!(said.len(), 3, "{said:?}");
        assert!(
            said[2].starts_with(
                "warning: cc-2 is registered in /src/api-2 now, and was in /src/api too"
            ),
            "{}",
            said[2]
        );
        assert_eq!(own("cc-1", &repos).len(), 1, "another session's isn't said");
    }

    #[test]
    fn labels_the_watches_open() {
        let watches = [None, Some("cc-1".to_owned()), None, Some("cc-2".to_owned())];
        assert_eq!(
            watching(&watches),
            "sessions cc-1 cc-2, and 2 watches without a session"
        );
        assert_eq!(
            watching(&watches[..2]),
            "sessions cc-1, and 1 watch without a session"
        );
        assert_eq!(watching(&watches[1..2]), "sessions cc-1");
    }

    #[test]
    fn says_loudly_when_connected_but_in_no_channels() {
        let status = |channels: &[&str]| ipc::Status {
            connected: true,
            server: "acme.irc.zirc.dev:6697".into(),
            nick: Some("m".into()),
            channels: channels.iter().map(|&it| it.into()).collect(),
            problem: None,
            watching: Vec::new(),
            protocol: ipc::PROTOCOL,
            version: None,
            orgs: Vec::new(),
            breaker: None,
            guest: None,
            busy_ms: None,
        };
        let repo = Repo {
            path: "/src/api".into(),
            channel: "#api".into(),
            sessions: Vec::new(),
            org: None,
            shared: Vec::new(),
        };
        assert!(
            channelless(&status(&["#api", "#general"]), std::slice::from_ref(&repo)).is_empty()
        );
        assert!(channelless(&status(&[]), &[]).is_empty(), "nothing watched");
        let said = channelless(&status(&[]), &[repo]);
        assert_eq!(said.len(), 1);
        assert!(
            said[0].starts_with("zirc: connected, but in no channels"),
            "{}",
            said[0]
        );
    }

    #[test]
    fn says_a_zircd_was_slow_only_when_it_was() {
        assert_eq!(slow(0), None);
        assert_eq!(slow(999), None);
        assert_eq!(
            slow(3250).as_deref(),
            Some(
                "zircd was busy 3.2s with one thing in the last minute: commands waited that long"
            )
        );
    }
}
