//! How each machine's agent sessions stand now, for a reader that isn't in the organisation's
//! channels: zirc.dev's dashboard, through the tenant's sidecar. A machine says which of its
//! sessions are online and which away ([`crate::session::Live`]) in the channels, live only;
//! it tells the tenant's recorder the same ([`crate::activity::RECORDER`]), again every
//! [`TELL_EVERY_SECS`], and the sidecar answers what it last heard at [`PATH`]: each session
//! online or away, in words, which for one away with messages waiting are the ones every
//! app says ([`crate::session::not_listening`]), with what its card says it's working on and
//! its tool, which the machine tells the recorder too. Nothing a session said in a message.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::session::{Live, SessionCard};

/// The sidecar's endpoint for how the sessions stand now: `GET`, a [`Standing`].
pub const PATH: &str = "/v1/sessions";

/// How often a machine tells the recorder its live list again though nothing changed, in
/// seconds: a recorder that started since hears it within this.
pub const TELL_EVERY_SECS: u64 = 5 * 60;

/// How long what a machine last told the recorder is taken as how it stands, in seconds:
/// two tellings and a little. After that its sessions aren't said to be anything.
pub const STALE_SECS: u64 = 2 * TELL_EVERY_SECS + 60;

/// One agent session as its machine last said it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "SessionStanding")]
pub struct Session {
    /// Its machine's account.
    pub machine: String,
    /// The session's id.
    pub session: String,
    /// Its short name, as its card says: `tls-rotation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// What it's working on, as its card says: the title its agent tool shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// The agent tool it runs in, as people read it ([`crate::tool_name`]): `Claude Code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool: Option<String>,
    /// Whether a `zirc watch` of its is open: a message reaches it at once.
    pub online: bool,
    /// How many messages from people wait for it, when it's away.
    pub waiting: u32,
    /// Whether a request of its waits for its person's answer ([`crate::request::waiting`]).
    pub asking: bool,
    /// How it stands in words: [`ONLINE`], [`AWAY`], or, away with messages waiting, what
    /// every app says ([`crate::session::not_listening`]): `Not listening for 17 min · 1
    /// message waiting`.
    pub says: String,
    /// It in one line, where a line names it ([`line`]): `tls-rotation · Claude Code ·
    /// listening`.
    pub line: String,
}

/// What a session with a watch open is said to be.
pub const ONLINE: &str = "Online";

/// What a session online is said to be while a request of its waits for its person's answer
/// ([`crate::request::waiting`]), in place of [`ONLINE`].
pub const WAITING_FOR_ANSWER: &str = "Waiting for your answer";

/// What a session with no watch open, and nothing waiting for it, is said to be.
pub const AWAY: &str = "Away";

/// What a person's agent is said to be doing while a watch of its is open, where a line names
/// it rather than a column: `tls-rotation · Claude Code · listening`.
pub const LISTENING: &str = "listening";

/// A session in a line, where a line names it: what it's called ([`crate::session::agent_title`]),
/// its tool and, online, [`LISTENING`],
/// or how it stands away: `tls-rotation · Claude Code · listening`.
#[must_use]
pub fn line(session: &Session) -> String {
    let standing = if session.online {
        LISTENING
    } else {
        session.says.as_str()
    };
    // Called as every app calls an agent (session::agent_title): its name, else its task.
    let called = crate::session::agent_title(
        session.name.as_deref(),
        session.title.as_deref(),
        &session.session,
    );
    [
        Some(called.as_str()),
        session.tool.as_deref(),
        Some(standing),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// What [`PATH`] answers: every session the machines connected now said online or away.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "SessionsStanding")]
pub struct Standing {
    /// Each session, by machine, then by session.
    pub sessions: Vec<Session>,
}

/// How the sessions stand at `now` (seconds since the epoch), by what each machine last said
/// (`machines`, by account), with what each card says (`cards`) and those with a request
/// waiting for their person (`asking`, by machine and session). A session in neither of a machine's lists has been away longer
/// than an app keeps it, and isn't here.
#[must_use]
pub fn standing(
    machines: &BTreeMap<String, Live>,
    cards: &BTreeMap<(String, String), SessionCard>,
    asking: &BTreeSet<(String, String)>,
    now: u64,
) -> Standing {
    let asks =
        |machine: &str, session: &str| asking.contains(&(machine.to_owned(), session.to_owned()));
    let named = |machine: &str, session: &str| {
        let card = cards.get(&(machine.to_owned(), session.to_owned()));
        (
            card.map(|card| card.title.clone()),
            card.and_then(|card| card.tool.as_deref().map(crate::tool_name)),
            card.and_then(|card| card.name.clone()),
        )
    };
    let mut sessions = Vec::new();
    for (machine, live) in machines {
        let mut of_machine: BTreeMap<&str, Session> = BTreeMap::new();
        for session in &live.sessions {
            if live.online(session) {
                of_machine.insert(
                    session,
                    Session {
                        machine: machine.clone(),
                        session: session.clone(),
                        name: named(machine, session).2,
                        title: named(machine, session).0,
                        tool: named(machine, session).1,
                        online: true,
                        waiting: 0,
                        asking: asks(machine, session),
                        says: if asks(machine, session) {
                            WAITING_FOR_ANSWER
                        } else {
                            ONLINE
                        }
                        .to_owned(),
                        line: String::new(),
                    },
                );
            }
        }
        for (session, away) in &live.away {
            let says = if away.waiting > 0 {
                crate::session::not_listening(now.saturating_sub(away.since), away.waiting)
            } else {
                AWAY.to_owned()
            };
            of_machine.insert(
                session,
                Session {
                    machine: machine.clone(),
                    session: session.clone(),
                    name: named(machine, session).2,
                    title: named(machine, session).0,
                    tool: named(machine, session).1,
                    online: false,
                    waiting: away.waiting,
                    asking: asks(machine, session),
                    says,
                    line: String::new(),
                },
            );
        }
        sessions.extend(of_machine.into_values().map(|mut session| {
            session.line = line(&session);
            session
        }));
    }
    Standing { sessions }
}

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Session::decl(config),
        Standing::decl(config),
        format!("const SESSIONS_STANDING_PATH = {PATH:?};"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Away;

    /// Ada's machine: cc-a online with a card, cc-b away with a message waiting though still
    /// within the grace, cc-c away with nothing waiting; said 17 minutes after cc-b went.
    fn adas() -> Standing {
        let away = |since, waiting| Away { since, waiting };
        let live = Live {
            sessions: vec!["cc-a".into(), "cc-b".into(), "cc-d".into()],
            away: [
                ("cc-b".to_owned(), away(1_000, 1)),
                ("cc-c".to_owned(), away(400, 0)),
            ]
            .into(),
        };
        let card = SessionCard {
            session: "cc-a".into(),
            name: Some("tls-rotation".into()),
            title: "Rotate the server's TLS certificates".into(),
            tool: Some(crate::CLAUDE_CODE.into()),
        };
        let machines = [("ada-mbp".to_owned(), live)].into();
        let cards = [(("ada-mbp".to_owned(), "cc-a".to_owned()), card)].into();
        let asking = [("ada-mbp".to_owned(), "cc-d".to_owned())].into();
        standing(&machines, &cards, &asking, 1_000 + 17 * 60)
    }

    #[test]
    fn a_session_is_online_or_away_with_what_waits_in_the_apps_words() {
        let said = adas();

        let lines: Vec<String> = said
            .sessions
            .iter()
            .map(|s| format!("{} {} {}", s.session, s.online, s.says))
            .collect();
        assert_eq!(
            lines,
            [
                "cc-a true Online",
                // Within the grace a session is still in `sessions`, and away all the same.
                "cc-b false Not listening for 17 min · 1 message waiting",
                "cc-c false Away",
                // A request of its waits for Ada's answer.
                "cc-d true Waiting for your answer",
            ]
        );
    }

    #[test]
    fn a_session_carries_its_cards_words_and_its_line() {
        let said = adas();

        let online = &said.sessions[0];
        assert_eq!(
            online.title.as_deref(),
            Some("Rotate the server's TLS certificates")
        );
        assert_eq!(online.tool.as_deref(), Some("Claude Code"));
        assert_eq!(online.name.as_deref(), Some("tls-rotation"));
        assert_eq!(online.line, "tls-rotation · Claude Code · listening");
        // No card heard: called by its id.
        assert_eq!(said.sessions[1].title, None);
        assert_eq!(
            said.sessions[1].line,
            "cc-b · Not listening for 17 min · 1 message waiting"
        );
    }
}
