//! Tools reaching a role at its address ([`zirc_proto::inbound`]): `POST /in/<role>/<secret>`
//! on the server's own host, so nothing a tool posts passes zirc.dev. The address is a job's
//! ([`zirc_proto::coworkers::Given::addresses`]), `<role>` being the job's name, as a role was
//! its own job: whoever holds the job takes what's posted, and a tool isn't set up again when
//! the job moves. "Role" below is that job.
//!
//! The secret opens the role's address when its digest is the one zirc.dev gave with the staff
//! ([`zirc_proto::roles::Address::opens`], compared in constant time); a rotated one stops at
//! once, as the next staff replaces it. A post to a role that isn't one, or with a secret
//! that doesn't open it, is answered 404 alike, so the answer says nothing of which. Each role
//! takes [`A_MINUTE`] posts a minute (429 beyond), each at most [`MAX_BODY`] bytes (413).
//!
//! What the known tools post is read into reports ([`senders`]), which the role's nick says in
//! `#ops`, where its items take them ([`crate::items`]): one open item per upstream issue, so
//! the same Sentry issue firing 40 times wakes the holder once. A tool saying it's resolved
//! closes the item. An unknown tool's post wakes the holder as it was sent, or waits for the
//! next, as a direct message does. When each role last heard each tool is kept beside the
//! history ([`Store`]), each known tool heard org-wide ([`Tool::org_wide`]), and read by zirc.dev ([`zirc_proto::inbound::HEARD_PATH`]).
//!
//! Only counts and outcomes are logged: which tool, and what came of it; never a word it
//! sent, nor which role.

pub mod senders;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use tracing::info;
use zirc_proto::inbound::{A_MINUTE, Heard, HeardState, HeardTool, MAX_BODY, Tool};
use zirc_proto::role_report::{Closing, End, item_id};
use zirc_proto::woke::Woke;

use crate::roles::Roles;
use senders::{Read, Refused, Signing};

/// When each role last heard each tool, kept beside the history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// By role, then tool.
    #[serde(default)]
    pub roles: BTreeMap<String, BTreeMap<Tool, HeardTool>>,
}

/// The file beside the history it's kept in.
pub const KEPT: &str = "zirc_inbound.json";

/// What the endpoint keeps: when each tool was heard, and how many posts each role took this
/// minute.
#[derive(Clone)]
pub struct Store {
    kept: Arc<Mutex<Kept>>,
    path: PathBuf,
    minutes: Arc<Mutex<HashMap<String, (u64, u32)>>>,
}

impl Store {
    /// The store beside the history at `history`.
    #[must_use]
    pub fn beside(history: &Path) -> Self {
        let path = history.with_file_name(KEPT);
        let kept = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            kept: Arc::new(Mutex::new(kept)),
            path,
            minutes: Arc::default(),
        }
    }

    /// Whether `role` may take another post at `now` (unix seconds): [`A_MINUTE`] a minute.
    fn admits(&self, role: &str, now: u64) -> bool {
        let mut minutes = self.minutes.lock().unwrap_or_else(PoisonError::into_inner);
        let minute = now / 60;
        minutes.retain(|_, (at, _)| *at == minute);
        let count = minutes.entry(role.to_owned()).or_insert((minute, 0));
        count.1 = count.1.saturating_add(1);
        count.1 <= A_MINUTE
    }

    /// Notes that `role` heard `tool` at `now`, a test or not, and keeps it.
    fn heard(&self, role: &str, tool: Tool, now: u64, test: bool) {
        let kept = {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            kept.roles
                .entry(role.to_owned())
                .or_default()
                .entry(tool)
                .or_insert_with(|| HeardTool::new(tool))
                .heard(now, test);
            kept.clone()
        };
        if let Err(error) = crate::save_json(&self.path, &kept) {
            tracing::warn!(%error, "couldn't keep when tools were heard");
        }
    }

    /// Forgets when `role` heard `tool`, and keeps that: whether it had.
    pub fn forget(&self, role: &str, tool: Tool) -> bool {
        let kept = {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(tools) = kept.roles.get_mut(role) else {
                return false;
            };
            if tools.remove(&tool).is_none() {
                return false;
            }
            if tools.is_empty() {
                kept.roles.remove(role);
            }
            kept.clone()
        };
        if let Err(error) = crate::save_json(&self.path, &kept) {
            tracing::warn!(%error, "couldn't keep when tools were heard");
        }
        true
    }

    /// What `role` heard, as zirc.dev reads it now ([`HeardTool::as_of`]): each known tool as
    /// the organisation heard it, at any role's address ([`Tool::org_wide`]), with this role's
    /// own times and words where it heard it too; any tool as this role heard it.
    #[must_use]
    pub fn of(&self, role: &str) -> Heard {
        let now = crate::now();
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let own = kept.roles.get(role);
        let tools = Tool::ALL
            .iter()
            .filter_map(|&tool| {
                let mine = own
                    .and_then(|tools| tools.get(&tool))
                    .map(|heard| heard.as_of(role, now));
                if !tool.org_wide() {
                    return mine;
                }
                let last = kept
                    .roles
                    .values()
                    .filter_map(|tools| tools.get(&tool))
                    .filter_map(|heard| heard.last.max(heard.last_test))
                    .max()?;
                let mut shown = mine.unwrap_or_else(|| HeardTool::new(tool));
                shown.heard = Some(HeardState::of(Some(last), now));
                Some(shown)
            })
            .collect();
        Heard {
            role: role.to_owned(),
            tools,
        }
    }
}

/// What the endpoint answers with.
#[derive(Clone)]
pub struct Inbound {
    /// The organisation's roles, whose addresses open and whose nicks say what came.
    pub roles: Roles,
    /// When each tool was heard.
    pub store: Store,
    /// Where a report that's never said in `#ops` goes straight to its role's items (a new
    /// issue, [`zirc_proto::role_report::Subject::in_ops`]).
    pub items: tokio::sync::mpsc::UnboundedSender<zirc_proto::role_report::RoleReport>,
}

/// The endpoint's route, `POST /in/{role}/{secret}`, its bodies held to [`MAX_BODY`].
pub fn router(inbound: Inbound) -> Router {
    Router::new()
        .route(
            zirc_proto::roles::INBOUND_PATH,
            post(posted).layer(DefaultBodyLimit::max(MAX_BODY)),
        )
        .with_state(inbound)
}

/// What came of a post, as it's logged and answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A report said for the role's items.
    Reported,
    /// An item closed, as its tool resolved it.
    Resolved,
    /// A test, heard.
    Test,
    /// Nothing to wake anyone for.
    Ignored,
    /// An unknown tool's post, to the role as it was sent.
    Message,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Resolved => "resolved",
            Self::Test => "test",
            Self::Ignored => "ignored",
            Self::Message => "message",
        }
    }
}

/// A plain answer, `status` with nothing more: an error says nothing of why to whoever asks.
fn answer(status: StatusCode) -> Response {
    status.into_response()
}

async fn posted(
    State(inbound): State<Inbound>,
    UrlPath((role, secret)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let now = crate::now();
    match take(&inbound, &role, &secret, &headers, &body, now) {
        Ok((tool, outcome)) => {
            info!(
                tool = tool.as_str(),
                outcome = outcome.as_str(),
                "a tool posted to a role"
            );
            answer(StatusCode::ACCEPTED)
        }
        Err(status) => {
            info!(
                status = status.as_u16(),
                "a post to a role's address was refused"
            );
            answer(status)
        }
    }
}

/// Takes `body`, posted to `role`'s address with `secret` at `now`, and says which tool sent
/// it and what came of it.
///
/// # Errors
///
/// The status it's refused with.
pub fn take(
    inbound: &Inbound,
    role: &str,
    secret: &str,
    headers: &HeaderMap,
    body: &[u8],
    now: u64,
) -> Result<(Tool, Outcome), StatusCode> {
    let Some(tools) = inbound.roles.opened(role, secret) else {
        return Err(StatusCode::NOT_FOUND);
    };
    if !inbound.store.admits(role, now) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    // The tools' own signing secrets, as the role's admins gave them; GitHub's is the
    // address's.
    let signing = Signing {
        address: secret,
        tools: &tools,
    };
    let (tool, read) =
        senders::read(role, &header, body, signing).map_err(|refused| match refused {
            Refused::Signature => StatusCode::UNAUTHORIZED,
            Refused::Unreadable => StatusCode::UNPROCESSABLE_ENTITY,
        })?;
    let outcome = match read {
        Read::Report(report) => {
            // Said in `#ops` by a coworker's nick; with no coworker to say it, or for a kind
            // never said there, straight to the job's items.
            if !(report.kind.in_ops() && inbound.roles.report(&report)) {
                let _ = inbound.items.send(report);
            }
            Outcome::Reported
        }
        Read::Resolved { kind, key } => {
            inbound.roles.close(&Closing {
                role: role.to_owned(),
                id: item_id(role, kind, &key),
                end: End::Resolved,
            });
            Outcome::Resolved
        }
        Read::Test => Outcome::Test,
        Read::Ignored => Outcome::Ignored,
        Read::Message(text) => {
            // It names no repository: any of the job's holders takes it.
            inbound
                .roles
                .wake_job(role, None, Woke::webhook(&text), &text);
            Outcome::Message
        }
    };
    inbound
        .store
        .heard(role, tool, now, outcome == Outcome::Test);
    Ok((tool, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use zirc_proto::coworkers::{Given, Staff};
    use zirc_proto::inbound::HEARD_FOR;
    use zirc_proto::roles::Address;

    const SECRET: &str = "k7Qp2vXe9LmR0aBcDeFgHiJk";

    fn inbound(
        dir: &Path,
    ) -> (
        Inbound,
        tokio::sync::mpsc::UnboundedReceiver<Vec<crate::roles::Out>>,
    ) {
        let history = dir.join("history.db");
        let (roles, said) = Roles::beside(&history);
        let staff: Staff = serde_json::from_value(serde_json::json!({
            "coworkers": [{"name": "oncall", "jobs": ["oncall"], "repository": "server"}],
            "jobs": [{"name": "oncall", "brief": "Owns errors and incidents in production."}],
        }))
        .unwrap();
        roles
            .set_staff(Given {
                staff,
                addresses: vec![Address {
                    role: "oncall".into(),
                    digest: Address::digest_of(SECRET),
                    signing: BTreeMap::new(),
                }],
                version: "1".into(),
            })
            .unwrap();
        (
            Inbound {
                roles,
                store: Store::beside(&history),
                items: tokio::sync::mpsc::unbounded_channel().0,
            },
            said,
        )
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn a_wrong_secret_or_role_is_not_found_alike_and_says_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, _said) = inbound(dir.path());
        let none = HeaderMap::new();
        for (role, secret) in [
            ("oncall", "k7Qp2vXe9LmR0aBcDeFgHiJ0"),
            ("triage", SECRET),
            ("oncall", ""),
        ] {
            assert_eq!(
                take(&inbound, role, secret, &none, b"hello", 0),
                Err(StatusCode::NOT_FOUND)
            );
        }
        assert!(inbound.store.of("oncall").tools.is_empty());
    }

    #[test]
    fn a_rotated_secret_stops_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, _said) = inbound(dir.path());
        let none = HeaderMap::new();
        assert!(take(&inbound, "oncall", SECRET, &none, b"hello", 0).is_ok());
        let staff = inbound.roles.standing().staff;
        let addresses = vec![Address {
            role: "oncall".into(),
            digest: Address::digest_of("aNewSecretAfterRotation0"),
            signing: BTreeMap::new(),
        }];
        inbound
            .roles
            .set_staff(Given {
                staff,
                addresses,
                version: "2".into(),
            })
            .unwrap();
        assert_eq!(
            take(&inbound, "oncall", SECRET, &none, b"hello", 1),
            Err(StatusCode::NOT_FOUND)
        );
        assert!(
            take(
                &inbound,
                "oncall",
                "aNewSecretAfterRotation0",
                &none,
                b"hello",
                1
            )
            .is_ok()
        );
    }

    #[test]
    fn each_role_takes_so_many_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, _said) = inbound(dir.path());
        let none = HeaderMap::new();
        for _ in 0..A_MINUTE {
            assert!(take(&inbound, "oncall", SECRET, &none, b"x", 60).is_ok());
        }
        assert_eq!(
            take(&inbound, "oncall", SECRET, &none, b"x", 61),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        assert!(take(&inbound, "oncall", SECRET, &none, b"x", 120).is_ok());
    }

    #[test]
    fn what_each_tool_posts_comes_to_its_outcome_and_is_heard() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, _said) = inbound(dir.path());
        let sentry = headers(&[("sentry-hook-resource", "issue")]);
        let body = include_bytes!("inbound/testdata/sentry_issue_created.json");
        assert_eq!(
            take(&inbound, "oncall", SECRET, &sentry, body, 100),
            Ok((Tool::Sentry, Outcome::Reported))
        );
        let ping = headers(&[("x-github-event", "ping")]);
        assert_eq!(
            take(
                &inbound,
                "oncall",
                SECRET,
                &ping,
                include_bytes!("inbound/testdata/github_ping.json"),
                101
            ),
            Ok((Tool::Github, Outcome::Test))
        );
        assert_eq!(
            take(
                &inbound,
                "oncall",
                SECRET,
                &HeaderMap::new(),
                b"backup done",
                102
            ),
            Ok((Tool::Other, Outcome::Message))
        );
        let heard = inbound.store.of("oncall");
        let tools: Vec<(Tool, Option<u64>, Option<u64>)> = heard
            .tools
            .iter()
            .map(|t| (t.tool, t.last, t.last_test))
            .collect();
        assert_eq!(
            tools,
            vec![
                (Tool::Sentry, Some(100), None),
                (Tool::Github, None, Some(101)),
                (Tool::Other, Some(102), None),
            ]
        );
        // Kept across a restart.
        let again = Store::beside(&dir.path().join("history.db"));
        assert_eq!(again.of("oncall"), heard);
    }

    #[test]
    fn a_known_tool_is_heard_on_every_role_once_one_heard_it_and_any_tool_only_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::beside(&dir.path().join("history.db"));
        let now = crate::now();
        store.heard("oncall", Tool::Github, now - 60, false);
        store.heard("oncall", Tool::Other, now - 60, false);
        store.heard("oncall", Tool::Sentry, now - HEARD_FOR - 60, false);
        store.heard("fixer", Tool::Sentry, now - 30, true);
        // A role that heard nothing at its own address, made after.
        let shown = |role: &str| -> Vec<(Tool, Option<HeardState>, bool)> {
            store
                .of(role)
                .tools
                .iter()
                .map(|t| (t.tool, t.heard, t.said.is_some()))
                .collect()
        };
        assert_eq!(
            shown("triage"),
            vec![
                (Tool::Sentry, Some(HeardState::Heard), false),
                (Tool::Github, Some(HeardState::Heard), false),
            ]
        );
        // Its own words stay its own: oncall's last Sentry post was days ago, fixer's a test.
        assert_eq!(
            shown("oncall"),
            vec![
                (Tool::Sentry, Some(HeardState::Heard), true),
                (Tool::Github, Some(HeardState::Heard), true),
                (Tool::Other, Some(HeardState::Heard), true),
            ]
        );
        let oncall = store.of("oncall");
        let said = oncall.tools[0].said.as_ref().unwrap();
        assert!(said.lead.starts_with("Last heard from Sentry"), "{said:?}");
        let triage = store.of("triage");
        assert_eq!((triage.tools[1].last, triage.tools[1].today), (None, 0));

        // Forgotten, as an operator drops a test post's, and kept so.
        assert!(store.forget("oncall", Tool::Github));
        assert!(!store.forget("oncall", Tool::Github));
        assert!(!store.forget("triage", Tool::Github));
        let again = Store::beside(&dir.path().join("history.db"));
        assert!(
            again
                .of("triage")
                .tools
                .iter()
                .all(|t| t.tool != Tool::Github)
        );
    }

    #[test]
    fn a_tools_report_is_said_in_ops_and_its_card_in_the_agents_room() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, mut said) = inbound(dir.path());
        let sentry = headers(&[("sentry-hook-resource", "issue")]);
        let body = include_bytes!("inbound/testdata/sentry_issue_created.json");
        take(&inbound, "oncall", SECRET, &sentry, body, 0).unwrap();
        let outs = said.try_recv().unwrap();
        assert_eq!(outs[0].to, "#ops");
        // An error, said so by the sender (the user's table).
        assert_eq!(outs[0].message.level, Some(zirc_proto::Level::Error));
        let report = zirc_proto::role_report::RoleReport::of(&outs[0].message.event).unwrap();
        assert_eq!(report.tool, Some(Tool::Sentry));
        // Its items open one and wake the role, which nobody holds: its card says it waits.
        let wake = crate::items::Wake {
            role: "oncall".into(),
            item: zirc_proto::role_report::OpenItem::opened(report, 0),
            reminder: false,
        };
        inbound.roles.reported(&wake);
        let outs = said.try_recv().unwrap();
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].to, zirc_proto::AGENTS_CHANNEL);
        assert_eq!(outs[0].message.repo.as_deref(), Some("server"));
        assert_eq!(
            outs[0].message.event,
            zirc_proto::inbound::card_event(&wake.item, false)
        );
        assert_eq!(inbound.roles.standing().seats[0].waiting, 1);
        // A reminder wakes again but says no card.
        inbound.roles.reported(&crate::items::Wake {
            reminder: true,
            ..wake
        });
        assert!(said.try_recv().is_err());
    }

    #[test]
    fn a_new_issue_goes_straight_to_its_roles_items_never_to_ops() {
        let dir = tempfile::tempdir().unwrap();
        let (mut inbound, mut said) = inbound(dir.path());
        let (items, mut direct) = tokio::sync::mpsc::unbounded_channel();
        inbound.items = items;
        let github = headers(&[("x-github-event", "issues")]);
        let body = include_bytes!("inbound/testdata/github_issues_opened.json");
        assert_eq!(
            take(&inbound, "oncall", SECRET, &github, body, 0),
            Ok((Tool::Github, Outcome::Reported))
        );
        assert!(said.try_recv().is_err(), "nothing said in #ops");
        let report = direct.try_recv().unwrap();
        assert_eq!(report.kind, zirc_proto::role_report::Subject::Issue);
    }

    #[test]
    fn a_tools_resolving_closes_its_item() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, mut said) = inbound(dir.path());
        let sentry = headers(&[("sentry-hook-resource", "issue")]);
        let body = include_bytes!("inbound/testdata/sentry_issue_resolved.json");
        assert_eq!(
            take(&inbound, "oncall", SECRET, &sentry, body, 0),
            Ok((Tool::Sentry, Outcome::Resolved))
        );
        let outs = said.try_recv().unwrap();
        let closing = Closing::of(&outs[0].message.event).unwrap();
        assert_eq!(closing.end, End::Resolved);
        assert_eq!(
            closing.id,
            item_id(
                "oncall",
                zirc_proto::role_report::Subject::Error,
                "sentry:4821007"
            )
        );
    }

    #[test]
    fn a_tool_whose_signing_secret_was_given_must_sign_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, _said) = inbound(dir.path());
        let staff = inbound.roles.standing().staff;
        // What zirc.dev gives, which the standing leaves out.
        let addresses = vec![Address {
            role: "oncall".into(),
            digest: Address::digest_of(SECRET),
            signing: BTreeMap::from([(Tool::Sentry, "sentry-client-secret".to_owned())]),
        }];
        inbound
            .roles
            .set_staff(Given {
                staff,
                addresses,
                version: "2".into(),
            })
            .unwrap();
        let body = include_bytes!("inbound/testdata/sentry_issue_created.json");
        let unsigned = headers(&[("sentry-hook-resource", "issue")]);
        assert_eq!(
            take(&inbound, "oncall", SECRET, &unsigned, body, 0),
            Err(StatusCode::UNAUTHORIZED)
        );
        let signature = zirc_proto::sign_hex("sentry-client-secret", body);
        let mut signed = unsigned.clone();
        signed.insert(
            "sentry-hook-signature",
            HeaderValue::from_str(&signature).unwrap(),
        );
        assert_eq!(
            take(&inbound, "oncall", SECRET, &signed, body, 0),
            Ok((Tool::Sentry, Outcome::Reported))
        );
    }

    #[test]
    fn a_bad_signature_is_refused_and_nothing_is_heard() {
        let dir = tempfile::tempdir().unwrap();
        let (inbound, _said) = inbound(dir.path());
        let signed = headers(&[
            ("x-github-event", "issues"),
            ("x-hub-signature-256", "sha256=00"),
        ]);
        assert_eq!(
            take(
                &inbound,
                "oncall",
                SECRET,
                &signed,
                include_bytes!("inbound/testdata/github_issues_opened.json"),
                0
            ),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert!(inbound.store.of("oncall").tools.is_empty());
    }
}
