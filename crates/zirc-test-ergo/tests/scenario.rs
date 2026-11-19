//! The apps' end-to-end path, walked whole over the control port against a real Ergo, a
//! plain IRC connection standing in for the app: what each app's driver does through its
//! screen, this does on the wire.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use zirc_line::Line;
use zirc_line::multiline::Batches;
use zirc_line::registration::{Identity, Registration, Sasl, Step};
use zirc_proto::login::{LoginStarted, LoginStatus, START_PATH, status_path};
use zirc_test_ergo::scenario::{Scenario, WORDS};
use zirc_test_ergo::{control, require_ergo};

/// Asks the control port, or the stand-in zirc.dev, and gives the status and the answer.
fn ask(address: &str, method: &str, path: &str, body: &Value) -> (u16, Value) {
    let body = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    let (status, answer) = control::ask(address, method, path, &body).expect("an answer");
    (
        status,
        serde_json::from_str(&answer).unwrap_or(Value::String(answer)),
    )
}

/// The same, which must be answered 200.
fn asked(address: &str, method: &str, path: &str, body: &Value) -> Value {
    let (status, answer) = ask(address, method, path, body);
    assert_eq!(status, 200, "{method} {path}: {answer}");
    answer
}

/// The app's side of the path: a person's connection, signed in to their account.
struct App {
    read: BufReader<TcpStream>,
    write: TcpStream,
    batches: Batches,
}

impl App {
    /// Signs in as `handle` with `password`, as an app does with what zirc.dev answered.
    fn sign_in(port: u16, handle: &str, password: &str) -> Self {
        let write = TcpStream::connect(("127.0.0.1", port)).expect("connecting");
        write
            .set_read_timeout(Some(Duration::from_secs(20)))
            .expect("a timeout");
        let read = BufReader::new(write.try_clone().expect("a second handle"));
        let mut app = Self {
            read,
            write,
            batches: Batches::default(),
        };
        let caps = [
            "batch",
            "draft/chathistory",
            "echo-message",
            "message-tags",
            "server-time",
        ];
        let mut identity = Identity::new(handle, caps.map(String::from).to_vec());
        identity.sasl = Some(Sasl::Plain {
            account: handle.to_owned(),
            password: password.to_owned(),
        });
        let mut registration = Registration::new(identity);
        for line in registration.opening() {
            app.send(&line);
        }
        loop {
            let line = app.next().expect("the server's welcome");
            match registration.step(&line).expect("registering") {
                Step::Reply(lines) => lines.iter().for_each(|line| app.send(line)),
                Step::Registered(registered) => {
                    assert_eq!(registered.nick, handle);
                    return app;
                }
            }
        }
    }

    fn send(&mut self, line: &Line) {
        write!(self.write, "{line}\r\n").expect("writing");
    }

    /// The next whole line, or `None` once the server has gone.
    fn next(&mut self) -> Option<Line> {
        loop {
            let mut text = String::new();
            match self.read.read_line(&mut text) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            let line: Line = text.parse().expect("a line");
            if line.command == "PING" {
                self.send(&Line::new("PONG", line.params.clone()));
                continue;
            }
            if let Some(line) = self.batches.feed(line) {
                return Some(line);
            }
        }
    }

    /// The next line `wanted` says yes to.
    fn hears(&mut self, what: &str, wanted: impl Fn(&Line) -> bool) -> Line {
        loop {
            let line = self.next().unwrap_or_else(|| panic!("{what} never came"));
            if wanted(&line) {
                return line;
            }
        }
    }

    fn join(&mut self, channel: &str) {
        self.send(&Line::new("JOIN", [channel]));
        self.hears("the end of the channel's names", |line| {
            line.command == "366"
        });
    }
}

/// Whether `line` is a message in the path's channel whose text contains `text`.
fn says(line: &Line, text: &str) -> bool {
    line.command == "PRIVMSG"
        && line.params.first().is_some_and(|to| to == WORDS.channel)
        && line.params.get(1).is_some_and(|said| said.contains(text))
}

/// Signs in through the stand-in zirc.dev at `web`, and gives the account's password.
fn credentials(web: &str) -> String {
    let request = json!({ "client": "zirc-test 0", "host": "test", "kind": "person" });
    let started: LoginStarted =
        serde_json::from_value(asked(web, "POST", START_PATH, &request)).expect("a login");
    for _ in 0..10 {
        let status = asked(web, "GET", &status_path(&started.id), &Value::Null);
        if let LoginStatus::Done(credentials) = serde_json::from_value(status).expect("a status") {
            return credentials.account.expect("an account").password;
        }
    }
    panic!("the login never finished");
}

/// What to ask `/heard` about dana in the path's channel, with `more` of an ask's fields.
fn about(more: &Value) -> Value {
    let mut ask = json!({ "from": "dana", "channel": WORDS.channel, "within": 10 });
    let more = more.as_object().cloned().unwrap_or_default();
    ask.as_object_mut().expect("an object").extend(more);
    ask
}

/// Step 1, signing in: the stand-in's account is one the server takes. Its password.
fn signs_in(control: &str, port: u16) -> String {
    let signin = asked(
        control,
        "POST",
        "/signin",
        &json!({ "handle": "dana", "pending": 1 }),
    );
    assert_eq!(signin["irc_port"], port);
    // Asked again, the same stand-in.
    let again = asked(control, "POST", "/signin", &json!({ "handle": "dana" }));
    assert_eq!(again["web"], signin["web"]);
    let web = signin["web"].as_str().expect("the stand-in's address");
    let password = credentials(web);
    assert_eq!(signin["passphrase"], password.as_str());
    password
}

/// Step 2, history: said before the person came, and there for them to read. The app, in
/// the channel.
fn reads_history(control: &str, port: u16, password: &str) -> App {
    let history = asked(control, "POST", "/step/history", &Value::Null);
    let said = history["said"].as_array().expect("what was said");
    assert_eq!(said[0]["text"], WORDS.history);
    assert_eq!(said[0]["from"], WORDS.teammate);
    assert_eq!(said[1]["from"], WORDS.machine);
    let intended = said[1]["text"].as_str().unwrap_or_default();
    assert!(intended.contains(WORDS.intended), "{said:?}");
    // Played once: asked again, what it said then.
    assert_eq!(
        asked(control, "POST", "/step/history", &Value::Null),
        history
    );
    let mut app = App::sign_in(port, "dana", password);
    app.join(WORDS.channel);
    app.send(&Line::new(
        "CHATHISTORY",
        ["LATEST", WORDS.channel, "*", "20"],
    ));
    app.hears("the teammate's message in the history", |line| {
        says(line, WORDS.history)
    });
    app.hears("the machine's intent in the history", |line| {
        says(line, WORDS.intended)
    });
    app
}

/// Steps 3 to 5: the person's message, the teammate's answer, and the person's reaction to
/// it, each heard by the server.
fn talks(control: &str, app: &mut App) {
    app.send(&Line::new(
        "PRIVMSG",
        [WORDS.channel, "hello from the test"],
    ));
    let heard = asked(
        control,
        "POST",
        "/heard",
        &about(&json!({ "text": "hello from the test" })),
    );
    assert_eq!(
        heard["messages"].as_array().map(Vec::len),
        Some(1),
        "{heard}"
    );
    // What wasn't said is answered 408, with what was.
    let (status, lacking) = ask(
        control,
        "POST",
        "/heard",
        &about(&json!({ "text": "goodbye", "within": 0 })),
    );
    assert_eq!(status, 408);
    assert_eq!(lacking["messages"].as_array().map(Vec::len), Some(1));

    let reply = asked(control, "POST", "/step/reply", &Value::Null);
    let id = reply["said"][0]["id"].as_str().expect("the reply's id");
    let line = app.hears("the teammate's answer", |line| says(line, WORDS.reply));
    assert_eq!(line.tag("msgid"), Some(id));

    let mut react = Line::new("TAGMSG", [WORDS.channel]);
    react.tags = vec![
        ("+draft/react".to_owned(), Some("👍".to_owned())),
        ("+draft/reply".to_owned(), Some(id.to_owned())),
    ];
    app.send(&react);
    let heard = asked(control, "POST", "/heard", &about(&json!({ "react": "👍" })));
    assert_eq!(heard["reactions"][0]["to"], id, "{heard}");
}

#[test]
fn the_path_is_walked_whole_over_the_control_port() {
    let ergo = require_ergo!();
    let port = ergo.port();
    let scenario = Arc::new(Mutex::new(Scenario::new(ergo)));
    let control = control::start(Arc::clone(&scenario), 0).expect("the control port");

    // The words a driver looks for, as JSON and as a shell reads them.
    let words = asked(&control, "GET", "/words", &Value::Null);
    assert_eq!(words["channel"], WORDS.channel);
    let (_, shell) = control::ask(&control, "GET", "/words?sh", "").expect("the words");
    assert!(shell.contains("E2E_REPLY='welcome aboard"), "{shell}");

    let password = signs_in(&control, port);
    // A step before the teammate has arrived, and one by no name.
    assert_eq!(ask(&control, "POST", "/step/reply", &Value::Null).0, 500);
    assert_eq!(ask(&control, "POST", "/step/lunch", &Value::Null).0, 404);
    let mut app = reads_history(&control, port, &password);
    talks(&control, &mut app);

    // Step 6: the server restarts. The app's connection ends, it signs in again as before,
    // and the teammate, back too, says something more.
    let restarted = asked(&control, "POST", "/restart", &Value::Null);
    assert_eq!(restarted["restarted"], true);
    while app.next().is_some() {}
    let mut app = App::sign_in(port, "dana", &password);
    app.join(WORDS.channel);
    // The server saw the app come back.
    let back = json!({ "nick": "dana", "channel": WORDS.channel, "within": 10 });
    assert_eq!(asked(&control, "POST", "/joined", &back)["joined"], true);
    let nobody = json!({ "nick": "erin", "channel": WORDS.channel, "within": 0 });
    assert_eq!(ask(&control, "POST", "/joined", &nobody).0, 408);
    asked(&control, "POST", "/step/back", &Value::Null);
    app.hears("the teammate after the restart", |line| {
        says(line, WORDS.back)
    });
    // What was heard before the restart is still answered.
    let heard = asked(
        &control,
        "POST",
        "/heard",
        &about(&json!({ "text": "hello from the test" })),
    );
    assert_eq!(
        heard["reactions"].as_array().map(Vec::len),
        Some(1),
        "{heard}"
    );
}
