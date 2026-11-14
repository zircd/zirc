//! zirc's events as the IRC lines that say them, for tests in other languages: a harness
//! there keeps a plain connection (send a line, wait for a line) and asks here, through
//! `serve`'s standard input, for the lines to send. zirc-proto and zirc-line stay the only
//! places that write zirc's framing and its multiline batches.
//!
//! One JSON object on a line asks, and one answers it:
//!
//! ```text
//! {"id":1,"stage":"event","target":"#web","session":"cc-1","event":{"type":"text","body":"hi"},"more":{"name":"checkout"}}
//! {"id":1,"lines":["PRIVMSG #web :\u0001ACTION ZIRC eyJ2Ijox… :hi\u0001"]}
//! ```
//!
//! `event` is a [`zirc_proto::Event`] as JSON and `more` any of a [`zirc_proto::Message`]'s
//! other fields, such as its session's `name`. They're read as those types, so what
//! zirc-proto can't say is an `{"id":1,"error":"…"}` naming what was wrong, a field it
//! hasn't included: nothing is sent that zircd couldn't have sent. `id` comes back as it
//! was given, for a harness with several asking at once.
//!
//! The lines are the ones zircd sends for the message, without a source: a `TAGMSG` with
//! the event in its tag for what's only true now ([`Message::is_live`], such as a working
//! set), one `PRIVMSG` for what fits a line, and a `draft/multiline` batch for what
//! doesn't, as long as the server takes.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use zirc_line::Line;
use zirc_line::multiline::{self, Limits};
use zirc_line::registration::{Identity, Registration, Step};
use zirc_proto::{Event, Message};

/// How long the server gets to say what it takes.
const WITHIN: Duration = Duration::from_secs(15);

/// Whom [`limits`] registers as, for a moment.
const NICK: &str = "zirc-stager";

/// What a harness asks, besides the `id` its answer gives back: see the [module](self).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ask {
    stage: What,
    /// The channel or nick the lines go to.
    target: String,
    session: String,
    event: Value,
    #[serde(default)]
    more: Map<String, Value>,
}

/// What can be staged.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum What {
    /// A zirc event, as zircd sends it.
    Event,
}

/// Where a multiline message's limits come from.
enum Multiline {
    /// The server on this port, asked when a message first needs them.
    Server(u16),
    Known(Limits),
}

/// Answers what a harness asks, a line at a time.
pub struct Stager {
    multiline: Multiline,
    /// How many batches it has named, each by its number.
    batches: u64,
}

impl Stager {
    /// One for the server on `port` of 127.0.0.1, whose multiline limits it asks for when
    /// an event is first too long for a line.
    #[must_use]
    pub fn for_server(port: u16) -> Self {
        Self {
            multiline: Multiline::Server(port),
            batches: 0,
        }
    }

    /// One for a server that takes multiline messages up to `limits`.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            multiline: Multiline::Known(limits),
            batches: 0,
        }
    }

    /// The answer to `ask`, a line of JSON: its `id` with the `lines` to send, or with the
    /// `error` that says what was wrong with it.
    pub fn answer(&mut self, ask: &str) -> String {
        let (id, lines) = match serde_json::from_str::<Value>(ask) {
            // Its id first, so an answer has it whatever is wrong with the rest.
            Ok(mut asked) => {
                let id = asked.as_object_mut().and_then(|asked| asked.remove("id"));
                (id.unwrap_or_default(), self.lines(asked))
            }
            Err(error) => (Value::Null, Err(format!("not JSON: {error}"))),
        };
        match lines {
            Ok(lines) => {
                let lines: Vec<String> = lines.iter().map(ToString::to_string).collect();
                json!({ "id": id, "lines": lines })
            }
            Err(error) => json!({ "id": id, "error": error }),
        }
        .to_string()
    }

    /// The lines that say `event`, a [`zirc_proto::Event`] as JSON, from `session` to
    /// `target`: what [`Self::answer`] answers, for a caller in Rust ([`crate::scenario`]).
    ///
    /// # Errors
    ///
    /// What [`Self::answer`] says as an `error`.
    pub fn staged(
        &mut self,
        target: &str,
        session: &str,
        event: &Value,
    ) -> Result<Vec<Line>, String> {
        let message = message(session, event, &Map::new())?;
        self.event(target, &message)
    }

    fn lines(&mut self, asked: Value) -> Result<Vec<Line>, String> {
        let ask: Ask = serde_json::from_value(asked).map_err(|error| error.to_string())?;
        if ask.target.is_empty() || ask.target.contains([' ', ',', '\r', '\n']) {
            return Err(format!(
                "target: {:?} isn't one channel or nick",
                ask.target
            ));
        }
        let message = message(&ask.session, &ask.event, &ask.more)?;
        match ask.stage {
            What::Event => self.event(&ask.target, &message),
        }
    }

    /// The lines that send `message` to `target`, as zirc-core's `send_message` sends one.
    ///
    /// A shorter copy of that routing (`zirc_core::send::send_message` and `bodies_of`),
    /// kept because this crate is linked into every crate's tests and zirc-core is not to
    /// be: change the two together. It differs where a test needn't follow: a post of
    /// several lines that fits one line goes on one, and an event too long even for a
    /// multiline message is an error here, where zirc-core cuts it to fit.
    fn event(&mut self, target: &str, message: &Message) -> Result<Vec<Line>, String> {
        if message.is_live() {
            let payload = zirc_proto::payload(message).map_err(|error| error.to_string())?;
            let mut line = Line::new("TAGMSG", [target]);
            line.tags = vec![(zirc_proto::EVENT_TAG.to_owned(), Some(payload))];
            return Ok(vec![line]);
        }
        match zirc_proto::encode(message) {
            Ok(body) => Ok(vec![Line::new("PRIVMSG", [target, body.as_str()])]),
            Err(zirc_proto::Error::TooLong(_)) => {
                let limits = self.limits()?;
                let body = zirc_proto::encode_up_to(message, limits.max_bytes)
                    .map_err(|error| format!("event: {error}, even as a multiline message"))?;
                self.batches += 1;
                multiline::batch(&format!("staged{}", self.batches), target, &body, limits)
                    .ok_or_else(|| {
                        format!(
                            "event: more lines than the server's {} to a multiline message",
                            limits.max_lines
                        )
                    })
            }
            Err(error) => Err(format!("event: {error}")),
        }
    }

    fn limits(&mut self) -> Result<Limits, String> {
        match self.multiline {
            Multiline::Known(limits) => Ok(limits),
            Multiline::Server(port) => {
                let limits = limits(port)?;
                self.multiline = Multiline::Known(limits);
                Ok(limits)
            }
        }
    }
}

/// `session`'s `event` with `more` of a message's fields, as the [`Message`] they say.
///
/// # Errors
///
/// What zirc-proto doesn't read them as, or a field it would leave out.
fn message(session: &str, event: &Value, more: &Map<String, Value>) -> Result<Message, String> {
    let typed = Event::deserialize(event).map_err(|error| format!("event: {error}"))?;
    let kept = serde_json::to_value(&typed).map_err(|error| error.to_string())?;
    if let Some(field) = left_out(event, &kept, "event") {
        return Err(format!("{field}: not a field zirc-proto has there"));
    }
    if let Some(own) = ["v", "session", "event"]
        .into_iter()
        .find(|own| more.contains_key(*own))
    {
        return Err(format!(
            "more.{own}: not for more, it has a place of its own"
        ));
    }
    let mut given = more.clone();
    given.insert("v".to_owned(), zirc_proto::VERSION.into());
    given.insert("session".to_owned(), session.into());
    given.insert("event".to_owned(), event.clone());
    let message: Message =
        serde_json::from_value(Value::Object(given)).map_err(|error| format!("more: {error}"))?;
    let kept = serde_json::to_value(&message).map_err(|error| error.to_string())?;
    if let Some(field) = left_out(&Value::Object(more.clone()), &kept, "more") {
        return Err(format!("{field}: not a field zirc-proto has there"));
    }
    Ok(message)
}

/// The first field of `given`, as `at.field`, that `kept`, the same read by zirc-proto and
/// written again, lacks: one zirc-proto doesn't know, which serde passes over in silence.
/// A field that says nothing (`null`, `false`, nothing in it) may be left out: writing
/// leaves out what's the default.
fn left_out(given: &Value, kept: &Value, at: &str) -> Option<String> {
    match (given, kept) {
        (Value::Object(given), Value::Object(kept)) => given.iter().find_map(|(key, value)| {
            let here = format!("{at}.{key}");
            match kept.get(key) {
                Some(kept) => left_out(value, kept, &here),
                None => (!says_nothing(value)).then_some(here),
            }
        }),
        (Value::Array(given), Value::Array(kept)) => given
            .iter()
            .zip(kept)
            .enumerate()
            .find_map(|(i, (given, kept))| left_out(given, kept, &format!("{at}[{i}]"))),
        _ => None,
    }
}

fn says_nothing(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(fields) => fields.is_empty(),
        _ => false,
    }
}

/// What the server on `port` of 127.0.0.1 takes in one multiline message, as it tells a
/// client registering, which this does for a moment as zirc's clients do.
///
/// # Errors
///
/// When the server can't be reached or asked, or takes no multiline messages.
pub fn limits(port: u16) -> Result<Limits, String> {
    let failed = |error: std::io::Error| format!("asking the server its limits: {error}");
    let stream = TcpStream::connect(("127.0.0.1", port)).map_err(failed)?;
    stream.set_read_timeout(Some(WITHIN)).map_err(failed)?;
    let mut write = stream.try_clone().map_err(failed)?;
    let mut send = |lines: &[Line]| {
        lines
            .iter()
            .try_for_each(|line| write!(write, "{line}\r\n"))
    };
    let caps = ["batch", "message-tags", multiline::CAP].map(String::from);
    let mut registration = Registration::new(Identity::new(NICK, caps.to_vec()));
    send(&registration.opening()).map_err(failed)?;
    for text in BufReader::new(stream).lines() {
        let Ok(line) = text.map_err(failed)?.parse::<Line>() else {
            continue;
        };
        match registration
            .step(&line)
            .map_err(|error| error.to_string())?
        {
            Step::Reply(lines) => send(&lines).map_err(failed)?,
            Step::Registered(registered) => {
                send(&[Line::new("QUIT", ["asked"])]).map_err(failed)?;
                return registered
                    .multiline
                    .ok_or_else(|| "the server takes no multiline messages".to_owned());
            }
        }
    }
    Err("the server closed the connection before saying its limits".to_owned())
}

#[cfg(test)]
mod tests {
    use zirc_line::multiline::Batches;

    use super::*;

    const LIMITS: Limits = Limits {
        max_bytes: 4096,
        max_lines: 100,
    };

    /// What `ask` is answered: its lines, or its error.
    fn staged(ask: &Value) -> Result<Vec<Line>, String> {
        let answer: Value =
            serde_json::from_str(&Stager::with_limits(LIMITS).answer(&ask.to_string())).unwrap();
        assert_eq!(answer["id"], ask["id"], "{answer}");
        match answer["error"].as_str() {
            Some(error) => Err(error.to_owned()),
            None => Ok(answer["lines"]
                .as_array()
                .expect("lines")
                .iter()
                .map(|line| line.as_str().expect("a line").parse().expect("an IRC line"))
                .collect()),
        }
    }

    fn ask(event: &Value, more: &Value) -> Value {
        json!({ "id": 7, "stage": "event", "target": "#web", "session": "cc-1", "event": event, "more": more })
    }

    #[test]
    fn a_short_event_is_one_privmsg_zirc_proto_reads_back() {
        let event = json!({ "type": "text", "body": "the form posts twice" });
        let lines = staged(&ask(
            &event,
            &json!({ "name": "checkout", "avatar": "rocket" }),
        ))
        .unwrap();
        let [line] = lines.as_slice() else {
            panic!("{lines:?}");
        };
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.params[0], "#web");
        assert!(line.tags.is_empty() && line.source.is_none(), "{line}");
        // What it says for people is zirc-proto's, not the harness's.
        assert!(
            line.params[1].ends_with(" :the form posts twice\u{1}"),
            "{line}"
        );
        let message = zirc_proto::decode(&line.params[1]).unwrap().unwrap();
        assert_eq!(message.session, "cc-1");
        assert_eq!(message.name.as_deref(), Some("checkout"));
        assert_eq!(message.avatar.as_deref(), Some("rocket"));
        assert_eq!(
            message.event,
            Event::Text {
                body: "the form posts twice".into()
            }
        );
    }

    #[test]
    fn a_long_event_is_a_batch_zirc_lines_reader_joins_to_the_same_body() {
        let paths: Vec<String> = (0..40)
            .map(|i| format!("web/src/checkout/part-{i}.tsx"))
            .collect();
        let event = json!({ "type": "intent", "summary": "Fix the login form", "paths": paths });
        let lines = staged(&ask(&event, &json!({}))).unwrap();
        assert!(lines.len() > 3, "{lines:?}");
        assert_eq!(lines[0].to_string(), "BATCH +staged1 draft/multiline #web");
        assert_eq!(lines.last().unwrap().to_string(), "BATCH -staged1");
        // As a client hears them: every line but the last is held, then the whole comes.
        let mut batches = Batches::default();
        let mut heard: Vec<Line> = lines
            .into_iter()
            .filter_map(|line| batches.feed(line))
            .collect();
        let whole = heard.pop().expect("the message, once its batch closes");
        assert!(heard.is_empty(), "{heard:?}");
        assert_eq!(
            (whole.command.as_str(), whole.params[0].as_str()),
            ("PRIVMSG", "#web")
        );
        let message = zirc_proto::decode(&whole.params[1]).unwrap().unwrap();
        assert_eq!(
            whole.params[1],
            zirc_proto::encode_up_to(&message, LIMITS.max_bytes).unwrap()
        );
        assert!(matches!(message.event, Event::Intent { paths: read, .. } if read == paths));
    }

    #[test]
    fn each_batch_has_a_name_of_its_own() {
        let paths: Vec<String> = (0..40)
            .map(|i| format!("web/src/checkout/part-{i}.tsx"))
            .collect();
        let long = ask(
            &json!({ "type": "working", "paths": paths, "continued": false }),
            &json!({ "level": "info" }),
        );
        let mut stager = Stager::with_limits(LIMITS);
        let first: Value = serde_json::from_str(&stager.answer(&long.to_string())).unwrap();
        let second: Value = serde_json::from_str(&stager.answer(&long.to_string())).unwrap();
        assert_eq!(first["lines"][0], "BATCH +staged1 draft/multiline #web");
        assert_eq!(second["lines"][0], "BATCH +staged2 draft/multiline #web");
    }

    #[test]
    fn what_is_only_true_now_is_a_tagmsg_as_zircd_says_it() {
        let event = json!({ "type": "working", "paths": ["docs/search.md"], "part": [1, 1] });
        let lines = staged(&ask(&event, &json!({ "name": "docs-search" }))).unwrap();
        let [line] = lines.as_slice() else {
            panic!("{lines:?}");
        };
        assert_eq!(
            (line.command.as_str(), line.params.as_slice()),
            ("TAGMSG", &["#web".to_owned()][..])
        );
        let message = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap();
        assert!(message.is_live());
        assert_eq!(message.name.as_deref(), Some("docs-search"));
        assert!(matches!(
            message.event,
            Event::Working {
                part: Some([1, 1]),
                ..
            }
        ));
    }

    #[test]
    fn a_wrong_shape_is_an_error_naming_what_was_wrong() {
        // A commit says its `subject`: `summary` is an intent's.
        let commit =
            json!({ "type": "commit", "branch": "main", "sha": "abc1234", "summary": "Fix it" });
        let error = staged(&ask(&commit, &json!({}))).unwrap_err();
        assert!(
            error.starts_with("event: ") && error.contains("subject"),
            "{error}"
        );
        // A field zirc-proto doesn't have, which serde alone would pass over.
        let text = json!({ "type": "text", "body": "hi", "colour": "red" });
        assert_eq!(
            staged(&ask(&text, &json!({}))).unwrap_err(),
            "event.colour: not a field zirc-proto has there"
        );
        let text = json!({ "type": "text", "body": "hi" });
        assert_eq!(
            staged(&ask(&text, &json!({ "nickname": "checkout" }))).unwrap_err(),
            "more.nickname: not a field zirc-proto has there"
        );
        let error = staged(&ask(&text, &json!({ "level": "loud" }))).unwrap_err();
        assert!(
            error.starts_with("more: ") && error.contains("loud"),
            "{error}"
        );
        assert!(
            staged(&ask(&text, &json!({ "session": "cc-2" })))
                .unwrap_err()
                .starts_with("more.session: ")
        );
        assert!(
            staged(&ask(&json!({ "type": "shrug" }), &json!({})))
                .unwrap_err()
                .starts_with("event: ")
        );
        // Not an event at all, and not a command at all: each still answers its id.
        let mut unknown = ask(&text, &json!({}));
        unknown["stage"] = "showcase".into();
        assert!(staged(&unknown).unwrap_err().contains("showcase"));
        let mut two = ask(&text, &json!({}));
        two["target"] = "#web #api".into();
        assert!(staged(&two).unwrap_err().starts_with("target: "));
        let answer: Value =
            serde_json::from_str(&Stager::with_limits(LIMITS).answer("restart now")).unwrap();
        assert!(
            answer["error"].as_str().unwrap().starts_with("not JSON: "),
            "{answer}"
        );
    }

    #[test]
    fn a_field_that_says_nothing_is_no_error() {
        let push = json!({ "type": "push", "branch": "main", "head": "abc1234def", "forced": false, "subjects": [], "repo": null });
        let lines = staged(&ask(&push, &json!({ "everyone": false }))).unwrap();
        let message = zirc_proto::decode(&lines[0].params[1]).unwrap().unwrap();
        assert!(matches!(message.event, Event::Push { forced: false, .. }));
    }

    #[test]
    fn an_event_too_long_for_the_server_is_an_error() {
        let event = json!({ "type": "text", "body": "x".repeat(5000) });
        let error = staged(&ask(&event, &json!({}))).unwrap_err();
        assert!(error.contains("even as a multiline message"), "{error}");
    }
}
