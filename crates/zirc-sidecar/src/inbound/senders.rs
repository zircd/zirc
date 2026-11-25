//! What each known tool posts to a role's address, read into what reaches the role
//! ([`Read`]): Sentry's issue, error and event alerts (its integration's webhooks, and the
//! older `WebHooks` plugin's), GitHub's `issues` events, Linear's Issue webhooks and
//! `PagerDuty`'s v3 incident webhooks. Anything else is a message, as it was sent.
//!
//! A tool that signs its posts is checked where the server has the secret it signs with
//! ([`Signing`]): GitHub's with the address's own secret, which its webhook is set up with,
//! and Sentry's, Linear's and `PagerDuty`'s with their own, once zirc.dev gives them. Nothing a
//! tool sends is logged.

use std::collections::BTreeMap;

use serde_json::Value;
use zirc_proto::inbound::{MAX_MESSAGE, Signs, Tool};
use zirc_proto::role_report::{RoleReport, Subject};

/// What a post comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// Something to report to the role: a new error, issue or incident.
    Report(RoleReport),
    /// The tool says what it reported is resolved there: its item, of `kind` and `key`,
    /// closes.
    Resolved {
        /// The report's kind.
        kind: Subject,
        /// Its key.
        key: String,
    },
    /// A test, as GitHub's ping: heard, and nobody woken.
    Test,
    /// Something the tool sends that no role is woken for, such as a comment.
    Ignored,
    /// From a tool zirc doesn't know: what it sent, as it was sent.
    Message(String),
}

/// Why a post is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// It's signed, or should be, and the signature isn't the tool's.
    Signature,
    /// It isn't what its tool sends, or says nothing.
    Unreadable,
}

/// The secrets posts may be signed with.
#[derive(Debug, Clone, Copy)]
pub struct Signing<'a> {
    /// The address's own secret, as the post came to it.
    pub address: &'a str,
    /// Each tool's own signing secret, where zirc.dev gave it.
    pub tools: &'a BTreeMap<Tool, String>,
}

impl Signing<'_> {
    /// The secret `tool` signs with, where the server has it: its own, or for GitHub, the
    /// address's.
    fn of(&self, tool: Tool) -> Option<&str> {
        match tool.signs()? {
            Signs::Address => Some(self.address),
            Signs::Own => self.tools.get(&tool).map(String::as_str),
        }
    }
}

/// Which tool posted, from its headers (`header` answers one by its lowercase name) and,
/// for Sentry's plugin and `PagerDuty`, its body.
#[must_use]
pub fn tool_of(header: &dyn Fn(&str) -> Option<String>, body: &Value) -> Tool {
    if header("sentry-hook-resource").is_some() {
        return Tool::Sentry;
    }
    if header("x-github-event").is_some() {
        return Tool::Github;
    }
    if header("linear-event").is_some() || header("linear-signature").is_some() {
        return Tool::Linear;
    }
    if header("x-pagerduty-signature").is_some()
        || body["event"]["event_type"].is_string() && body["event"]["resource_type"].is_string()
    {
        return Tool::Pagerduty;
    }
    if body["project_slug"].is_string() && body["triggering_rules"].is_array() {
        return Tool::Sentry;
    }
    Tool::Other
}

/// What `body`, posted to role `role`'s address with `header`s, comes to, and which tool
/// sent it.
///
/// # Errors
///
/// When it's signed wrongly, or unsigned where the server has the tool's secret, or isn't
/// what its tool sends.
pub fn read(
    role: &str,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
    signing: Signing<'_>,
) -> Result<(Tool, Read), Refused> {
    let json: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let tool = tool_of(header, &json);
    if tool != Tool::Other {
        check_signature(tool, header, body, signing)?;
    }
    let read = match tool {
        Tool::Sentry => sentry(role, header, &json),
        Tool::Github => github(role, header, &json),
        Tool::Linear => linear(role, &json),
        Tool::Pagerduty => pagerduty(role, &json),
        Tool::Other => message(body),
    }
    .ok_or(Refused::Unreadable)?;
    Ok((tool, read))
}

/// Whether `body` is signed by `tool`, where the server has its secret: a post that should be
/// signed and isn't, or is signed with another secret, is refused. A tool whose secret the
/// server doesn't have is taken on its address's secret alone.
fn check_signature(
    tool: Tool,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
    signing: Signing<'_>,
) -> Result<(), Refused> {
    let Some(secret) = signing.of(tool) else {
        return Ok(());
    };
    let signed = match tool {
        Tool::Github => match header("x-hub-signature-256") {
            // GitHub signs only where its webhook has a secret: one without is the address's
            // alone.
            None => return Ok(()),
            Some(signature) => signature
                .strip_prefix("sha256=")
                .is_some_and(|hex| zirc_proto::verify_hex(secret, body, hex)),
        },
        Tool::Sentry => header("sentry-hook-signature")
            .is_some_and(|hex| zirc_proto::verify_hex(secret, body, &hex)),
        Tool::Linear => {
            header("linear-signature").is_some_and(|hex| zirc_proto::verify_hex(secret, body, &hex))
        }
        // `v1=<hex>`, more than one while PagerDuty rotates its secret.
        Tool::Pagerduty => header("x-pagerduty-signature").is_some_and(|said| {
            said.split(',')
                .filter_map(|one| one.trim().strip_prefix("v1="))
                .any(|hex| zirc_proto::verify_hex(secret, body, hex))
        }),
        Tool::Other => true,
    };
    if signed {
        Ok(())
    } else {
        Err(Refused::Signature)
    }
}

/// `value` as a string, when it's one that says something; a number as its digits.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// What's known of something a tool reports, beside its kind and key.
struct Known {
    summary: String,
    details: Option<String>,
    reference: Option<String>,
    link: Option<String>,
}

/// A report from `tool` to `role` of `kind` about `key`, with what's `known` of it: a link
/// only to an `https://` address.
fn report(role: &str, tool: Tool, kind: Subject, key: &str, known: Known) -> Read {
    Read::Report(RoleReport {
        summary: known.summary,
        details: known.details,
        reference: known.reference,
        link: known.link.filter(|link| link.starts_with("https://")),
        tool: Some(tool),
        ..RoleReport::new(role, kind, key)
    })
}

/// What Sentry's error says in a line: `TypeError in web: Cannot read …`, from its type and
/// value where it has them, else its title.
fn sentry_summary(thing: &Value, project: Option<&str>) -> Option<String> {
    let metadata = &thing["metadata"];
    let place = project.map(|p| format!(" in {p}")).unwrap_or_default();
    // Sentry titles an event it can't name `<untitled>`, as its own test sends: that's no
    // title, and its message or exception says more.
    let titled = text(&thing["title"]).filter(|title| title != SENTRY_UNTITLED);
    let exception = &thing["exception"]["values"][0];
    let thrown = match (text(&exception["type"]), text(&exception["value"])) {
        (Some(kind), Some(value)) => Some(format!("{kind}: {value}")),
        (kind, value) => kind.or(value),
    };
    match (text(&metadata["type"]), text(&metadata["value"])) {
        (Some(kind), Some(value)) => Some(format!("{kind}{place}: {value}")),
        _ => titled
            .or_else(|| text(&thing["message"]))
            .or(thrown)
            .map(|title| format!("{title}{place}")),
    }
}

/// What Sentry titles an event it can't name.
const SENTRY_UNTITLED: &str = "<untitled>";

/// Sentry's post: an issue's (created, or resolved), an error's, an event alert's, its
/// integration's installation (a test), or its `WebHooks` plugin's alert.
fn sentry(role: &str, header: &dyn Fn(&str) -> Option<String>, json: &Value) -> Option<Read> {
    let report_of = |thing: &Value, issue: Option<String>, project: Option<&str>| {
        let issue = issue?;
        let known = Known {
            summary: sentry_summary(thing, project)?,
            details: text(&thing["culprit"]).or_else(|| text(&thing["location"])),
            reference: text(&thing["shortId"]),
            link: text(&thing["web_url"])
                .or_else(|| text(&thing["permalink"]))
                .or_else(|| text(&thing["url"])),
        };
        Some(report(
            role,
            Tool::Sentry,
            Subject::Error,
            &format!("sentry:{issue}"),
            known,
        ))
    };
    let Some(resource) = header("sentry-hook-resource") else {
        // The WebHooks plugin's alert: the issue's id, and the event.
        let event = &json["event"];
        let mut thing = event.clone();
        thing["culprit"] = json["culprit"].clone();
        thing["web_url"] = json["url"].clone();
        if thing["title"].is_null() {
            thing["title"] = json["message"].clone();
        }
        return report_of(&thing, text(&json["id"]), json["project_slug"].as_str());
    };
    let data = &json["data"];
    match (resource.as_str(), json["action"].as_str()) {
        ("installation", _) => Some(Read::Test),
        ("issue", Some("created" | "unresolved")) => {
            let issue = &data["issue"];
            report_of(issue, text(&issue["id"]), issue["project"]["slug"].as_str())
        }
        ("issue", Some("resolved")) => Some(Read::Resolved {
            kind: Subject::Error,
            key: format!("sentry:{}", text(&data["issue"]["id"])?),
        }),
        ("event_alert", _) => {
            let event = &data["event"];
            report_of(event, text(&event["issue_id"]), None)
        }
        ("error", Some("created")) => {
            let error = &data["error"];
            report_of(error, text(&error["issue_id"]), None)
        }
        _ if json.is_object() => Some(Read::Ignored),
        _ => None,
    }
}

/// GitHub's post: an issue opened or reopened, one closed, or its ping.
fn github(role: &str, header: &dyn Fn(&str) -> Option<String>, json: &Value) -> Option<Read> {
    let event = header("x-github-event")?;
    if !json.is_object() {
        return None;
    }
    if event == "ping" {
        return Some(Read::Test);
    }
    if event != "issues" {
        return Some(Read::Ignored);
    }
    let issue = &json["issue"];
    let repository = text(&json["repository"]["full_name"]);
    let number = text(&issue["number"])?;
    let key = format!("github:{}#{number}", repository.as_deref().unwrap_or(""));
    match json["action"].as_str() {
        Some("opened" | "reopened") => {
            let by = text(&issue["user"]["login"]).map(|login| format!("opened by {login}"));
            let details = match (by, repository) {
                (Some(by), Some(repository)) => Some(format!("{by} in {repository}")),
                (by, repository) => by.or(repository),
            };
            let known = Known {
                summary: text(&issue["title"])?,
                details,
                reference: Some(format!("#{number}")),
                link: text(&issue["html_url"]),
            };
            Some(report(role, Tool::Github, Subject::Issue, &key, known))
        }
        Some("closed") => Some(Read::Resolved {
            kind: Subject::Issue,
            key,
        }),
        _ => Some(Read::Ignored),
    }
}

/// Linear's post: an issue created, one completed, cancelled or removed.
fn linear(role: &str, json: &Value) -> Option<Read> {
    if !json.is_object() {
        return None;
    }
    if json["type"].as_str() != Some("Issue") {
        return Some(Read::Ignored);
    }
    let data = &json["data"];
    let key = format!("linear:{}", text(&data["id"])?);
    let ended = |field: &str| {
        !data[field].is_null() && json["updatedFrom"].get(field).is_some_and(Value::is_null)
    };
    match json["action"].as_str() {
        Some("create") => {
            let known = Known {
                summary: text(&data["title"])?,
                details: text(&data["team"]["name"]),
                reference: text(&data["identifier"]),
                link: text(&data["url"]).or_else(|| text(&json["url"])),
            };
            Some(report(role, Tool::Linear, Subject::Issue, &key, known))
        }
        Some("remove") => Some(Read::Resolved {
            kind: Subject::Issue,
            key,
        }),
        Some("update") if ended("completedAt") || ended("canceledAt") => Some(Read::Resolved {
            kind: Subject::Issue,
            key,
        }),
        _ => Some(Read::Ignored),
    }
}

/// `PagerDuty`'s post: an incident triggered or resolved, or its ping.
fn pagerduty(role: &str, json: &Value) -> Option<Read> {
    let event = &json["event"];
    let data = &event["data"];
    match event["event_type"].as_str()? {
        "pagey.ping" => Some(Read::Test),
        "incident.triggered" => {
            let service = text(&data["service"]["summary"]);
            let urgency = text(&data["urgency"]).map(|u| format!("{u} urgency"));
            let details = [service, urgency].into_iter().flatten().collect::<Vec<_>>();
            let known = Known {
                summary: text(&data["title"])?,
                details: (!details.is_empty()).then(|| details.join(" · ")),
                reference: text(&data["number"]).map(|n| format!("#{n}")),
                link: text(&data["html_url"]),
            };
            let key = format!("pagerduty:{}", text(&data["id"])?);
            Some(report(
                role,
                Tool::Pagerduty,
                Subject::Incident,
                &key,
                known,
            ))
        }
        "incident.resolved" => Some(Read::Resolved {
            kind: Subject::Incident,
            key: format!("pagerduty:{}", text(&data["id"])?),
        }),
        _ => Some(Read::Ignored),
    }
}

/// An unknown tool's post, as it was sent: its text, without control characters but its
/// lines, cut at [`MAX_MESSAGE`] characters. Nothing, when it says nothing.
fn message(body: &[u8]) -> Option<Read> {
    let text: String = String::from_utf8_lossy(body)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(Read::Message(zirc_proto::clip(text, MAX_MESSAGE)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "k7Qp2vXe9LmR0aBcDeFgHiJk";

    fn post(
        headers: &[(&str, &str)],
        body: &[u8],
        tools: &BTreeMap<Tool, String>,
    ) -> Result<(Tool, Read), Refused> {
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let header = move |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        read(
            "oncall",
            &header,
            body,
            Signing {
                address: SECRET,
                tools,
            },
        )
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        zirc_proto::sign_hex(secret, body)
    }

    fn reported(result: Result<(Tool, Read), Refused>) -> RoleReport {
        match result {
            Ok((_, Read::Report(report))) => report,
            other => panic!("not a report: {other:?}"),
        }
    }

    const SENTRY_ISSUE: &[u8] = include_bytes!("testdata/sentry_issue_created.json");

    #[test]
    fn a_sentry_issue_is_an_error_keyed_by_the_issue() {
        let report = reported(post(
            &[("sentry-hook-resource", "issue")],
            SENTRY_ISSUE,
            &BTreeMap::new(),
        ));
        assert_eq!(report.kind, Subject::Error);
        assert_eq!(report.tool, Some(Tool::Sentry));
        assert_eq!(report.key, "sentry:4821007");
        assert_eq!(
            report.summary,
            "TypeError in web: Cannot read properties of undefined (reading 'slug')"
        );
        assert_eq!(
            report.details.as_deref(),
            Some("web/src/app/orgs/[slug]/page.tsx in OrgPage")
        );
        assert_eq!(report.reference.as_deref(), Some("WEB-7"));
        assert_eq!(
            report.link.as_deref(),
            Some("https://acme.sentry.io/organizations/acme/issues/4821007/")
        );
        assert_eq!(
            report.words(),
            "Sentry: TypeError in web: Cannot read properties of undefined (reading 'slug')"
        );
    }

    #[test]
    fn every_way_sentry_says_one_issue_is_the_same_item() {
        let none = BTreeMap::new();
        let issue = reported(post(
            &[("sentry-hook-resource", "issue")],
            SENTRY_ISSUE,
            &none,
        ));
        let alert = reported(post(
            &[("sentry-hook-resource", "event_alert")],
            include_bytes!("testdata/sentry_event_alert.json"),
            &none,
        ));
        let plugin = reported(post(
            &[("user-agent", "sentry-plugins/0.1")],
            include_bytes!("testdata/sentry_plugin.json"),
            &none,
        ));
        assert_eq!(issue.id(), alert.id());
        assert_eq!(issue.id(), plugin.id());
        assert_eq!(plugin.summary, issue.summary);
        let error = reported(post(
            &[("sentry-hook-resource", "error")],
            include_bytes!("testdata/sentry_error.json"),
            &none,
        ));
        assert_eq!(error.summary, "zircd panicked in history::replay");
        assert_eq!(error.key, "sentry:4821011");
        assert_eq!(
            post(
                &[("sentry-hook-resource", "issue")],
                include_bytes!("testdata/sentry_issue_resolved.json"),
                &none
            ),
            Ok((
                Tool::Sentry,
                Read::Resolved {
                    kind: Subject::Error,
                    key: "sentry:4821007".into()
                }
            ))
        );
        assert_eq!(
            post(
                &[("sentry-hook-resource", "installation")],
                b"{\"action\":\"created\"}",
                &none
            ),
            Ok((Tool::Sentry, Read::Test))
        );
        assert_eq!(
            post(
                &[("sentry-hook-resource", "comment")],
                b"{\"action\":\"created\"}",
                &none
            ),
            Ok((Tool::Sentry, Read::Ignored))
        );
    }

    #[test]
    fn sentrys_untitled_test_event_is_said_by_its_message() {
        let none = BTreeMap::new();
        let report = reported(post(
            &[("sentry-hook-resource", "event_alert")],
            include_bytes!("testdata/sentry_event_alert_untitled.json"),
            &none,
        ));
        assert_eq!(report.summary, "This is an example Next.js exception");
        let mut bare: Value =
            serde_json::from_slice(include_bytes!("testdata/sentry_event_alert_untitled.json"))
                .unwrap();
        bare["data"]["event"]["message"] = Value::Null;
        let report = reported(post(
            &[("sentry-hook-resource", "event_alert")],
            &serde_json::to_vec(&bare).unwrap(),
            &none,
        ));
        assert_eq!(
            report.summary,
            "TypeError: Object [object Object] has no method 'updateFrom'"
        );
    }

    #[test]
    fn sentry_is_checked_once_the_server_has_its_client_secret() {
        let tools = BTreeMap::from([(Tool::Sentry, "sentry-client-secret".to_owned())]);
        let good = sign("sentry-client-secret", SENTRY_ISSUE);
        assert!(
            post(
                &[
                    ("sentry-hook-resource", "issue"),
                    ("sentry-hook-signature", &good)
                ],
                SENTRY_ISSUE,
                &tools
            )
            .is_ok()
        );
        let bad = sign("another", SENTRY_ISSUE);
        assert_eq!(
            post(
                &[
                    ("sentry-hook-resource", "issue"),
                    ("sentry-hook-signature", &bad)
                ],
                SENTRY_ISSUE,
                &tools
            ),
            Err(Refused::Signature)
        );
        assert_eq!(
            post(&[("sentry-hook-resource", "issue")], SENTRY_ISSUE, &tools),
            Err(Refused::Signature)
        );
        // Without it, a signature can't be checked, and the address's secret admits it.
        assert!(
            post(
                &[
                    ("sentry-hook-resource", "issue"),
                    ("sentry-hook-signature", &bad)
                ],
                SENTRY_ISSUE,
                &BTreeMap::new()
            )
            .is_ok()
        );
    }

    const GITHUB_ISSUE: &[u8] = include_bytes!("testdata/github_issues_opened.json");

    #[test]
    fn a_github_issue_opened_is_a_new_issue_signed_with_the_address() {
        let none = BTreeMap::new();
        let signature = format!("sha256={}", sign(SECRET, GITHUB_ISSUE));
        let report = reported(post(
            &[
                ("x-github-event", "issues"),
                ("x-hub-signature-256", &signature),
            ],
            GITHUB_ISSUE,
            &none,
        ));
        assert_eq!(report.kind, Subject::Issue);
        assert_eq!(report.key, "github:acme/server#412");
        assert_eq!(
            report.summary,
            "Reconnect drops history on iPad after sleep"
        );
        assert_eq!(
            report.details.as_deref(),
            Some("opened by example-customer in acme/server")
        );
        assert_eq!(report.reference.as_deref(), Some("#412"));
        assert_eq!(
            report.link.as_deref(),
            Some("https://github.com/acme/server/issues/412")
        );
        // Signed with another secret: refused. Unsigned: the address's secret admits it.
        let wrong = format!("sha256={}", sign("guess", GITHUB_ISSUE));
        assert_eq!(
            post(
                &[
                    ("x-github-event", "issues"),
                    ("x-hub-signature-256", &wrong)
                ],
                GITHUB_ISSUE,
                &none
            ),
            Err(Refused::Signature)
        );
        assert!(post(&[("x-github-event", "issues")], GITHUB_ISSUE, &none).is_ok());
    }

    #[test]
    fn githubs_ping_is_a_test_and_a_closed_issue_resolves_it() {
        let none = BTreeMap::new();
        assert_eq!(
            post(
                &[("x-github-event", "ping")],
                include_bytes!("testdata/github_ping.json"),
                &none
            ),
            Ok((Tool::Github, Read::Test))
        );
        assert_eq!(
            post(&[("x-github-event", "push")], b"{}", &none),
            Ok((Tool::Github, Read::Ignored))
        );
        let closed = String::from_utf8_lossy(GITHUB_ISSUE).replace("\"opened\"", "\"closed\"");
        assert_eq!(
            post(&[("x-github-event", "issues")], closed.as_bytes(), &none),
            Ok((
                Tool::Github,
                Read::Resolved {
                    kind: Subject::Issue,
                    key: "github:acme/server#412".into()
                }
            ))
        );
    }

    #[test]
    fn a_linear_issue_created_is_a_new_issue_and_done_resolves_it() {
        let body = include_bytes!("testdata/linear_issue_create.json");
        let tools = BTreeMap::from([(Tool::Linear, "lin_wh_example".to_owned())]);
        let signature = sign("lin_wh_example", body);
        let report = reported(post(
            &[("linear-event", "Issue"), ("linear-signature", &signature)],
            body,
            &tools,
        ));
        assert_eq!(report.kind, Subject::Issue);
        assert_eq!(report.key, "linear:7a1b2c3d-0000-4000-8000-000000000012");
        assert_eq!(
            report.summary,
            "Invites to a deleted organisation answer 500"
        );
        assert_eq!(report.reference.as_deref(), Some("ENG-12"));
        assert_eq!(report.details.as_deref(), Some("Engineering"));
        assert_eq!(
            post(&[("linear-event", "Issue")], body, &tools),
            Err(Refused::Signature)
        );
        let done = include_bytes!("testdata/linear_issue_done.json");
        assert_eq!(
            post(&[("linear-event", "Issue")], done, &BTreeMap::new()),
            Ok((
                Tool::Linear,
                Read::Resolved {
                    kind: Subject::Issue,
                    key: report.key.clone()
                }
            ))
        );
        assert_eq!(
            post(
                &[("linear-event", "Comment")],
                b"{\"type\":\"Comment\",\"action\":\"create\"}",
                &BTreeMap::new()
            ),
            Ok((Tool::Linear, Read::Ignored))
        );
    }

    #[test]
    fn a_pagerduty_incident_triggered_is_an_incident_until_resolved() {
        let body = include_bytes!("testdata/pagerduty_incident_triggered.json");
        let tools = BTreeMap::from([(Tool::Pagerduty, "pd-signing".to_owned())]);
        // PagerDuty sends more than one while it rotates: any of them will do.
        let signature = format!("v1=00ff, v1={}", sign("pd-signing", body));
        let report = reported(post(&[("x-pagerduty-signature", &signature)], body, &tools));
        assert_eq!(report.kind, Subject::Incident);
        assert_eq!(report.key, "pagerduty:Q0EXAMPLE0001");
        assert_eq!(report.summary, "API latency p99 over 2 s");
        assert_eq!(
            report.details.as_deref(),
            Some("zirc.dev web · high urgency")
        );
        assert_eq!(report.reference.as_deref(), Some("#3"));
        assert_eq!(
            report.link.as_deref(),
            Some("https://acme.pagerduty.com/incidents/Q0EXAMPLE0001")
        );
        assert_eq!(
            post(&[("x-pagerduty-signature", "v1=00ff")], body, &tools),
            Err(Refused::Signature)
        );
        // Known by its body, too.
        let none = BTreeMap::new();
        assert_eq!(
            post(
                &[],
                include_bytes!("testdata/pagerduty_incident_resolved.json"),
                &none
            ),
            Ok((
                Tool::Pagerduty,
                Read::Resolved {
                    kind: Subject::Incident,
                    key: report.key.clone()
                }
            ))
        );
        assert_eq!(
            post(&[], include_bytes!("testdata/pagerduty_ping.json"), &none),
            Ok((Tool::Pagerduty, Read::Test))
        );
    }

    #[test]
    fn anything_else_is_a_message_as_it_was_sent() {
        let none = BTreeMap::new();
        assert_eq!(
            post(
                &[],
                b"backup job nightly-db finished with warnings\n\x1b[0mtwo tables skipped",
                &none
            ),
            Ok((
                Tool::Other,
                Read::Message(
                    "backup job nightly-db finished with warnings\n[0mtwo tables skipped".into()
                )
            ))
        );
        let json = br#"{"status":"warning","job":"nightly-db"}"#;
        assert_eq!(
            post(&[], json, &none),
            Ok((
                Tool::Other,
                Read::Message(String::from_utf8_lossy(json).into_owned())
            ))
        );
        let long = "x".repeat(MAX_MESSAGE * 2);
        let Ok((_, Read::Message(cut))) = post(&[], long.as_bytes(), &none) else {
            panic!("a message");
        };
        assert_eq!(cut.chars().count(), MAX_MESSAGE);
        assert_eq!(post(&[], b"  \n ", &none), Err(Refused::Unreadable));
    }

    #[test]
    fn what_a_known_tool_sends_must_be_what_it_sends() {
        let none = BTreeMap::new();
        assert_eq!(
            post(&[("x-github-event", "issues")], b"not json", &none),
            Err(Refused::Unreadable)
        );
        assert_eq!(
            post(
                &[("x-github-event", "issues")],
                b"{\"action\":\"opened\"}",
                &none
            ),
            Err(Refused::Unreadable)
        );
    }
}
