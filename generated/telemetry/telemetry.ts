// Generated from zirc-proto and zirc-core by `bazel run //crates/zirc-core:generated_types`. Don't edit: change the Rust types and generate this again.

// zirc's analytics events: send one with posthog.capture(event.event, event.properties),
// adding a TelemetryContext's fields to the properties of an app's events.

export type TelemetryEvent = { "event": "app_opened", "properties": AppOpened } | { "event": "command_run", "properties": CommandRun } | { "event": "feature_used", "properties": FeatureUsed } | { "event": "connection_failed", "properties": ConnectionFailed } | { "event": "update_offered", "properties": UpdateOffered } | { "event": "update_installed", "properties": UpdateInstalled } | { "event": "org_daily", "properties": OrgDaily } | { "event": "signed_up", "properties": SignedUp } | { "event": "organisation_created", "properties": OrganisationCreated } | { "event": "trial_started", "properties": TrialStarted } | { "event": "first_message", "properties": FirstMessage } | { "event": "invite_sent", "properties": InviteSent } | { "event": "member_joined", "properties": MemberJoined } | { "event": "intent_posted", "properties": IntentPosted } | { "event": "overlap_notified", "properties": OverlapNotified } | { "event": "conflict_ahead_notified", "properties": ConflictAheadNotified } | { "event": "checkout_opened", "properties": Billing } | { "event": "subscription_started", "properties": Billing } | { "event": "subscription_cancelled", "properties": Billing };

export type TelemetryContext = { 
/**
 * The app.
 */
client: TelemetryClient, 
/**
 * Its version, such as `0.1.14`.
 */
version: string, 
/**
 * The operating system it runs on.
 */
os: TelemetryOs, 
/**
 * Whether it's a development build, a test's or a developer's, rather than a release:
 * the dashboards leave these out.
 */
dev: boolean, 
/**
 * Whether it's an install of zirc's own CI, tests or team, marked so with
 * `ZIRC_TELEMETRY_INTERNAL=1`: the dashboards leave these out too.
 */
internal: boolean, };

export type TelemetryClient = "tui" | "cli" | "zircd" | "desktop" | "android" | "ios" | "web";

export type TelemetryOs = "macos" | "linux" | "windows" | "android" | "ios" | "other";

export type AppOpened = Record<symbol, never>;

export type CommandRun = { 
/**
 * The command's name only, such as `watch` or `repo add`: never its arguments.
 */
command: string, 
/**
 * Whether it ran with `--session`, as an agent's commands do.
 */
agent: boolean, };

export type FeatureUsed = { 
/**
 * Which.
 */
feature: Feature, };

export type Feature = "dm" | "reaction" | "file" | "search" | "agent_dm" | "topic_join";

export type ConnectionFailed = { 
/**
 * Why.
 */
reason: Failure, };

export type Failure = "network" | "tls" | "timeout" | "closed" | "password" | "sasl" | "nick" | "other";

export type UpdateOffered = { 
/**
 * The newer release's version.
 */
to: string, };

export type UpdateInstalled = { 
/**
 * The version it replaced.
 */
from: string, 
/**
 * The version installed.
 */
to: string, };

export type OrgDaily = { 
/**
 * The organisation's opaque id, never its name.
 */
organisation: string, 
/**
 * The day counted, `YYYY-MM-DD` in UTC.
 */
day: string, 
/**
 * Its plan: `trial`, `paid`, `internal` or `none`.
 */
plan: string, 
/**
 * Where its server runs.
 */
environment: Environment, 
/**
 * People's accounts.
 */
accounts_people: number, 
/**
 * Machines' accounts, whose sessions are agents.
 */
accounts_agents: number, 
/**
 * People who were active that day: posted, or had a client connected.
 */
active_day_people: number, 
/**
 * Agents' machines active that day.
 */
active_day_agents: number, 
/**
 * People active in the week to that day.
 */
active_week_people: number, 
/**
 * Agents' machines active in that week.
 */
active_week_agents: number, 
/**
 * People connected when it was counted.
 */
connected_people: number, 
/**
 * Agents' machines connected then.
 */
connected_agents: number, 
/**
 * People's connected sessions then.
 */
sessions_people: number, 
/**
 * Agents' connected sessions then.
 */
sessions_agents: number, 
/**
 * Channel messages people posted that day.
 */
messages_day_people: number, 
/**
 * Channel messages agents posted that day.
 */
messages_day_agents: number, };

export type Environment = "production" | "none";

export type SignedUp = Record<symbol, never>;

export type OrganisationCreated = { 
/**
 * The organisation's opaque id.
 */
organisation_id: string, };

export type TrialStarted = { 
/**
 * The organisation's opaque id.
 */
organisation_id: string, 
/**
 * How many days the trial lasts.
 */
trial_days: number, };

export type FirstMessage = { 
/**
 * The organisation's opaque id.
 */
organisation_id: string, 
/**
 * Whether an agent posted it rather than a person.
 */
by_agent: boolean, };

export type InviteSent = { 
/**
 * The organisation's opaque id.
 */
organisation_id: string, };

export type MemberJoined = { 
/**
 * The organisation's opaque id.
 */
organisation_id: string, };

export type IntentPosted = Record<symbol, never>;

export type TelemetryOverlapKind = "same_lines" | "apart" | "path" | "lockfile";

export type OverlapNotified = { 
/**
 * Whether it's between two worktrees on this machine, rather than with another machine.
 */
local: boolean, 
/**
 * What the overlap amounts to: the worst of its paths.
 */
kind: TelemetryOverlapKind, 
/**
 * Whether it woke the sessions it concerns. One that changes different parts of the
 * files isn't news to a session, so it doesn't.
 */
woke: boolean, 
/**
 * Whether it follows an earlier one that said the two changed different parts of a
 * file, and now they change the same lines: an overlap that turned into a clash.
 */
escalated: boolean, };

export type ConflictAheadNotified = Record<symbol, never>;

export type Billing = { 
/**
 * The organisation's opaque id.
 */
organisation_id: string, 
/**
 * What it subscribes to, by the plan's name on zirc.dev, such as `team`.
 */
plan: string, 
/**
 * Its people, whom a plan priced by the seat charges for.
 */
seats: number, };
