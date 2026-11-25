//! Checks role definitions by the rules a server keeps ([`zirc_proto::roles::Definition::check`]),
//! for a reader that can't run this crate: a JSON array of definitions on stdin, and on stdout a
//! JSON array saying of each what's wrong (`problem`, `null` for none), the picture a server
//! draws it with ([`zirc_proto::agent_picture`]) and what it wakes on, in words
//! ([`Definition::wakes_on`]). zirc.dev's ready-made roles' test checks every answer's definition
//! with it, and its trigger words' test that it says each trigger as the server does.

use std::io::Read as _;

use serde_json::{Value, json};
use zirc_proto::roles::Definition;

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("definitions on stdin");
    let definitions: Vec<Value> =
        serde_json::from_str(&input).expect("a JSON array of definitions");
    let found: Vec<Value> = definitions
        .into_iter()
        .map(|value| match serde_json::from_value::<Definition>(value) {
            Ok(definition) => json!({
                "problem": definition.check().err(),
                "picture": zirc_proto::agent_picture(Some(&definition.name), Some(&definition.name)),
                "wakes": definition.wakes_on(),
            }),
            Err(why) => json!({ "problem": why.to_string(), "picture": null }),
        })
        .collect();
    print!("{}", Value::Array(found));
}
