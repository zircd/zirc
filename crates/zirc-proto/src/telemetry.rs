//! The analytics events zirc's apps and servers send to `PostHog`, declared once. The
//! TypeScript types the website sends them with are generated from these (`bazel run
//! //crates/zirc-core:generated_types`, into `generated/telemetry`), so a client can
//! only send what's declared here, and adding a property is a visible change to this file:
//! it's also the privacy review.
//!
//! Nothing here may carry what people say or who or where they are: no message text, nicks,
//! account, channel or repository names, paths, hostnames or IP addresses. Organisations
//! and accounts appear only by their opaque ids. (`PostHog` itself sees the address events
//! are sent from.)
//!
//! An event serializes as `PostHog` takes it, `{"event": "command_run", "properties": {…}}`.
//! The apps add their [`Context`] to every event's properties.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which of zirc's programs sent an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "TelemetryClient")]
#[serde(rename_all = "snake_case")]
pub enum Client {
    /// The terminal client, `zirc tui`.
    Tui,
    /// The `zirc` command line.
    Cli,
    /// The per-machine daemon.
    Zircd,
    /// The desktop app.
    Desktop,
    /// The Android app.
    Android,
    /// The iOS app.
    Ios,
    /// The website and web client.
    Web,
}

/// The operating system an app runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "TelemetryOs")]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// macOS.
    Macos,
    /// Linux.
    Linux,
    /// Windows.
    Windows,
    /// Android.
    Android,
    /// iOS.
    Ios,
    /// Anything else.
    Other,
}

impl Os {
    /// The one this program was built for.
    #[must_use]
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Self::Macos,
            "linux" => Self::Linux,
            "windows" => Self::Windows,
            "android" => Self::Android,
            "ios" => Self::Ios,
            _ => Self::Other,
        }
    }
}

/// What every app event says about the app that sent it, among its properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "TelemetryContext")]
pub struct Context {
    /// The app.
    pub client: Client,
    /// Its version, such as `0.1.14`.
    pub version: String,
    /// The operating system it runs on.
    pub os: Os,
    /// Whether it's a development build, a test's or a developer's, rather than a release:
    /// the dashboards leave these out.
    pub dev: bool,
    /// Whether it's an install of zirc's own CI, tests or team, marked so with
    /// `ZIRC_TELEMETRY_INTERNAL=1`: the dashboards leave these out too.
    #[serde(default)]
    pub internal: bool,
}

/// An analytics event. App events are sent with the install's random id as `distinct_id`,
/// or the account's opaque id once the install is linked to it; server events say which id
/// they're sent with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "TelemetryEvent")]
#[serde(tag = "event", content = "properties", rename_all = "snake_case")]
pub enum Event {
    /// An app started: the TUI, the desktop or a phone app opened, or zircd started.
    AppOpened(AppOpened),
    /// A `zirc` command ran.
    CommandRun(CommandRun),
    /// Someone used one of the apps' features.
    FeatureUsed(FeatureUsed),
    /// Connecting to the server failed, the first time since it last worked.
    ConnectionFailed(ConnectionFailed),
    /// A newer release was found.
    UpdateOffered(UpdateOffered),
    /// A newer release was installed.
    UpdateInstalled(UpdateInstalled),
    /// The controller's daily summary of an organisation, sent with `organisation:<its id>`
    /// as `distinct_id`, the organisation as its group, and no person profile.
    OrgDaily(OrgDaily),
    /// Someone signed up on zirc.dev, sent by the server with the new account's opaque id as
    /// `distinct_id`.
    SignedUp(SignedUp),
    /// An organisation was created, sent by zirc.dev's server with its creator's account's
    /// opaque id as `distinct_id`, and the organisation as its group, whose properties say how
    /// its creator first came to zirc.dev: the referring site's domain and the campaign tags.
    OrganisationCreated(OrganisationCreated),
    /// An organisation's trial started, sent by zirc.dev's server as
    /// [`Event::OrganisationCreated`] is.
    TrialStarted(TrialStarted),
    /// The first message was posted in an organisation, sent by the controller as
    /// [`Event::OrgDaily`] is, at the time it was posted.
    FirstMessage(FirstMessage),
    /// Someone was invited to an organisation, sent by zirc.dev's server with the inviter's
    /// account's opaque id as `distinct_id`, and the organisation as its group.
    InviteSent(InviteSent),
    /// Someone joined an organisation by accepting an invite, sent by zirc.dev's server as
    /// [`Event::InviteSent`] is, for the account that joined.
    MemberJoined(MemberJoined),
    /// An agent said what it's about to change, with `zirc intend`: sent by zircd, with its
    /// organisation as its group.
    IntentPosted(IntentPosted),
    /// zircd told an agent that someone else is changing the files it's changing.
    OverlapNotified(OverlapNotified),
    /// zircd told an agent that someone's push changes the same lines it's changing.
    ConflictAheadNotified(ConflictAheadNotified),
    /// An organisation's owner opened Stripe's checkout to subscribe, sent by zirc.dev's
    /// server as [`Event::InviteSent`] is.
    CheckoutOpened(Billing),
    /// An organisation subscribed, sent from Stripe's webhook by zirc.dev's server with
    /// `organisation:<its id>` as `distinct_id`, the organisation as its group.
    SubscriptionStarted(Billing),
    /// An organisation's subscription ended, sent as [`Event::SubscriptionStarted`] is.
    SubscriptionCancelled(Billing),
}

/// [`Event::AppOpened`]: the app is in the [`Context`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct AppOpened {}

/// [`Event::CommandRun`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct CommandRun {
    /// The command's name only, such as `watch` or `repo add`: never its arguments.
    pub command: String,
    /// Whether it ran with `--session`, as an agent's commands do.
    pub agent: bool,
}

/// A feature of the apps, for [`Event::FeatureUsed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// A direct message, to one person or a group.
    Dm,
    /// A reaction to a message.
    Reaction,
    /// A file shared in a conversation.
    File,
    /// A search of the conversations.
    Search,
    /// A direct message with an agent session.
    AgentDm,
    /// Joining a channel or topic.
    TopicJoin,
}

/// [`Event::FeatureUsed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct FeatureUsed {
    /// Which.
    pub feature: Feature,
}

/// Why connecting failed, as a class rather than the error's text, which can name hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// The network: no route, refused, reset or a name that doesn't resolve.
    Network,
    /// TLS couldn't be set up.
    Tls,
    /// It took too long.
    Timeout,
    /// The server closed the connection while registering.
    Closed,
    /// The server refused the server password.
    Password,
    /// The server refused the account.
    Sasl,
    /// No acceptable nick was free.
    Nick,
    /// Anything else.
    Other,
}

/// [`Event::ConnectionFailed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct ConnectionFailed {
    /// Why.
    pub reason: Failure,
}

/// [`Event::UpdateOffered`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct UpdateOffered {
    /// The newer release's version.
    pub to: String,
}

/// [`Event::UpdateInstalled`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct UpdateInstalled {
    /// The version it replaced.
    pub from: String,
    /// The version installed.
    pub to: String,
}

/// Where an organisation's server runs, for [`Event::OrgDaily`]: the dashboards count
/// production's only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    /// zirc.dev's own.
    Production,
    /// One that doesn't say.
    None,
}

/// [`Event::OrgDaily`]: one organisation's day, counted by the controller from its server.
/// People and agents are counted apart throughout: an agent's account is its machine's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct OrgDaily {
    /// The organisation's opaque id, never its name.
    pub organisation: String,
    /// The day counted, `YYYY-MM-DD` in UTC.
    pub day: String,
    /// Its plan: `trial`, `paid`, `internal` or `none`.
    pub plan: String,
    /// Where its server runs.
    pub environment: Environment,
    /// People's accounts.
    pub accounts_people: u32,
    /// Machines' accounts, whose sessions are agents.
    pub accounts_agents: u32,
    /// People who were active that day: posted, or had a client connected.
    pub active_day_people: u32,
    /// Agents' machines active that day.
    pub active_day_agents: u32,
    /// People active in the week to that day.
    pub active_week_people: u32,
    /// Agents' machines active in that week.
    pub active_week_agents: u32,
    /// People connected when it was counted.
    pub connected_people: u32,
    /// Agents' machines connected then.
    pub connected_agents: u32,
    /// People's connected sessions then.
    pub sessions_people: u32,
    /// Agents' connected sessions then.
    pub sessions_agents: u32,
    /// Channel messages people posted that day.
    pub messages_day_people: u32,
    /// Channel messages agents posted that day.
    pub messages_day_agents: u32,
}

/// [`Event::SignedUp`]: who is in its `distinct_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct SignedUp {}

/// [`Event::OrganisationCreated`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct OrganisationCreated {
    /// The organisation's opaque id.
    pub organisation_id: String,
}

/// [`Event::TrialStarted`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct TrialStarted {
    /// The organisation's opaque id.
    pub organisation_id: String,
    /// How many days the trial lasts.
    pub trial_days: u32,
}

/// [`Event::FirstMessage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct FirstMessage {
    /// The organisation's opaque id.
    pub organisation_id: String,
    /// Whether an agent posted it rather than a person.
    pub by_agent: bool,
}

/// [`Event::InviteSent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct InviteSent {
    /// The organisation's opaque id.
    pub organisation_id: String,
}

/// [`Event::MemberJoined`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct MemberJoined {
    /// The organisation's opaque id.
    pub organisation_id: String,
}

/// [`Event::IntentPosted`]: counted only, never its paths or summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct IntentPosted {}

/// [`Event::OverlapNotified`]: counted only, never the paths or who.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct OverlapNotified {
    /// Whether it's between two worktrees on this machine, rather than with another machine.
    pub local: bool,
    /// What the overlap amounts to: the worst of its paths.
    pub kind: OverlapKind,
    /// Whether it woke the sessions it concerns. One that changes different parts of the
    /// files isn't news to a session, so it doesn't.
    pub woke: bool,
    /// Whether it follows an earlier one that said the two changed different parts of a
    /// file, and now they change the same lines: an overlap that turned into a clash.
    pub escalated: bool,
}

/// What an overlap amounts to, as far as zircd can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "TelemetryOverlapKind")]
#[serde(rename_all = "snake_case")]
pub enum OverlapKind {
    /// Both change the same or neighbouring lines of a file: likely to conflict.
    SameLines,
    /// Both change files, but in different parts: likely to merge.
    Apart,
    /// Both change a file, and which parts isn't known, as with another machine.
    Path,
    /// Only a lockfile, which the second to land regenerates.
    Lockfile,
}

/// [`Event::ConflictAheadNotified`]: counted only, never the paths, lines or branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct ConflictAheadNotified {}

/// [`Event::CheckoutOpened`], [`Event::SubscriptionStarted`] and
/// [`Event::SubscriptionCancelled`]: an organisation's subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct Billing {
    /// The organisation's opaque id.
    pub organisation_id: String,
    /// What it subscribes to, by the plan's name on zirc.dev, such as `team`.
    pub plan: String,
    /// Its people, whom a plan priced by the seat charges for.
    pub seats: u32,
}

impl Event {
    /// Its name, as `PostHog` gets it, such as `command_run`.
    ///
    /// # Panics
    ///
    /// Never: every event serializes with its name.
    #[must_use]
    pub fn name(&self) -> String {
        let value = serde_json::to_value(self).expect("events serialize");
        value["event"]
            .as_str()
            .expect("events have a name")
            .to_owned()
    }

    /// Its properties, as `PostHog` gets them.
    ///
    /// # Panics
    ///
    /// Never: every event's properties serialize to an object.
    #[must_use]
    pub fn properties(&self) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::to_value(self).expect("events serialize") {
            serde_json::Value::Object(mut event) => match event.remove("properties") {
                Some(serde_json::Value::Object(properties)) => properties,
                _ => serde_json::Map::new(),
            },
            _ => unreachable!("events serialize to objects"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_posthog_takes_events() {
        let event = Event::CommandRun(CommandRun {
            command: "repo add".into(),
            agent: true,
        });
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "event": "command_run",
                "properties": {"command": "repo add", "agent": true},
            })
        );
        assert_eq!(event.name(), "command_run");
        assert_eq!(event.properties()["command"], "repo add");
        let feature = Event::FeatureUsed(FeatureUsed {
            feature: Feature::AgentDm,
        });
        assert_eq!(feature.properties()["feature"], "agent_dm");
        assert!(Event::AppOpened(AppOpened {}).properties().is_empty());
    }
}
