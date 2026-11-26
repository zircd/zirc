//! Checks role definitions by the rules a server keeps ([`zirc_proto::roles::Definition::check`]),
//! for a reader that can't run this crate: a JSON array of definitions on stdin, and on stdout a
//! JSON array saying of each what's wrong (`problem`, `null` for none), the picture a server
//! draws it with ([`zirc_proto::roles::Look::picture_of`]) and what it wakes on, in words
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
        .map(
            |value| match serde_json::from_value::<Definition>(value.clone()) {
                Ok(definition) => json!({
                    "problem": unread(&value, &definition).or_else(|| definition.check().err()),
                    "picture": definition.look().picture_of(&definition.name),
                    "wakes": definition.wakes_on(),
                }),
                Err(why) => json!({ "problem": why.to_string(), "picture": null }),
            },
        )
        .collect();
    print!("{}", Value::Array(found));
}

/// What a server would leave out of `value` as it reads it ([`zirc_proto::lenient`]): a trigger
/// or a kind of report it can't read, which a definition zirc.dev offers never has.
fn unread(value: &Value, definition: &Definition) -> Option<String> {
    let said = |field: &str| {
        value
            .get(field)
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    if said("triggers") != definition.triggers.len() {
        return Some("a trigger a server can't read".to_owned());
    }
    (said("reports") != definition.reports.len())
        .then(|| "a kind of report a server doesn't know".to_owned())
}
