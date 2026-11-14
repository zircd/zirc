//! Files generated from the Rust types so they can't drift, checked in under `oss/generated`:
//! a JSON Schema of the socket protocol and the envelope that any language can use, and the
//! analytics events' types for the website and anything else ([`telemetry`]).
//! `bazel run //crates/zirc-core:generated_types` writes them; a test fails when what's
//! checked in isn't what this makes.

use schemars::{JsonSchema, SchemaGenerator};
use zirc_proto::Message;

use crate::ipc::{PROTOCOL, Request, Response};

pub mod telemetry;

/// Each generated file, by its path under `oss/generated/`, with what it should hold.
#[must_use]
pub fn files() -> Vec<(&'static str, String)> {
    std::iter::once(("schema.json", schema()))
        .chain(telemetry::files())
        .collect()
}

const GENERATED: &str = "Generated from zirc-proto and zirc-core by \
                         `bazel run //crates/zirc-core:generated_types`. Don't edit: change \
                         the Rust types and generate this again.";

/// The JSON Schema of the socket protocol and the envelope: every type under `$defs`, and
/// what a line on the socket or a message's payload can be at the top.
#[must_use]
pub fn schema() -> String {
    let mut generator = SchemaGenerator::default();
    let roots = [
        ("Request", Request::json_schema(&mut generator)),
        ("Response", Response::json_schema(&mut generator)),
        ("Message", Message::json_schema(&mut generator)),
        ("Card", zirc_proto::card::Card::json_schema(&mut generator)),
        ("CiData", zirc_proto::ci::Data::json_schema(&mut generator)),
    ];
    let mut defs = generator.take_definitions(true);
    for (name, schema) in roots {
        defs.insert(name.to_owned(), schema.to_value());
    }
    for (name, def) in &mut defs {
        title_variants(name, def);
    }
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "zirc",
        "description": GENERATED,
        "x-protocol": PROTOCOL,
        "x-version": zirc_proto::VERSION,
        "anyOf": [
            {"$ref": "#/$defs/Request"},
            {"$ref": "#/$defs/Response"},
            {"$ref": "#/$defs/Message"},
        ],
        "$defs": defs,
    });
    format!("{schema:#}\n")
}

/// Names each variant of a tagged union after its tag and the union, such as
/// `CustomEvent` for `{"type": "custom", …}` in `Event`, so the types generated from the
/// schema are named for what they are rather than numbered.
fn title_variants(union: &str, def: &mut serde_json::Value) {
    let Some(variants) = def
        .get_mut("oneOf")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for variant in variants {
        let tag = variant
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .and_then(|properties| {
                properties
                    .values()
                    .find_map(|property| property.get("const")?.as_str())
            })
            .map(pascal_case);
        if let (Some(tag), Some(variant)) = (tag, variant.as_object_mut()) {
            variant.insert("title".to_owned(), format!("{tag}{union}").into());
        }
    }
}

/// `repo-add` as `RepoAdd`.
fn pascal_case(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect()
}
