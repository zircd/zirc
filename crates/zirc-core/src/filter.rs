//! jq filters over what `zirc watch` hears, through jaq: `zirc watch --filter` prints what
//! a filter makes of each line's JSON, so a script or agent wakes only for what it asks for.

use jaq_core::load::{self, Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, data, unwrap_valr};
use jaq_json::Val;

/// A compiled jq filter.
pub struct Filter {
    filter: jaq_core::Filter<data::JustLut<Val>>,
}

/// Why a filter couldn't be compiled or run.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The filter isn't jq jaq can read or compile.
    #[error("not a jq filter zirc can run: {0}")]
    Filter(String),
    /// Running it on a line failed, such as indexing a string.
    #[error("the filter failed: {0}")]
    Run(String),
}

impl Filter {
    /// Compiles `code`, jq with its standard library.
    ///
    /// # Errors
    ///
    /// [`Error::Filter`] when it isn't jq, or names something that isn't defined.
    pub fn new(code: &str) -> Result<Self, Error> {
        let defs = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs());
        let funs = jaq_core::funs()
            .chain(jaq_std::funs())
            .chain(jaq_json::funs());
        let arena = Arena::default();
        let modules = Loader::new(defs)
            .load(&arena, File { code, path: () })
            .map_err(|errs| {
                let problems = errs.into_iter().flat_map(|(_, err)| match err {
                    load::Error::Io(errs) => errs.into_iter().map(|(_, err)| err).collect(),
                    load::Error::Lex(errs) => errs
                        .into_iter()
                        .map(|(expected, rest)| expected_at(expected.as_str(), rest))
                        .collect(),
                    load::Error::Parse(errs) => errs
                        .into_iter()
                        .map(|(expected, rest)| expected_at(expected.as_str(), rest))
                        .collect::<Vec<_>>(),
                });
                Error::Filter(problems.collect::<Vec<_>>().join("; "))
            })?;
        let filter = Compiler::default()
            .with_funs(funs)
            .compile(modules)
            .map_err(|errs| {
                let problems = errs.into_iter().flat_map(|(_, errs)| {
                    errs.into_iter()
                        .map(|(name, undefined)| format!("no {} {name}", undefined.as_str()))
                });
                Error::Filter(problems.collect::<Vec<_>>().join("; "))
            })?;
        Ok(Self { filter })
    }

    /// The filter that picks a message saying `text`, whatever its case or the spaces around
    /// it: `zirc watch --said ping`.
    ///
    /// # Panics
    ///
    /// Never: `text` goes into the jq as a quoted JSON string, which always compiles.
    #[must_use]
    pub fn said(text: &str) -> Self {
        let wanted = serde_json::Value::String(text.trim().to_ascii_lowercase());
        Self::new(&format!(
            r#"select((.message.event.body? // "" | ascii_downcase | trim) == {wanted})"#
        ))
        .expect("a quoted string makes jq that compiles")
    }

    /// What the filter makes of `input`, each output as one line of JSON: none when it
    /// selects nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] when the filter fails on it.
    pub fn run(&self, input: &serde_json::Value) -> Result<Vec<String>, Error> {
        let input = jaq_json::read::parse_single(input.to_string().as_bytes())
            .map_err(|err| Error::Run(err.to_string()))?;
        let ctx = Ctx::<data::JustLut<Val>>::new(&self.filter.lut, Vars::new([]));
        self.filter
            .id
            .run((ctx, input))
            .map(unwrap_valr)
            .map(|out| {
                out.map(|val| val.to_string())
                    .map_err(|err| Error::Run(err.to_string()))
            })
            .collect()
    }
}

/// "expected a term at `)`", where `rest` is what's left of the filter where it went wrong.
fn expected_at(expected: &str, rest: &str) -> String {
    match rest.chars().take(20).collect::<String>() {
        rest if rest.is_empty() => format!("expected {expected} at the end"),
        rest => format!("expected {expected} at `{rest}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deploy(env: &str) -> serde_json::Value {
        json!({
            "channel": "#api",
            "from": "deploy-bot",
            "message": {"v": 1, "session": "ci", "event": {
                "type": "custom", "kind": "x-acme/deploy",
                "data": {"env": env}, "text": format!("deployed to {env}"),
            }},
            "why": null,
        })
    }

    #[test]
    fn selects_what_it_asks_for() {
        let filter = Filter::new(
            r#"select(.message.event.kind == "x-acme/deploy" and .message.event.data.env == "prod")"#,
        )
        .unwrap();
        assert_eq!(filter.run(&deploy("prod")).unwrap().len(), 1);
        assert!(filter.run(&deploy("staging")).unwrap().is_empty());
    }

    #[test]
    fn prints_what_it_makes_of_a_line_as_json() {
        let filter = Filter::new(".message.event.text, .message.event.data").unwrap();
        assert_eq!(
            filter.run(&deploy("prod")).unwrap(),
            [r#""deployed to prod""#, r#"{"env":"prod"}"#]
        );
    }

    #[test]
    fn said_picks_a_message_saying_it_in_any_case() {
        let says = |body: &str| {
            json!({"channel": "#web/guest-1a2b", "from": "guest-1a2b",
                   "message": {"v": 1, "session": "", "event": {"type": "text", "body": body}}})
        };
        let ping = Filter::said("ping");
        assert_eq!(ping.run(&says(" Ping ")).unwrap().len(), 1);
        assert!(ping.run(&says("ping pong")).unwrap().is_empty());
        assert!(
            ping.run(&deploy("prod")).unwrap().is_empty(),
            "no body, no match"
        );
        let quoted = Filter::said(r#"say "hi" \ there"#);
        assert_eq!(quoted.run(&says(r#"say "hi" \ there"#)).unwrap().len(), 1);
    }

    #[test]
    fn says_what_isnt_jq_or_fails() {
        assert_eq!(
            Filter::new("select(").err().unwrap().to_string(),
            "not a jq filter zirc can run: expected closing parenthesis at the end"
        );
        assert_eq!(
            Filter::new(". | nonesuch").err().unwrap().to_string(),
            "not a jq filter zirc can run: no filter nonesuch"
        );
        let filter = Filter::new(".channel[0]").unwrap();
        assert!(matches!(filter.run(&deploy("prod")), Err(Error::Run(_))));
    }
}
