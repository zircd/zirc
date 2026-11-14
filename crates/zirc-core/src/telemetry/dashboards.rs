//! zirc's `PostHog` dashboard, "zirc usage", declared next to the events it charts: active
//! organisations, people and agents; the sign-up funnel, organisations' activation and
//! subscriptions; clients and versions; features; commands by people and agents; countries. `bazel run //crates/zirc-core:posthog_dashboards` makes them in the project,
//! replacing the ones it made before, so they change only here. A test checks that every
//! insight charts declared events and properties only.

use serde_json::{Value, json};

/// One dashboard and its insights.
pub struct Dashboard {
    /// Its name, which [`apply`] finds it again by.
    pub name: &'static str,
    /// What it shows.
    pub description: &'static str,
    /// Its insights, in order.
    pub insights: Vec<Insight>,
}

/// One chart.
pub struct Insight {
    /// Its title.
    pub name: &'static str,
    /// What it shows.
    pub description: &'static str,
    /// `PostHog`'s query for it.
    pub query: Value,
    /// The events a SQL query reads, which its text names: the others' are in the query.
    pub events: &'static [&'static str],
}

/// What the project's "filter out test accounts" leaves out, which every insight turns on, as
/// a backstop to tests and automation not sending at all:
/// - app events from development builds (tests' and developers'), and those from before app
///   events said whether they were (they were nearly all tests');
/// - app events from installs marked internal, zirc's CI's, tests' and team's
///   (`ZIRC_TELEMETRY_INTERNAL=1`);
/// - organisations' days on servers other than production's, and zirc's own organisations'
///   (plan `internal`), from the controller and from zirc.dev's server alike;
/// - the website seen by automation: a browser driven by `WebDriver` (Playwright) or an
///   agent's browser (Claude's), or anywhere but zirc.dev.
pub const TEST_FILTER: &str = "not(\
    (properties.$lib = 'zirc' and (coalesce(toBool(properties.dev), true) \
        or coalesce(toBool(properties.internal), false))) \
    or (event = 'org_daily' and (properties.environment != 'production' \
        or properties.plan = 'internal')) \
    or (properties.$lib = 'zirc.dev' and coalesce(toBool(properties.internal), false)) \
    or (event = '$groupidentify' and coalesce(toBool(properties.$group_set.internal), false)) \
    or (properties.$lib = 'web' and (properties.$host != 'zirc.dev' \
        or properties.$raw_user_agent ilike '%HeadlessChrome%' \
        or properties.$raw_user_agent ilike '% Claude/%')))";

/// The website's page views, which posthog-js sends: the one event not declared in
/// [`zirc_proto::telemetry`].
const PAGEVIEW: &str = "$pageview";

/// A series of `event`, counted by `math` (`total`, `dau`, `weekly_active`, or `sum` of
/// `property`), of the events whose `filters` match: each a property, how it's compared,
/// such as `exact` or `gt`, and with what.
fn series(
    event: &str,
    math: &str,
    property: Option<&str>,
    filters: &[(&str, &str, Value)],
) -> Value {
    let mut node = json!({"kind": "EventsNode", "event": event, "name": event, "math": math});
    if let Some(property) = property {
        node["math_property"] = property.into();
    }
    if !filters.is_empty() {
        node["properties"] = filters
            .iter()
            .map(|(key, operator, value)| {
                json!({"key": key, "value": value, "operator": operator, "type": "event"})
            })
            .collect();
    }
    node
}

/// Shown as what `display` says, such as `ActionsLineGraph` or `ActionsBarValue`.
fn trends(series: Vec<Value>, breakdown: Option<&str>, display: &str, interval: &str) -> Value {
    let mut source = json!({
        "kind": "TrendsQuery",
        "filterTestAccounts": true,
        "series": Value::Array(series),
        "interval": interval,
        "dateRange": {"date_from": "-90d"},
        "trendsFilter": {"display": display},
    });
    if let Some(breakdown) = breakdown {
        source["breakdownFilter"] = json!({"breakdown": breakdown, "breakdown_type": "event"});
    }
    json!({"kind": "InsightVizNode", "source": source})
}

fn line(series: Vec<Value>, breakdown: Option<&str>) -> Value {
    trends(series, breakdown, "ActionsLineGraph", "day")
}

fn bars(series: Vec<Value>, breakdown: &str) -> Value {
    trends(series, Some(breakdown), "ActionsBarValue", "day")
}

fn insight(name: &'static str, description: &'static str, query: Value) -> Insight {
    Insight {
        name,
        description,
        query,
        events: &[],
    }
}

/// A table from SQL (`HogQL`) reading `events`, with the test filter where it says
/// `{filters}`.
fn table(
    name: &'static str,
    description: &'static str,
    events: &'static [&'static str],
    sql: &str,
) -> Insight {
    Insight {
        name,
        description,
        query: json!({
            "kind": "DataTableNode",
            "source": {
                "kind": "HogQLQuery",
                "query": sql,
                "filters": {"filterTestAccounts": true, "dateRange": {"date_from": "-90d"}},
            },
        }),
        events,
    }
}

/// The organisation an event is about: zirc.dev's events say so, and every organisation's event
/// is its group's, as zircd's are.
const ORGANISATION: &str =
    "coalesce(nullIf(properties.organisation_id, ''), nullIf(properties.$groups.organisation, ''))";

/// What organisations made in the last 90 days went on to do, and whether they activated: a
/// second person, an agent's intent and an overlap notice between machines, all within a week of
/// being made.
fn activation() -> String {
    format!(
        "SELECT count() AS made, \
           countIf(posted > made_at) AS posted_a_message, \
           countIf(joined > made_at) AS second_person, \
           countIf(intent > made_at) AS intent_posted, \
           countIf(overlap > made_at) AS overlap_notified, \
           countIf(joined > made_at AND intent > made_at AND overlap > made_at \
             AND greatest(greatest(joined, intent), overlap) <= made_at + INTERVAL 7 DAY) AS activated_in_a_week, \
           countIf(subscribed > made_at) AS subscribed \
         FROM (SELECT {ORGANISATION} AS organisation, \
             minIf(timestamp, event = 'organisation_created') AS made_at, \
             minIf(timestamp, event = 'first_message') AS posted, \
             minIf(timestamp, event = 'member_joined') AS joined, \
             minIf(timestamp, event = 'intent_posted') AS intent, \
             minIf(timestamp, event = 'overlap_notified' AND NOT toBool(properties.local)) AS overlap, \
             minIf(timestamp, event = 'subscription_started') AS subscribed \
           FROM events \
           WHERE event IN ('organisation_created', 'first_message', 'member_joined', \
             'intent_posted', 'overlap_notified', 'subscription_started') AND {{filters}} \
           GROUP BY organisation \
           HAVING organisation IS NOT NULL AND made_at > toDateTime(0))"
    )
}

/// Dashboards made before, by name, which [`Api::apply`] deletes: they became sections of
/// "zirc usage".
pub const RETIRED: &[&str] = &[
    "zirc: active use",
    "zirc: sign-up funnel",
    "zirc: clients and versions",
    "zirc: features and commands",
];

/// A funnel through `steps`, each within two weeks of the first.
fn funnel(steps: &[&str]) -> Value {
    json!({
        "kind": "InsightVizNode",
        "source": {
            "kind": "FunnelsQuery",
            "filterTestAccounts": true,
            "series": steps.iter().map(|step| series(step, "total", None, &[])).collect::<Vec<_>>(),
            "dateRange": {"date_from": "-90d"},
            "funnelsFilter": {"funnelWindowInterval": 14, "funnelWindowIntervalUnit": "day"},
        },
    })
}

/// The dashboards: one, "zirc usage", read top to bottom.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one list of dashboards, read top to bottom"
)]
pub fn dashboards() -> Vec<Dashboard> {
    let opened = |math| series("app_opened", math, None, &[]);
    let by = |agent: bool| {
        series(
            "command_run",
            "total",
            None,
            &[("agent", "exact", agent.into())],
        )
    };
    vec![Dashboard {
        name: "zirc usage",
        description: "Whether people use zirc, which parts, and where they stop on the way in. \
                      Declared in oss/crates/zirc-core/src/telemetry/dashboards.rs: \
                      change it there and apply it again, not here.",
        insights: vec![
            // Active organisations, people and agents, from the controller's daily summary.
            insight(
                "Active organisations",
                "Organisations with anyone active that day (org_daily).",
                line(
                    vec![series(
                        "org_daily",
                        "dau",
                        None,
                        &[("active_day_people", "gt", 0.into())],
                    )],
                    None,
                ),
            ),
            insight(
                "Active people and agents",
                "People, and agents' machines, active that day, summed over organisations \
                 (org_daily).",
                line(
                    vec![
                        series("org_daily", "sum", Some("active_day_people"), &[]),
                        series("org_daily", "sum", Some("active_day_agents"), &[]),
                    ],
                    None,
                ),
            ),
            insight(
                "Messages by people and by agents",
                "Messages posted each day, summed over organisations (org_daily).",
                line(
                    vec![
                        series("org_daily", "sum", Some("messages_day_people"), &[]),
                        series("org_daily", "sum", Some("messages_day_agents"), &[]),
                    ],
                    None,
                ),
            ),
            // The way in.
            insight(
                "Sign-up funnel",
                "Visitors who go on to sign up, create an organisation, start a trial and \
                 open the checkout to subscribe, each within two weeks.",
                funnel(&[
                    PAGEVIEW,
                    "signed_up",
                    "organisation_created",
                    "trial_started",
                    "checkout_opened",
                ]),
            ),
            table(
                "Organisations' activation",
                "Of the organisations made in the last 90 days, how many posted a message, \
                 got a second person, had an agent post an intent, got an overlap notice \
                 between machines, did all three within a week of being made (activated), \
                 and subscribed.",
                &[
                    "organisation_created",
                    "first_message",
                    "member_joined",
                    "intent_posted",
                    "overlap_notified",
                    "subscription_started",
                ],
                &activation(),
            ),
            insight(
                "Activation, each week",
                "Invites, people joining, agents' intents, and overlap and conflict-ahead \
                 notices.",
                trends(
                    vec![
                        series("invite_sent", "total", None, &[]),
                        series("member_joined", "total", None, &[]),
                        series("intent_posted", "total", None, &[]),
                        series("overlap_notified", "total", None, &[]),
                        series("conflict_ahead_notified", "total", None, &[]),
                    ],
                    None,
                    "ActionsBar",
                    "week",
                ),
            ),
            insight(
                "Overlap notices by kind",
                "Overlap notices each week by what they amounted to: the same lines (a clash), \
                 different parts of a file (these don't wake a session), only a path, or a \
                 lockfile. Notices that escalated were first told as different parts.",
                trends(
                    vec![series("overlap_notified", "total", None, &[])],
                    Some("kind"),
                    "ActionsBar",
                    "week",
                ),
            ),
            table(
                "Organisations by first touch",
                "Organisations made, by how their creator first came to zirc.dev: the \
                 referring site and the campaign tags of the page they signed up on.",
                &["$groupidentify"],
                "SELECT coalesce(properties.$group_set.utm_source, '') AS utm_source, \
                   coalesce(properties.$group_set.utm_medium, '') AS utm_medium, \
                   coalesce(properties.$group_set.utm_campaign, '') AS utm_campaign, \
                   coalesce(properties.$group_set.referring_domain, '') AS referring_domain, \
                   count(DISTINCT properties.$group_key) AS organisations \
                 FROM events \
                 WHERE event = '$groupidentify' AND properties.$group_type = 'organisation' \
                   AND toBool(properties.$group_set.made) AND {filters} \
                 GROUP BY utm_source, utm_medium, utm_campaign, referring_domain \
                 ORDER BY organisations DESC",
            ),
            insight(
                "Subscriptions",
                "Checkouts opened, subscriptions started and cancelled, each week.",
                trends(
                    vec![
                        series("checkout_opened", "total", None, &[]),
                        series("subscription_started", "total", None, &[]),
                        series("subscription_cancelled", "total", None, &[]),
                    ],
                    None,
                    "ActionsBar",
                    "week",
                ),
            ),
            insight(
                "Sign-ups, organisations and trials",
                "Each week.",
                trends(
                    vec![
                        series("signed_up", "total", None, &[]),
                        series("organisation_created", "total", None, &[]),
                        series("trial_started", "total", None, &[]),
                    ],
                    None,
                    "ActionsBar",
                    "week",
                ),
            ),
            // Clients and versions.
            insight(
                "Daily active installs by app",
                "Installs that opened an app.",
                line(vec![opened("dau")], Some("client")),
            ),
            insight(
                "Installs by operating system",
                "Daily active installs on each system, over the last 90 days.",
                bars(vec![opened("dau")], "os"),
            ),
            insight(
                "Versions running",
                "Installs opening each version, a day.",
                line(vec![opened("dau")], Some("version")),
            ),
            insight(
                "Updates offered and installed",
                "Apps finding and installing newer releases.",
                line(
                    vec![
                        series("update_offered", "total", None, &[]),
                        series("update_installed", "total", None, &[]),
                    ],
                    None,
                ),
            ),
            insight(
                "Connection failures by kind",
                "The first failure in a row, by why.",
                line(
                    vec![series("connection_failed", "total", None, &[])],
                    Some("reason"),
                ),
            ),
            insight(
                "Install retention",
                "Of the installs first seen each week, how many come back in the weeks after.",
                json!({
                    "kind": "InsightVizNode",
                    "source": {
                        "kind": "RetentionQuery",
                        "filterTestAccounts": true,
                        "dateRange": {"date_from": "-90d"},
                        "retentionFilter": {
                            "period": "Week",
                            "totalIntervals": 8,
                            "retentionType": "retention_first_time",
                            "targetEntity": {"id": "app_opened", "name": "app_opened", "type": "events"},
                            "returningEntity": {"id": "app_opened", "name": "app_opened", "type": "events"},
                        },
                    },
                }),
            ),
            // Features.
            insight(
                "Features used",
                "DMs, reactions, files, search, agent DMs and joins, over the last 90 days.",
                bars(vec![series("feature_used", "total", None, &[])], "feature"),
            ),
            insight(
                "Features over time",
                "Each feature, a day.",
                line(
                    vec![series("feature_used", "total", None, &[])],
                    Some("feature"),
                ),
            ),
            // CLI commands, people against agents.
            insight(
                "Commands by people and agents",
                "zirc commands run without --session (people) and with it (agents).",
                line(vec![by(false), by(true)], None),
            ),
            insight(
                "Commands people run",
                "zirc commands by name, run without --session.",
                bars(vec![by(false)], "command"),
            ),
            insight(
                "Commands agents run",
                "zirc commands by name, run with --session.",
                bars(vec![by(true)], "command"),
            ),
            // Countries, from PostHog's GeoIP of where events came from.
            insight(
                "Installs by country",
                "Daily active installs, by the country PostHog places their IP address in.",
                trends(
                    vec![opened("dau")],
                    Some("$geoip_country_code"),
                    "WorldMap",
                    "day",
                ),
            ),
            insight(
                "Website visitors by country",
                "Unique visitors, by the country PostHog places their IP address in.",
                bars(
                    vec![series(PAGEVIEW, "dau", None, &[])],
                    "$geoip_country_name",
                ),
            ),
        ],
    }]
}

/// What went wrong making the dashboards.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `PostHog` couldn't be reached, or refused a request.
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    /// `PostHog` answered with something unexpected.
    #[error("unexpected answer from PostHog: {0}")]
    Answer(String),
}

/// `PostHog`'s API for one project.
pub struct Api {
    /// The HTTP client.
    pub http: reqwest::Client,
    /// Where `PostHog`'s app is, such as `https://eu.posthog.com`.
    pub host: String,
    /// The project's id.
    pub project: String,
    /// A personal API key with access to the project's dashboards and insights.
    pub key: String,
}

impl Api {
    fn url(&self, path: &str) -> String {
        format!(
            "{}/api/projects/{}/{path}",
            self.host.trim_end_matches('/'),
            self.project
        )
    }

    async fn call(&self, request: reqwest::RequestBuilder) -> Result<Value, Error> {
        Ok(request
            .bearer_auth(&self.key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Deletes the dashboard `old` and the insights on it.
    async fn delete(&self, old: &Value) -> Result<(), Error> {
        let id = old["id"].to_string();
        self.delete_insights(&id).await?;
        self.call(
            self.http
                .patch(self.url(&format!("dashboards/{id}/")))
                .json(&json!({"deleted": true})),
        )
        .await?;
        Ok(())
    }

    /// Deletes the insights on the dashboard with id `id`.
    async fn delete_insights(&self, id: &str) -> Result<(), Error> {
        let old = self
            .call(self.http.get(self.url(&format!("dashboards/{id}/"))))
            .await?;
        for tile in old["tiles"].as_array().into_iter().flatten() {
            if let Some(insight) = tile["insight"]["id"].as_u64() {
                self.call(
                    self.http
                        .patch(self.url(&format!("insights/{insight}/")))
                        .json(&json!({"deleted": true})),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Makes `dashboards`, each replacing one of the same name and its insights, and returns
    /// each one's address.
    ///
    /// # Errors
    ///
    /// When `PostHog` can't be reached or refuses a request.
    pub async fn apply(&self, dashboards: &[Dashboard]) -> Result<Vec<String>, Error> {
        self.call(
            self.http
                .patch(format!(
                    "{}/api/projects/{}/",
                    self.host.trim_end_matches('/'),
                    self.project
                ))
                .json(&json!({"test_account_filters": [
                    {"type": "hogql", "key": TEST_FILTER, "value": null}
                ]})),
        )
        .await?;
        let existing = self
            .call(self.http.get(self.url("dashboards/?limit=1000")))
            .await?;
        let existing = existing["results"].as_array().cloned().unwrap_or_default();
        for retired in existing.iter().filter(|d| {
            d["name"]
                .as_str()
                .is_some_and(|name| RETIRED.contains(&name))
        }) {
            self.delete(retired).await?;
        }
        let mut made = Vec::new();
        for dashboard in dashboards {
            let id = if let Some(old) = existing.iter().find(|d| d["name"] == dashboard.name) {
                let id = old["id"].to_string();
                self.delete_insights(&id).await?;
                self.call(
                    self.http
                        .patch(self.url(&format!("dashboards/{id}/")))
                        .json(&json!({"description": dashboard.description})),
                )
                .await?;
                id
            } else {
                let new = self
                    .call(self.http.post(self.url("dashboards/")).json(&json!({
                        "name": dashboard.name,
                        "description": dashboard.description,
                        "tags": ["zirc"],
                    })))
                    .await?;
                match &new["id"] {
                    Value::Number(id) => id.to_string(),
                    other => return Err(Error::Answer(format!("dashboard id {other}"))),
                }
            };
            let dashboard_id: u64 = id
                .parse()
                .map_err(|_| Error::Answer(format!("dashboard id {id}")))?;
            for insight in &dashboard.insights {
                self.call(self.http.post(self.url("insights/")).json(&json!({
                    "name": insight.name,
                    "description": insight.description,
                    "query": insight.query,
                    "dashboards": [dashboard_id],
                    "tags": ["zirc"],
                })))
                .await?;
            }
            made.push(format!(
                "{}/project/{}/dashboard/{id}",
                self.host.trim_end_matches('/'),
                self.project
            ));
        }
        Ok(made)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// Each declared event's properties, the context's included, by its name.
    fn declared() -> BTreeMap<String, BTreeSet<String>> {
        let schema: Value = serde_json::from_str(&crate::generated::telemetry::schema())
            .expect("the schema is JSON");
        let defs = &schema["$defs"];
        let keys = |def: &Value| -> BTreeSet<String> {
            def["properties"]
                .as_object()
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default()
        };
        let context = keys(&defs["Context"]);
        defs["Event"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|variant| {
                let name = variant["properties"]["event"]["const"].as_str().unwrap();
                let def = variant["properties"]["properties"]["$ref"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("#/$defs/");
                (name.to_owned(), &keys(&defs[def]) | &context)
            })
            .collect()
    }

    /// Every `(event, property)` an insight's query names, with `None` for the event alone.
    fn named(query: &Value, out: &mut Vec<(String, Option<String>)>) {
        match query {
            Value::Object(object) => {
                if let Some(Value::String(event)) = object.get("event") {
                    out.push((event.clone(), None));
                    if let Some(Value::String(property)) = object.get("math_property") {
                        out.push((event.clone(), Some(property.clone())));
                    }
                    for filter in object
                        .get("properties")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let key = filter["key"].as_str().unwrap().to_owned();
                        out.push((event.clone(), Some(key)));
                    }
                }
                if object.get("type") == Some(&"events".into())
                    && let Some(Value::String(event)) = object.get("id")
                {
                    out.push((event.clone(), None));
                }
                for value in object.values() {
                    named(value, out);
                }
            }
            Value::Array(values) => values.iter().for_each(|value| named(value, out)),
            _ => {}
        }
    }

    #[test]
    fn leave_out_tests_and_development_builds() {
        for insight in dashboards().iter().flat_map(|d| &d.insights) {
            let source = &insight.query["source"];
            let filtered = if source["kind"] == "HogQLQuery" {
                source["query"].as_str().unwrap().contains("{filters}")
                    && source["filters"]["filterTestAccounts"] == true
            } else {
                source["filterTestAccounts"] == true
            };
            assert!(filtered, "{}", insight.name);
        }
        assert!(declared()["app_opened"].contains("dev"));
    }

    #[test]
    fn chart_only_declared_events_and_properties() {
        let declared = declared();
        for dashboard in dashboards() {
            assert!(!dashboard.insights.is_empty(), "{}", dashboard.name);
            for insight in &dashboard.insights {
                let mut names = Vec::new();
                named(&insight.query, &mut names);
                if let Some(sql) = insight.query["source"]["query"].as_str() {
                    for event in insight.events {
                        assert!(
                            sql.contains(&format!("'{event}'")),
                            "{}: doesn't read {event}",
                            insight.name
                        );
                        names.push(((*event).to_owned(), None));
                    }
                }
                assert!(!names.is_empty(), "{} charts nothing", insight.name);
                let events: BTreeSet<_> = names.iter().map(|(event, _)| event.clone()).collect();
                for (event, property) in &names {
                    // PostHog's own, such as page views and what zirc.dev says of an organisation.
                    if event.starts_with('$') {
                        continue;
                    }
                    let properties = declared
                        .get(event)
                        .unwrap_or_else(|| panic!("{}: no event {event}", insight.name));
                    if let Some(property) = property {
                        assert!(
                            properties.contains(property),
                            "{}: {event} has no {property}",
                            insight.name
                        );
                    }
                }
                let breakdown = &insight.query["source"]["breakdownFilter"]["breakdown"];
                // PostHog's own properties, such as the GeoIP country, are on every event.
                if let Some(breakdown) = breakdown.as_str().filter(|b| !b.starts_with('$')) {
                    for event in &events {
                        assert!(
                            declared[event].contains(breakdown),
                            "{}: {event} has no {breakdown}",
                            insight.name
                        );
                    }
                }
            }
        }
    }
}
