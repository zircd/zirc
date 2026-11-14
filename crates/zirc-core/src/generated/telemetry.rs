//! The analytics events' generated types, from [`zirc_proto::telemetry`]: TypeScript for the
//! website, and a JSON Schema for anything else, such as the controller. Each gets a type per
//! event with its name and its properties as `PostHog`'s SDKs take them.

use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value};
use ts_rs::{Config, TS};
use zirc_proto::telemetry::{
    AppOpened, Billing, Client, CommandRun, ConflictAheadNotified, ConnectionFailed, Context,
    Environment, Event, Failure, Feature, FeatureUsed, FirstMessage, IntentPosted, InviteSent,
    MemberJoined, OrgDaily, OrganisationCreated, Os, OverlapKind, OverlapNotified, SignedUp,
    TrialStarted, UpdateInstalled, UpdateOffered,
};

use super::GENERATED;

/// Each generated file, by its path under `oss/generated/`, with what it should hold.
#[must_use]
pub fn files() -> Vec<(&'static str, String)> {
    vec![
        ("telemetry/telemetry.ts", typescript()),
        ("telemetry/schema.json", schema()),
    ]
}

/// The TypeScript declarations of the events.
#[must_use]
pub fn typescript() -> String {
    let config = Config::new().with_large_int("number");
    let mut out = format!(
        "// {GENERATED}\n\n\
         // zirc's analytics events: send one with posthog.capture(event.event, event.properties),\n\
         // adding a TelemetryContext's fields to the properties of an app's events.\n"
    );
    for decl in [
        Event::decl(&config),
        Context::decl(&config),
        Client::decl(&config),
        Os::decl(&config),
        AppOpened::decl(&config),
        CommandRun::decl(&config),
        FeatureUsed::decl(&config),
        Feature::decl(&config),
        ConnectionFailed::decl(&config),
        Failure::decl(&config),
        UpdateOffered::decl(&config),
        UpdateInstalled::decl(&config),
        OrgDaily::decl(&config),
        Environment::decl(&config),
        SignedUp::decl(&config),
        OrganisationCreated::decl(&config),
        TrialStarted::decl(&config),
        FirstMessage::decl(&config),
        InviteSent::decl(&config),
        MemberJoined::decl(&config),
        IntentPosted::decl(&config),
        OverlapKind::decl(&config),
        OverlapNotified::decl(&config),
        ConflictAheadNotified::decl(&config),
        Billing::decl(&config),
    ] {
        out.push_str("\nexport ");
        out.push_str(&decl);
        out.push('\n');
    }
    out
}

/// The events' schema and the context's, under `$defs`, with the events at the top.
fn definitions() -> (Value, Map<String, Value>) {
    let mut generator = SchemaGenerator::default();
    let event = Event::json_schema(&mut generator).to_value();
    let context = Context::json_schema(&mut generator).to_value();
    let mut defs: Map<String, Value> = generator.take_definitions(true).into_iter().collect();
    defs.insert("Context".to_owned(), context);
    (event, defs)
}

/// The JSON Schema of the events.
#[must_use]
pub fn schema() -> String {
    let (event, mut defs) = definitions();
    defs.insert("Event".to_owned(), event);
    for (name, def) in &mut defs {
        super::title_variants(name, def);
    }
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "zirc analytics events",
        "description": GENERATED,
        "$ref": "#/$defs/Event",
        "$defs": defs,
    });
    format!("{schema:#}\n")
}
