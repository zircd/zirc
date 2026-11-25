//! Tools that reach a role themselves (step 3, the user's choices of 25 Nov 2026): each role
//! has a secret webhook address ([`crate::roles::INBOUND_PATH`]), answered by the
//! organisation's own server, so what a tool posts never passes zirc.dev.
//!
//! What the known tools post ([`Tool`]: Sentry's errors, GitHub's and Linear's new issues,
//! `PagerDuty`'s incidents) becomes a typed report ([`crate::role_report`]), one open item per
//! upstream issue, so 40 events wake the role once. Anything else arrives as a plain message
//! to the role, as it was sent ([`crate::woke::What::Webhook`]).
//!
//! zirc.dev makes each role's secret and keeps it, and gives the server its digest with the
//! roles; rotating it there stops the old one at once. The server says when it last heard
//! each tool, per role ([`Heard`], [`HEARD_PATH`]).
//!
//! Every word apps and zirc.dev show of a tool is here.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;
use crate::card::{self, Card};
use crate::role_report::{OpenItem, RoleReport, Subject};

/// The sidecar's account endpoint for when a role last heard each tool ([`Heard`]): `GET
/// /v1/roles/<role>/heard`.
pub const HEARD_PATH: &str = "/v1/roles/{role}/heard";

/// The most a tool may post at once, in bytes: a Sentry error's event, with its stack, is the
/// largest of the known tools'. More is answered 413.
pub const MAX_BODY: usize = 1024 * 1024;

/// The most characters of an unknown tool's post that reach the role.
pub const MAX_MESSAGE: usize = 2000;

/// How many posts a role takes a minute, beyond which it's answered 429.
pub const A_MINUTE: u32 = 120;

/// How a tool signs what it posts to a role's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "InboundSigns")]
pub enum Signs {
    /// With a secret of its own, which the admin pastes when connecting it: Sentry's client
    /// secret, Linear's and `PagerDuty`'s signing secrets.
    Own,
    /// With the address's own secret, which its webhook is set up with: GitHub's.
    Address,
}

impl Signs {
    /// How it's written: `own`, `address`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::Address => "address",
        }
    }
}

/// A tool that posts to a role's address.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "InboundTool")]
pub enum Tool {
    /// Sentry: its issue and error alerts.
    Sentry,
    /// GitHub: a repository's `issues` events.
    Github,
    /// Linear: its Issue webhooks.
    Linear,
    /// `PagerDuty`: its v3 incident webhooks.
    Pagerduty,
    /// Any other tool: what it posts arrives as a message to the role.
    Other,
}

impl Tool {
    /// Every tool, in the order zirc.dev shows them.
    pub const ALL: [Self; 5] = [
        Self::Sentry,
        Self::Github,
        Self::Linear,
        Self::Pagerduty,
        Self::Other,
    ];

    /// How it's written: `sentry`, `github`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sentry => "sentry",
            Self::Github => "github",
            Self::Linear => "linear",
            Self::Pagerduty => "pagerduty",
            Self::Other => "other",
        }
    }

    /// Its name, as people read it: `Sentry`, `GitHub`, `Any tool`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sentry => "Sentry",
            Self::Github => "GitHub",
            Self::Linear => "Linear",
            Self::Pagerduty => "PagerDuty",
            Self::Other => "Any tool",
        }
    }

    /// What it sends a role, under its name on zirc.dev's "Connect a tool": `errors`.
    #[must_use]
    pub const fn sends(self) -> &'static str {
        match self {
            Self::Sentry => "errors",
            Self::Github | Self::Linear => "new issues",
            Self::Pagerduty => "incidents",
            Self::Other => "a message",
        }
    }

    /// The kind of report what it posts becomes; none for a tool zirc doesn't know.
    #[must_use]
    pub const fn subject(self) -> Option<Subject> {
        match self {
            Self::Sentry => Some(Subject::Error),
            Self::Github | Self::Linear => Some(Subject::Issue),
            Self::Pagerduty => Some(Subject::Incident),
            Self::Other => None,
        }
    }

    /// How it signs its posts: with its own secret for [`crate::roles::SIGNED_BY_OWN`]'s,
    /// with the address's for GitHub, and not at all for a tool zirc doesn't know.
    #[must_use]
    pub fn signs(self) -> Option<Signs> {
        if crate::roles::SIGNED_BY_OWN.contains(&self) {
            Some(Signs::Own)
        } else if self == Self::Github {
            Some(Signs::Address)
        } else {
            None
        }
    }

    /// What a role it posts to wakes on, as its card's "Wakes on" says: `a Sentry error`.
    #[must_use]
    pub const fn wakes_on(self) -> &'static str {
        match self {
            Self::Sentry => "a Sentry error",
            Self::Github => "a new GitHub issue",
            Self::Linear => "a new Linear issue",
            Self::Pagerduty => "a PagerDuty incident",
            Self::Other => "a post from any tool",
        }
    }

    /// The link to what was reported, where it was: `Open in Sentry`.
    #[must_use]
    pub fn open_in(self) -> String {
        format!("Open in {}", self.name())
    }

    /// What reported something, as a card heads it: `Sentry`, `GitHub issue #412`,
    /// `PagerDuty incident #3`, with the tool's own `reference` for it.
    #[must_use]
    pub fn source(self, reference: Option<&str>) -> String {
        let thing = match self {
            Self::Github | Self::Linear => Some("issue"),
            Self::Pagerduty => Some("incident"),
            Self::Sentry | Self::Other => None,
        };
        match (thing, reference) {
            (Some(thing), Some(reference)) => format!("{} {thing} {reference}", self.name()),
            _ => self.name().to_owned(),
        }
    }

    /// The tool `name` is, as [`Tool::as_str`] writes it.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.as_str() == name)
    }
}

/// When a role last heard a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "InboundHeardTool", optional_fields)]
pub struct HeardTool {
    /// The tool.
    pub tool: Tool,
    /// When it last posted, a test aside, in unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub last: Option<u64>,
    /// When it last posted a test, which wakes no one: GitHub's ping, `PagerDuty`'s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub last_test: Option<u64>,
    /// How many times it posted today, UTC, tests aside.
    #[serde(default, skip_serializing_if = "crate::is_zero")]
    pub today: u32,
    /// The day [`HeardTool::today`] counts, in days since 1970, UTC.
    #[serde(default, skip_serializing_if = "crate::is_zero")]
    #[ts(type = "number")]
    pub day: u64,
    /// What zirc.dev's Connect a tool says of it, as the server answers ([`HeardTool::as_of`]):
    /// its words around the time it was last heard, which the page says as its reader reads
    /// times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<HeardSaid>,
}

/// What Connect a tool says of a tool heard: `lead`, then when (`at`, as the page says a time),
/// then `tail`: "✓ Heard from Sentry" · 1 min ago · " · 3 today"; "✓ Heard a test from Sentry"
/// · 1 min ago · ". oncall wasn't woken by it.".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "InboundHeardSaid")]
pub struct HeardSaid {
    /// Before the time.
    pub lead: String,
    /// The time, in unix seconds.
    #[ts(type = "number")]
    pub at: u64,
    /// After the time.
    pub tail: String,
}

impl HeardTool {
    /// `tool`, not heard yet.
    #[must_use]
    pub const fn new(tool: Tool) -> Self {
        Self {
            tool,
            last: None,
            last_test: None,
            today: 0,
            day: 0,
            said: None,
        }
    }

    /// It as the server answers at `now` (unix seconds) for role `role`: today's count only
    /// while it's today's, and what Connect a tool says of it. What it heard last says it: a
    /// test, which woke no one, or a post, with today's count.
    #[must_use]
    pub fn as_of(&self, role: &str, now: u64) -> Self {
        let today = if self.day == now / 86_400 {
            self.today
        } else {
            0
        };
        let name = self.tool.name();
        let said = match (self.last, self.last_test) {
            (last, Some(test)) if last.is_none_or(|last| test >= last) => Some(HeardSaid {
                lead: format!("✓ Heard a test from {name}"),
                at: test,
                tail: format!(". {role} wasn't woken by it."),
            }),
            (Some(last), _) => Some(HeardSaid {
                lead: format!("✓ Heard from {name}"),
                at: last,
                tail: if today > 0 {
                    format!(" · {today} today")
                } else {
                    String::new()
                },
            }),
            _ => None,
        };
        Self {
            today,
            said,
            ..self.clone()
        }
    }

    /// It, having heard a post at `now` (unix seconds), a test or not.
    pub fn heard(&mut self, now: u64, test: bool) {
        if test {
            self.last_test = Some(now);
            return;
        }
        let day = now / 86_400;
        if self.day != day {
            self.day = day;
            self.today = 0;
        }
        self.today = self.today.saturating_add(1);
        self.last = Some(now);
    }
}

/// Which tools a role has heard from and when, as the server answers at [`HEARD_PATH`]: one
/// for each tool heard, in [`Tool::ALL`]'s order. A tool never heard isn't listed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "InboundHeard")]
pub struct Heard {
    /// The role.
    pub role: String,
    /// The tools.
    pub tools: Vec<HeardTool>,
}

/// What a wake and a card say of a tool's report: it's one item however often it's sent.
pub const ONE_OPEN_ITEM: &str = "one open item";

/// What a wake says of a tool's report: `Sentry: TypeError in web · 37 events, one open
/// item`, `still open: …` for a reminder.
#[must_use]
pub fn woke_words(report: &RoleReport, events: u32, reminder: bool) -> String {
    let still = if reminder { "still open: " } else { "" };
    format!(
        "{still}{} · {}, {ONE_OPEN_ITEM}",
        report.words(),
        crate::events(events.max(1))
    )
}

/// What a wake says of a post from a tool zirc doesn't know, which began `excerpt`.
#[must_use]
pub fn webhook_words(excerpt: &str) -> String {
    format!("A webhook from an unknown tool, as it was sent: “{excerpt}”")
}

/// What a role's card in the agents' room heads a tool's report with: `Sentry · woke
/// on-call`, `GitHub issue #412 · vacant role`.
#[must_use]
pub fn card_head(report: &RoleReport, held: bool) -> String {
    let source = report
        .tool
        .unwrap_or(Tool::Other)
        .source(report.reference.as_deref());
    if held {
        format!("{source} · woke {}", report.role)
    } else {
        format!("{source} · vacant role")
    }
}

/// What a vacant role's card labels the command that takes it with.
pub const TAKE_IT: &str = "Take it";

/// The card a role's nick says in the agents' room when a tool's report `item` wakes it, or
/// waits for its next holder: what was reported, by which tool, and where to open it.
#[must_use]
pub fn card(item: &OpenItem, held: bool) -> Card {
    let report = &item.report;
    let tool = report.tool.unwrap_or(Tool::Other);
    let mut note = vec![card_head(report, held)];
    note.extend(report.details.clone());
    if item.reports > 1 {
        note.push(crate::events(item.reports));
    }
    let mut fields = Vec::new();
    if held {
        note.push(ONE_OPEN_ITEM.to_owned());
    } else {
        note.push(format!("waiting for whoever takes {}", report.role));
        fields.push(card::Field {
            label: TAKE_IT.to_owned(),
            value: format!("`{} {}`", crate::roles::TAKE_COMMAND, report.role),
        });
    }
    let links = report
        .link
        .iter()
        .filter(|link| link.starts_with("https://"))
        .map(|link| card::Link {
            label: tool.open_in(),
            url: link.clone(),
        })
        .collect();
    Card {
        id: format!("report/{}", item.id),
        title: crate::clip_line(&report.summary, 80),
        // Its title as the tool said it: a state's word after it ("failed") would misread.
        state: card::State::Info,
        about: None,
        fields,
        links,
        actions: Vec::new(),
        steps: Vec::new(),
        note: Some(crate::clip_line(&note.join(" · "), card::MAX_NOTE)),
        image: None,
    }
}

/// The event a role's nick says its card with ([`card`]): a report's, whose text says it for
/// plain clients, so it wakes no watch and no one's items count it, as only `#ops`'s do.
///
/// # Panics
/// Never: a report and a card always serialise.
#[must_use]
pub fn card_event(item: &OpenItem, held: bool) -> Event {
    let report = &item.report;
    let mut data = serde_json::to_value(report).expect("a report serialises");
    data[card::KEY] = serde_json::to_value(card(item, held)).expect("a card serialises");
    Event::Custom {
        kind: crate::role_report::KIND.to_owned(),
        data,
        text: format!("{}: {}", card_head(report, held), report.summary),
    }
}

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Tool::decl(config),
        Signs::decl(config),
        HeardTool::decl(config),
        HeardSaid::decl(config),
        Heard::decl(config),
        format!("const INBOUND_HEARD_PATH = {HEARD_PATH:?};"),
        format!(
            "const INBOUND_TOOLS: readonly {{ id: InboundTool; name: string; sends: string; wakesOn: string; openIn: string; subject: RoleReportSubject | null; signs: InboundSigns | null }}[] = [{}];",
            Tool::ALL
                .iter()
                .map(|tool| format!(
                    "{{ id: {:?}, name: {:?}, sends: {:?}, wakesOn: {:?}, openIn: {:?}, subject: {}, signs: {} }}",
                    tool.as_str(),
                    tool.name(),
                    tool.sends(),
                    tool.wakes_on(),
                    tool.open_in(),
                    tool.subject().map_or_else(|| "null".to_owned(), |s| format!("{:?}", s.as_str())),
                    tool.signs().map_or_else(|| "null".to_owned(), |s| format!("{:?}", s.as_str()))
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentry() -> RoleReport {
        RoleReport {
            tool: Some(Tool::Sentry),
            summary: "TypeError in web, reading 'slug' of undefined".into(),
            details: Some("web/src/app/orgs/[slug]/page.tsx:41".into()),
            link: Some("https://acme.sentry.io/issues/7/".into()),
            ..RoleReport::new("oncall", Subject::Error, "sentry:7")
        }
    }

    #[test]
    fn a_tools_wake_names_the_tool_and_counts_its_events() {
        assert_eq!(
            woke_words(&sentry(), 37, false),
            "Sentry: TypeError in web, reading 'slug' of undefined · 37 events, one open item"
        );
        assert!(woke_words(&sentry(), 1, true).starts_with("still open: Sentry: "));
        let mut item = OpenItem::opened(sentry(), 0);
        item.reports = 37;
        let woke = crate::woke::Woke::reported(&item, false);
        assert_eq!(woke.tone(), crate::woke::Tone::Failed);
        assert_eq!(
            woke.words(None),
            "Sentry: TypeError in web, reading 'slug' of undefined · 37 events, one open item"
        );
    }

    #[test]
    fn a_landing_that_closes_a_roles_item_says_so() {
        let woke = crate::woke::Woke::closed_by_landing(
            "oncall",
            Some("1502".into()),
            Some("ops-watch".into()),
            1,
        );
        assert_eq!(
            woke.words(None),
            "CI landed its branch ops-watch · build 1502 · the item is closed"
        );
        assert_eq!(woke.tone(), crate::woke::Tone::Passed);
        let two = crate::woke::Woke { closed: 2, ..woke };
        assert!(two.words(None).contains("· 2 items are closed"));
    }

    #[test]
    fn an_unknown_tools_post_is_said_as_it_was_sent() {
        let woke = crate::woke::Woke::webhook("backup job nightly-db\nfinished with warnings");
        assert_eq!(woke.tone(), crate::woke::Tone::Person);
        assert_eq!(
            woke.words(None),
            "A webhook from an unknown tool, as it was sent: “backup job nightly-db finished with warnings”"
        );
        let back = crate::woke::Woke::of(&woke.event()).unwrap();
        assert_eq!(back, woke);
    }

    #[test]
    fn a_card_names_the_tool_and_opens_it_there() {
        let item = OpenItem::opened(sentry(), 0);
        let held = card(&item, true);
        held.validate().unwrap();
        assert_eq!(held.title, "TypeError in web, reading 'slug' of undefined");
        assert_eq!(
            held.note.as_deref(),
            Some("Sentry · woke oncall · web/src/app/orgs/[slug]/page.tsx:41 · one open item")
        );
        assert_eq!(held.links[0].label, "Open in Sentry");
        let issue = OpenItem::opened(
            RoleReport {
                tool: Some(Tool::Github),
                reference: Some("#412".into()),
                summary: "Reconnect drops history on iPad after sleep".into(),
                details: Some("opened by a customer".into()),
                ..RoleReport::new("triage", Subject::Issue, "github:zircd/monorepo#412")
            },
            0,
        );
        let vacant = card(&issue, false);
        vacant.validate().unwrap();
        assert_eq!(
            vacant.note.as_deref(),
            Some(
                "GitHub issue #412 · vacant role · opened by a customer · waiting for whoever takes triage"
            )
        );
        assert!(vacant.links.is_empty());
        assert_eq!(vacant.fields[0].value, "`zirc role take triage`");
        // The card's event reads back as its report, and isn't a wake.
        let event = card_event(&issue, false);
        assert_eq!(RoleReport::of(&event), Some(issue.report));
    }

    #[test]
    fn a_tool_is_counted_by_the_day_and_a_test_apart() {
        let mut heard = HeardTool::new(Tool::Sentry);
        heard.heard(86_400 + 5, false);
        heard.heard(86_400 + 6, false);
        heard.heard(86_400 + 7, true);
        assert_eq!(
            (heard.today, heard.last, heard.last_test),
            (2, Some(86_406), Some(86_407))
        );
        heard.heard(2 * 86_400, false);
        assert_eq!(heard.today, 1);
    }

    #[test]
    fn connect_a_tool_says_what_it_heard_last_and_todays_count_only_today() {
        let mut heard = HeardTool::new(Tool::Sentry);
        heard.heard(86_400 + 5, false);
        heard.heard(86_400 + 6, false);
        let said = heard.as_of("oncall", 86_400 + 60).said.unwrap();
        assert_eq!(
            (said.lead.as_str(), said.at, said.tail.as_str()),
            ("✓ Heard from Sentry", 86_406, " · 2 today")
        );
        let tomorrow = heard.as_of("oncall", 2 * 86_400);
        assert_eq!(
            (tomorrow.today, tomorrow.said.unwrap().tail),
            (0, String::new())
        );
        heard.heard(86_400 + 70, true);
        let said = heard.as_of("oncall", 86_400 + 80).said.unwrap();
        assert_eq!(
            (said.lead.as_str(), said.tail.as_str()),
            ("✓ Heard a test from Sentry", ". oncall wasn't woken by it.")
        );
        assert_eq!(HeardTool::new(Tool::Linear).as_of("oncall", 0).said, None);
    }

    #[test]
    fn every_tool_says_itself_once() {
        for tool in Tool::ALL {
            assert_eq!(Tool::named(tool.as_str()), Some(tool));
        }
        assert_eq!(Tool::Pagerduty.source(Some("#3")), "PagerDuty incident #3");
        assert_eq!(Tool::Sentry.source(Some("WEB-7")), "Sentry");
    }
}
