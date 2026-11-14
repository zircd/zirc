//! What every session is doing, as `zirc sessions` and the apps' sessions panels show it,
//! from what zircd already hears: what each session said it's taking on and is about to
//! touch, the working sets machines send (all as `zirc who` has them), and in each channel's
//! history, its posts and the landing queue's events. Nothing here sends anything.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zirc_proto::Event;

use crate::chat::ago;
use crate::ipc::{self, ChannelPresence, Delivery, Request, Response};

/// The kind of event the landing queue says how a landing goes with, and `ops:land` says it
/// queued one with.
pub use zirc_proto::ci::LANDING_KIND;

/// How much of each channel's history is read.
pub const HISTORY: usize = 200;

/// How long a session that's said nothing it's taking on or about to touch stays listed
/// after it was last heard from: a day.
pub const QUIET: u64 = 24 * 60 * 60;

/// Words in a post or an intent that say a session is waiting on something.
const WAITING: [&str; 6] = [
    "waiting for",
    "waiting on",
    "wait for",
    "blocked on",
    "blocked by",
    "needs your",
];

/// Where a session's latest landing is, as the landing queue last said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Landing {
    /// Queued, or being tested, in a build.
    Queued {
        /// The branch.
        branch: String,
        /// The build it's in.
        build: Option<u64>,
    },
    /// On the default branch.
    Landed {
        /// The branch.
        branch: String,
        /// The commit it landed as, short.
        commit: String,
        /// The build that landed it.
        build: Option<u64>,
    },
    /// Its build failed, or it couldn't land.
    Failed {
        /// The branch, as the event's data or its text says: older lines name none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// The build.
        build: Option<u64>,
        /// What the queue said went wrong.
        why: String,
    },
    /// It conflicts with the default branch.
    Conflicts {
        /// The branch.
        branch: String,
    },
}

impl Landing {
    /// It in a few words, as a row shows it.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Queued {
                branch,
                build: Some(n),
            } => format!("landing {branch} in build {n}"),
            Self::Queued {
                branch,
                build: None,
            } => format!("landing {branch}"),
            Self::Landed {
                branch,
                commit,
                build: Some(n),
            } => format!("landed {branch} as {commit} (build {n})"),
            Self::Landed {
                branch,
                commit,
                build: None,
            } => format!("landed {branch} as {commit}"),
            Self::Failed { branch, build, why } => {
                let of = branch
                    .as_ref()
                    .map(|b| format!(" of {b}"))
                    .unwrap_or_default();
                match build {
                    Some(n) => format!("build {n}{of} failed: {why}"),
                    None => format!(
                        "{}failed: {why}",
                        branch.as_ref().map(|b| format!("{b} ")).unwrap_or_default()
                    ),
                }
            }
            Self::Conflicts { branch } => format!("{branch} conflicts with the default branch"),
        }
    }
}

/// One session, as the sessions view lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Its name, as it posts with `--session`.
    pub name: String,
    /// The nick of the machine it runs on, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The channel it was last heard in.
    pub channel: String,
    /// What it said it's taking on, until it's done or dropped it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// What it said it's about to touch, until that's done or expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// The branch it works on, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Its latest landing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<Landing>,
    /// What it's waiting on, as its landing or its latest post or intent says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    /// When it was last heard from, in seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard: Option<u64>,
}

impl Session {
    /// A session called `name`, last heard in `channel`, known nothing else of yet.
    fn new(name: &str, channel: &str) -> Self {
        Self {
            name: name.to_owned(),
            machine: None,
            channel: channel.to_owned(),
            task: None,
            intent: None,
            branch: None,
            landing: None,
            waiting: None,
            heard: None,
        }
    }

    /// It as a few lines of text, at `now`: who and where on the first, then what it's
    /// doing, as `zirc sessions` prints it and the terminal client's panel shows it.
    #[must_use]
    pub fn lines(&self, now: u64) -> Vec<String> {
        let mut head = self.name.clone();
        if let Some(machine) = &self.machine {
            head = format!("{head} on {machine}");
        }
        if let Some(branch) = &self.branch {
            head = format!("{head} · {branch}");
        }
        if let Some(heard) = self.heard {
            head = format!("{head} · {}", ago(now.saturating_sub(heard)));
        }
        let mut lines = vec![head];
        let landing = self.landing.as_ref().map(Landing::describe);
        for (label, text) in [
            ("taking", &self.task),
            ("about to", &self.intent),
            ("landing", &landing),
            ("waiting on", &self.waiting),
        ] {
            if let Some(text) = text {
                lines.push(format!("  {label}: {text}"));
            }
        }
        lines
    }
}

/// What a landing event says, and which session it's for when its text names one first,
/// as the landing queue's do: `cc-1531f8: landed login as 1a2b3c4 (build 12)`.
#[must_use]
pub fn landing_event(text: &str, data: &serde_json::Value) -> (Option<String>, Option<Landing>) {
    // Its first line: the details CI puts below it are for people.
    let text = text.lines().next().unwrap_or_default();
    let (session, said) = match zirc_proto::addressed(text) {
        Some((session, said)) => (Some(session.to_owned()), said),
        None => (None, text),
    };
    let build = data
        .get("build")
        .and_then(|build| build.as_u64().or_else(|| build.as_str()?.parse().ok()))
        .or_else(|| {
            let (_, after) = said.split_once("build ")?;
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        });
    let word = |n: usize| said.split_whitespace().nth(n).map(str::to_owned);
    let landing = if said.starts_with("landed ") {
        word(1)
            .zip(word(3))
            .map(|(branch, commit)| Landing::Landed {
                branch,
                commit,
                build,
            })
    } else if said.starts_with("queued ") {
        word(1).map(|branch| Landing::Queued { branch, build })
    } else if let Some((branch, _)) = said.split_once(" conflicts with ") {
        Some(Landing::Conflicts {
            branch: branch.to_owned(),
        })
    } else if said.starts_with("build ") && (said.contains(" failed") || said.contains("couldn't"))
    {
        // What went wrong, without the build's address at the end.
        let why = said.split_once(": ").map_or(said, |(_, why)| why);
        let why = why.rsplit_once(", http").map_or(why, |(why, _)| why);
        // "build N of <branch> failed in …", or "build N couldn't land <branch>: …".
        let named = (word(2).as_deref() == Some("of"))
            .then(|| word(3))
            .flatten()
            .or_else(|| {
                let (_, after) = said.split_once("couldn't land ")?;
                after.split(':').next().map(str::to_owned)
            });
        let branch = data
            .get("branch")
            .and_then(|branch| branch.as_str())
            .map(str::to_owned)
            .or(named);
        Some(Landing::Failed {
            branch,
            build,
            why: why.to_owned(),
        })
    } else {
        None
    };
    (session, landing)
}

/// What `text` says the session is waiting on, if it says it's waiting.
fn waiting_in(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    WAITING
        .iter()
        .any(|word| lower.contains(word))
        .then(|| text.to_owned())
}

/// The sessions `channels` say are at work, as `zirc who` has them: what they're taking on,
/// about to touch, and changing where.
fn at_work(channels: &[ChannelPresence]) -> BTreeMap<String, Session> {
    let mut sessions: BTreeMap<String, Session> = BTreeMap::new();
    for channel in channels {
        let here = &channel.channel;
        for claim in &channel.claims {
            let session = on(&mut sessions, &claim.session, &claim.nick, here);
            session.task = Some(claim.task.clone());
            session.heard = session.heard.max(Some(claim.since));
        }
        for intent in &channel.intents {
            on(&mut sessions, &intent.session, &intent.nick, here).intent =
                Some(intent.summary.clone());
        }
        for member in &channel.members {
            if let Some(name) = &member.session {
                let session = on(&mut sessions, name, &member.nick, here);
                if member.branch.is_some() {
                    session.branch.clone_from(&member.branch);
                }
            }
        }
    }
    sessions
}

/// The session called `name` among `sessions`, added if it's new, as on the machine `nick`
/// and heard of in `channel`.
fn on<'a>(
    sessions: &'a mut BTreeMap<String, Session>,
    name: &str,
    nick: &str,
    channel: &str,
) -> &'a mut Session {
    let session = sessions
        .entry(name.to_owned())
        .or_insert_with(|| Session::new(name, channel));
    session.machine = Some(nick.to_owned());
    session
}

/// Every session heard of in `channels` (as `zirc who` has them) and `heard` (their history,
/// oldest first), most lately heard from first: those that have said what they're taking on
/// or about to touch, or have a working set, and any other heard from in the last
/// [`QUIET`] before `now`.
#[must_use]
pub fn view(channels: &[ChannelPresence], heard: &[Delivery], now: u64) -> Vec<Session> {
    let mut sessions = at_work(channels);
    let working: Vec<String> = sessions.keys().cloned().collect();
    // What each session last posted, to say what it's waiting on.
    let mut posted: BTreeMap<String, String> = BTreeMap::new();
    for delivery in heard {
        let at = delivery.time.as_deref().and_then(crate::time::seconds_at);
        let own = delivery.message.session.as_str();
        // The queue's events name the session first; one a session sent is its own.
        let (named, landing) = match &delivery.message.event {
            Event::Custom { kind, data, text } if kind == LANDING_KIND => landing_event(text, data),
            _ => (None, None),
        };
        let itself = named.is_none();
        let name = named.unwrap_or_else(|| own.to_owned());
        if name.is_empty() {
            continue;
        }
        let session = sessions
            .entry(name.clone())
            .or_insert_with(|| Session::new(&name, &delivery.channel));
        // Only what the session itself said is it being heard from.
        if itself {
            if at > session.heard {
                session.heard = at;
                session.channel.clone_from(&delivery.channel);
            }
            session.machine.get_or_insert_with(|| delivery.from.clone());
        }
        if let Some(landing) = landing {
            if let Landing::Queued { branch, .. } | Landing::Landed { branch, .. } = &landing {
                session.branch.get_or_insert_with(|| branch.clone());
            }
            session.landing = Some(landing);
        } else if let Event::Text { body } = &delivery.message.event {
            posted.insert(name, body.clone());
        }
    }
    let mut listed: Vec<Session> = sessions
        .into_values()
        .filter(|session| {
            working.contains(&session.name)
                || session
                    .heard
                    .is_some_and(|heard| now.saturating_sub(heard) < QUIET)
        })
        .map(|mut session| {
            session.waiting = match &session.landing {
                Some(Landing::Queued {
                    build: Some(build), ..
                }) => Some(format!("its landing, build {build}")),
                _ => posted
                    .get(&session.name)
                    .and_then(|post| waiting_in(post))
                    .or_else(|| session.intent.as_deref().and_then(waiting_in)),
            };
            session
        })
        .collect();
    listed.sort_by(|a, b| b.heard.cmp(&a.heard).then_with(|| a.name.cmp(&b.name)));
    listed
}

/// The sessions view, at `now`, from the zircd listening on `socket`, for the organisation
/// `org` (see [`ipc::request_in`]): what `zirc who` has, and the latest [`HISTORY`] of each
/// channel it lists.
///
/// # Errors
///
/// When zircd can't be reached, or answers with an error or something else.
pub async fn fetch(socket: &Path, org: Option<&str>, now: u64) -> Result<Vec<Session>, ipc::Error> {
    let Response::Who { channels } = ipc::request_in(socket, org, &Request::Who).await? else {
        return Err(ipc::Error::Daemon(
            "zircd answered something else".to_owned(),
        ));
    };
    let mut heard = Vec::new();
    for channel in &channels {
        let read = Request::Read {
            target: channel.channel.clone(),
            limit: HISTORY,
            since: None,
            until: None,
        };
        if let Response::Messages { messages } = ipc::request_in(socket, org, &read).await? {
            heard.extend(messages);
        }
    }
    Ok(view(&channels, &heard, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::{Claim, Intent, Member};
    use zirc_proto::Message;

    const NOW: u64 = 1_793_618_707;

    fn delivery(from: &str, session: &str, minutes_ago: u64, event: Event) -> Delivery {
        // From JSON, as zircd sends it, so fields added since are left at their defaults.
        serde_json::from_value(serde_json::json!({
            "channel": "#zirc",
            "from": from,
            "message": Message::new(session, event),
            "time": crate::time::tag(NOW - minutes_ago * 60),
        }))
        .unwrap()
    }

    fn said(session: &str, minutes_ago: u64, body: &str) -> Delivery {
        let event = Event::Text { body: body.into() };
        delivery("mattias-matt", session, minutes_ago, event)
    }

    fn landing(minutes_ago: u64, text: &str, build: &str) -> Delivery {
        let event = Event::Custom {
            kind: LANDING_KIND.into(),
            data: serde_json::json!({ "build": build }),
            text: text.into(),
        };
        delivery("zirc-alerts", "ci", minutes_ago, event)
    }

    #[test]
    fn reads_what_the_landing_queue_says() {
        let none = serde_json::Value::Null;
        let read = |text: &str| landing_event(text, &none);
        assert_eq!(
            read("cc-1: landed login as 1a2b3c4 (build 12)"),
            (
                Some("cc-1".into()),
                Some(Landing::Landed {
                    branch: "login".into(),
                    commit: "1a2b3c4".into(),
                    build: Some(12),
                })
            )
        );
        assert_eq!(
            read("cc-1: build 13 failed in :bazel: land: //a:b, https://buildkite.com/x#1").1,
            Some(Landing::Failed {
                branch: None,
                build: Some(13),
                why: "land: //a:b".into(),
            })
        );
        // The details CI puts below its line are left out of what went wrong.
        assert_eq!(
            read(
                "cc-1: build 13 failed https://b.test/13: //a:b\n- failed: `//a:b`\n- run them: `bazel test //a:b`"
            ),
            (
                Some("cc-1".into()),
                Some(Landing::Failed {
                    branch: None,
                    build: Some(13),
                    why: "//a:b".into(),
                })
            )
        );
        assert_eq!(
            read("cc-1: build 15 of login failed in :bazel: land: //a:b, https://x#1").1,
            Some(Landing::Failed {
                branch: Some("login".into()),
                build: Some(15),
                why: "land: //a:b".into(),
            })
        );
        assert_eq!(
            landing_event(
                "cc-1: build 16 couldn't land login: main kept moving, land again",
                &serde_json::json!({"build": "16", "branch": "login"}),
            )
            .1,
            Some(Landing::Failed {
                branch: Some("login".into()),
                build: Some(16),
                why: "main kept moving, land again".into(),
            })
        );
        assert_eq!(
            read("cc-1: login conflicts with main, resolve, land again").1,
            Some(Landing::Conflicts {
                branch: "login".into()
            })
        );
        assert_eq!(
            landing_event("queued login (build 14)", &serde_json::json!({"build": 14})),
            (
                None,
                Some(Landing::Queued {
                    branch: "login".into(),
                    build: Some(14),
                })
            )
        );
        assert_eq!(read("cc-1: something else").1, None);
    }

    #[test]
    fn lists_each_session_with_what_it_is_doing() {
        let channels = [ChannelPresence {
            channel: "#zirc".into(),
            org: None,
            members: vec![Member {
                nick: "mattias-matt".into(),
                worktree: None,
                name: None,
                branch: Some("login".into()),
                session: Some("cc-1".into()),
                session_name: None,
                session_tool: None,
                session_avatar: None,
                paths: vec!["src/lib.rs".into()],
            }],
            claims: vec![Claim {
                nick: "mattias-matt".into(),
                session: "cc-1".into(),
                session_name: None,
                session_tool: None,
                session_avatar: None,
                task: "the login bug".into(),
                since: NOW - 3600,
            }],
            intents: vec![Intent {
                nick: "mattias-matt".into(),
                session: "cc-2".into(),
                session_name: None,
                session_tool: None,
                session_avatar: None,
                summary: "docs for /org".into(),
                paths: vec!["README.md".into()],
                expires: Some(NOW + 600),
            }],
            landings: Vec::new(),
        }];
        let queued = Event::Custom {
            kind: LANDING_KIND.into(),
            data: serde_json::json!({ "build": 10 }),
            text: "queued login (build 10)".into(),
        };
        let heard = [
            said("cc-3", 60 * 30, "long ago"),
            said("cc-1", 20, "taking: the login bug"),
            landing(
                15,
                "cc-1: build 9 failed in :bazel: land: //a:b, https://x",
                "9",
            ),
            delivery("mattias-matt", "cc-1", 12, queued),
            said("cc-2", 5, "waiting for the desktop's replies to land"),
        ];
        let sessions = view(&channels, &heard, NOW);
        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        // Most lately heard first; cc-3, quiet for a day and doing nothing, left out.
        assert_eq!(names, ["cc-2", "cc-1"]);
        assert_eq!(
            sessions[1].lines(NOW),
            [
                "cc-1 on mattias-matt · login · 12m ago",
                "  taking: the login bug",
                "  landing: landing login in build 10",
                "  waiting on: its landing, build 10",
            ]
        );
        assert_eq!(
            sessions[0].lines(NOW),
            [
                "cc-2 on mattias-matt · 5m ago",
                "  about to: docs for /org",
                "  waiting on: waiting for the desktop's replies to land",
            ]
        );

        // Once it lands, it's waiting on nothing.
        let mut heard = heard.to_vec();
        heard.push(landing(1, "cc-1: landed login as 1a2b3c4 (build 10)", "10"));
        let sessions = view(&channels, &heard, NOW);
        assert_eq!(
            sessions[1].lines(NOW)[2..],
            ["  landing: landed login as 1a2b3c4 (build 10)"]
        );
        assert_eq!(sessions[1].waiting, None);
    }

    #[test]
    fn says_how_long_ago() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(125), "2m ago");
        assert_eq!(ago(3 * 3600 + 5), "3h ago");
        assert_eq!(ago(2 * 86_400), "2d ago");
    }
}
