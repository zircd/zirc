//! How each machine's agent sessions stand now, for a reader that isn't in the organisation's
//! channels: zirc.dev's dashboard, through the tenant's sidecar. A machine says which of its
//! sessions are online and which away ([`crate::session::Live`]) in the channels, live only;
//! it tells the tenant's recorder the same ([`crate::activity::RECORDER`]), again every
//! [`TELL_EVERY_SECS`], and the sidecar answers what it last heard at [`PATH`]: each session
//! online or away, in words, which for one away with messages waiting are the ones every
//! app says ([`crate::session::not_listening`]). Nothing a session said.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::session::Live;

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
    /// Whether a `zirc watch` of its is open: a message reaches it at once.
    pub online: bool,
    /// How many messages from people wait for it, when it's away.
    pub waiting: u32,
    /// How it stands in words: [`ONLINE`], [`AWAY`], or, away with messages waiting, what
    /// every app says ([`crate::session::not_listening`]): `Not listening for 17 min · 1
    /// message waiting`.
    pub says: String,
}

/// What a session with a watch open is said to be.
pub const ONLINE: &str = "Online";

/// What a session with no watch open, and nothing waiting for it, is said to be.
pub const AWAY: &str = "Away";

/// What [`PATH`] answers: every session the machines connected now said online or away.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "SessionsStanding")]
pub struct Standing {
    /// Each session, by machine, then by session.
    pub sessions: Vec<Session>,
}

/// How the sessions stand at `now` (seconds since the epoch), by what each machine last said
/// (`machines`, by account). A session in neither of a machine's lists has been away longer
/// than an app keeps it, and isn't here.
#[must_use]
pub fn standing(machines: &BTreeMap<String, Live>, now: u64) -> Standing {
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
                        online: true,
                        waiting: 0,
                        says: ONLINE.to_owned(),
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
                    online: false,
                    waiting: away.waiting,
                    says,
                },
            );
        }
        sessions.extend(of_machine.into_values());
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

    #[test]
    fn a_session_is_online_or_away_with_what_waits_in_the_apps_words() {
        let live = Live {
            sessions: vec!["cc-a".into(), "cc-b".into()],
            away: [
                (
                    "cc-b".to_owned(),
                    Away {
                        since: 1_000,
                        waiting: 1,
                    },
                ),
                (
                    "cc-c".to_owned(),
                    Away {
                        since: 400,
                        waiting: 0,
                    },
                ),
            ]
            .into(),
            ..Live::default()
        };
        let machines = [("ada-mbp".to_owned(), live)].into();

        let said = standing(&machines, 1_000 + 17 * 60);

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
            ]
        );
    }
}
