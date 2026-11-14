//! What the stager answers, sent through a real Ergo by a connection that asked for no
//! capabilities, as a harness's plain one hasn't: a client with zirc's hears the event whole.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{Value, json};
use zirc_line::Line;
use zirc_line::multiline::Batches;
use zirc_proto::{Event, Message};
use zirc_test_ergo::require_ergo;
use zirc_test_ergo::stage::{Stager, limits};

/// A connection as `nick`, in `#stage`, having asked for `caps`.
struct Talker {
    read: BufReader<TcpStream>,
    write: TcpStream,
    batches: Batches,
}

impl Talker {
    fn join(port: u16, nick: &str, caps: &str) -> Self {
        let write = TcpStream::connect(("127.0.0.1", port)).expect("connecting");
        write
            .set_read_timeout(Some(Duration::from_secs(20)))
            .expect("a timeout");
        let read = BufReader::new(write.try_clone().expect("a second handle"));
        let mut talker = Self {
            read,
            write,
            batches: Batches::default(),
        };
        if !caps.is_empty() {
            talker.send(&format!("CAP REQ :{caps}"));
        }
        talker.send(&format!("NICK {nick}"));
        talker.send(&format!("USER {nick} 0 * :{nick}"));
        talker.send("CAP END");
        talker.send("JOIN #stage");
        talker.hears(|line| line.command == "JOIN");
        talker
    }

    fn send(&mut self, line: &str) {
        write!(self.write, "{line}\r\n").expect("writing");
    }

    /// The next whole line `wanted` says yes to, a multiline message as the one it is.
    fn hears(&mut self, wanted: impl Fn(&Line) -> bool) -> Line {
        let mut heard = Vec::new();
        loop {
            let mut text = String::new();
            let read = self.read.read_line(&mut text);
            assert!(
                matches!(read, Ok(n) if n > 0),
                "the line never came: {read:?}\n{}",
                heard.join("")
            );
            let line: Line = text.parse().expect("a line");
            if line.command == "PING" {
                self.send(&format!("PONG :{}", line.params[0]));
            }
            heard.push(text);
            if let Some(line) = self.batches.feed(line).filter(&wanted) {
                return line;
            }
        }
    }
}

/// The lines the stager for `port`'s server answers for `event` from `cc-1`, with `more`.
fn staged(stager: &mut Stager, event: &Value, more: &Value) -> Vec<String> {
    let ask = json!({ "id": "a", "stage": "event", "target": "#stage", "session": "cc-1", "event": event, "more": more });
    let answer: Value = serde_json::from_str(&stager.answer(&ask.to_string())).expect("JSON");
    let lines = answer["lines"]
        .as_array()
        .unwrap_or_else(|| panic!("{answer}"));
    lines
        .iter()
        .map(|line| line.as_str().expect("a line").to_owned())
        .collect()
}

#[test]
fn staged_events_reach_a_zirc_client_through_the_server() {
    let ergo = require_ergo!();
    let taken = limits(ergo.port()).expect("the server's limits");
    assert!(taken.max_bytes > zirc_proto::MAX_BODY_BYTES, "{taken:?}");

    let mut listener = Talker::join(ergo.port(), "hears", "message-tags batch draft/multiline");
    let mut sender = Talker::join(ergo.port(), "says", "");
    listener.hears(|line| {
        line.command == "JOIN"
            && line
                .source
                .as_deref()
                .is_some_and(|s| s.starts_with("says!"))
    });
    let mut stager = Stager::for_server(ergo.port());
    let read = |line: &Line| -> Option<Message> {
        match line.command.as_str() {
            "PRIVMSG" => zirc_proto::decode(line.params.get(1)?).ok().flatten(),
            "TAGMSG" => zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG)?).ok(),
            _ => None,
        }
    };

    // One line.
    let text = json!({ "type": "text", "body": "taking: the login form" });
    for line in staged(&mut stager, &text, &json!({ "name": "login" })) {
        sender.send(&line);
    }
    let heard = read(&listener.hears(|line| read(line).is_some())).unwrap();
    assert_eq!(heard.name.as_deref(), Some("login"));
    assert!(matches!(heard.event, Event::Text { body } if body == "taking: the login form"));

    // A multiline batch, within what the server said it takes.
    let paths: Vec<String> = (0..40)
        .map(|i| format!("web/src/checkout/part-{i}.tsx"))
        .collect();
    let intent = json!({ "type": "intent", "summary": "Fix the login form", "paths": paths });
    let lines = staged(&mut stager, &intent, &json!({}));
    assert!(lines[0].starts_with("BATCH +"), "{lines:?}");
    for line in lines {
        sender.send(&line);
    }
    let heard = read(&listener.hears(|line| read(line).is_some())).unwrap();
    assert!(matches!(heard.event, Event::Intent { paths: read, .. } if read == paths));

    // What's only true now: a TAGMSG, its event in its tag.
    let working = json!({ "type": "working", "paths": ["docs/search.md"], "part": [1, 1] });
    for line in staged(&mut stager, &working, &json!({})) {
        sender.send(&line);
    }
    let line = listener.hears(|line| read(line).is_some());
    assert_eq!(line.command, "TAGMSG");
    assert!(read(&line).unwrap().is_live());
}
