//! `serve`'s control port: the end-to-end path ([`crate::scenario`]) for a driver that can't
//! hold `serve`'s standard input, such as an iPhone's UI test, a shell script or an app's own
//! scripted harness. HTTP on the loopback, JSON both ways, a request a connection:
//!
//! | Ask | Answer |
//! |---|---|
//! | `GET /words` | The path's fixed names and words ([`Words`]): `{"channel":"#web","teammate":"bob",…}`. `GET /words?sh` says them as a shell reads them, `E2E_CHANNEL='#web'` a line each. |
//! | `POST /signin` `{"handle":"dana"}` | Starts the stand-in zirc.dev that signs the app in as `handle` ([`crate::signin`], the body its [`Options`]): `{"web":"http://127.0.0.1:8787","passphrase":"…","irc_port":6667}`. Asked again for the same handle, the same stand-in. |
//! | `POST /step/history`, `/step/reply`, `/step/back` | Plays that step of the teammate's side ([`Scenario::play`]): `{"said":[{"id":"…","from":"bob","text":"…"}]}`. Asked again, the same answer and nothing said twice. |
//! | `POST /restart` | Restarts the server ([`Scenario::restart`]): `{"restarted":true}`, once it listens again and the teammates are back. |
//! | `POST /joined` `{"nick":"dana","channel":"#web","within":60}` | Whether `nick` is in `channel` on the server as it runs now ([`Whether`]): `{"joined":true}`, as after a restart once the app is back. After `within` seconds without them, 408. |
//! | `POST /heard` `{"from":"dana","channel":"#web","text":"hello","react":"👍","within":20}` | What the server heard `from` say in `channel` ([`Ask`]), once it has a message containing `text` and a standing reaction `react`, where given: `{"messages":[{"id","from","text","reply_to"}],"reactions":[{"from","emoji","to"}]}`. After `within` seconds without them, 408 with what was heard so far. |
//!
//! What goes wrong is `{"error":"…"}` with 400, 404 or 500. Each step may be asked again
//! after a failure or a lost answer.
//!
//! `serve` starts it when `ZIRC_SERVE_CONTROL` names a port, 0 for a free one, and prints
//! its address on a third line, `control http://127.0.0.1:<port>`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::json;

use crate::http::{self, Answer};
use crate::scenario::{Ask, HeardLog, Scenario, WORDS, Whether};
use crate::signin::{self, Options};

/// What the control port keeps between requests.
struct State {
    scenario: Arc<Mutex<Scenario>>,
    /// What the server heard, asked about while a step plays.
    heard: HeardLog,
    irc_port: u16,
    /// The stand-in zirc.dev started for each handle, by its address.
    signins: Mutex<HashMap<String, String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the control port for `scenario` on the loopback `port`, 0 for a free one, and
/// answers its address, such as `http://127.0.0.1:4711`. It answers each request on a thread
/// of its own until the process ends.
///
/// # Errors
///
/// When the port can't be listened on, as when something else has it.
pub fn start(scenario: Arc<Mutex<Scenario>>, port: u16) -> std::io::Result<String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let address = format!("http://{}", listener.local_addr()?);
    let (heard, irc_port) = {
        let scenario = lock(&scenario);
        (scenario.heard(), scenario.port())
    };
    let state = Arc::new(State {
        scenario,
        heard,
        irc_port,
        signins: Mutex::default(),
    });
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = Arc::clone(&state);
            std::thread::spawn(move || serve(&state, stream));
        }
    });
    Ok(address)
}

/// Asks the control port at `control` (its address, as [`start`] answered it), for a driver
/// in Rust: `method` and `path` with the JSON `body`, answered with the status and the body.
///
/// # Errors
///
/// When the port can't be reached or its answer isn't HTTP.
pub fn ask(control: &str, method: &str, path: &str, body: &str) -> std::io::Result<(u16, String)> {
    use std::io::{Read as _, Write as _};
    let mut stream = TcpStream::connect(control.trim_start_matches("http://"))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer)?;
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    let (head, body) = answer.split_once("\r\n\r\n").ok_or_else(invalid)?;
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok());
    Ok((status.ok_or_else(invalid)?, body.to_owned()))
}

/// Reads one request from `stream` and answers it.
fn serve(state: &State, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Some((head, body)) = http::read_request(&mut stream) else {
        return;
    };
    let mut first = head.lines().next().unwrap_or_default().split(' ');
    let (method, target) = (
        first.next().unwrap_or_default(),
        first.next().unwrap_or_default(),
    );
    http::respond(&mut stream, state.answer(method, target, &body));
}

impl State {
    fn answer(&self, method: &str, target: &str, body: &str) -> Answer {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match (method, path) {
            ("GET", "/words") if query == "sh" => {
                Answer::file("text/plain; charset=utf-8", WORDS.shell().into_bytes())
            }
            ("GET", "/words") => Answer::json(200, &WORDS),
            ("POST", "/signin") => self.signin(body),
            ("POST", "/restart") => match lock(&self.scenario).restart() {
                Ok(()) => Answer::json(200, &json!({ "restarted": true })),
                Err(error) => Answer::error(500, &error),
            },
            ("POST", "/joined") => match serde_json::from_str::<Whether>(body) {
                Ok(whether) => match self.heard.joined(&whether) {
                    Ok(()) => Answer::json(200, &json!({ "joined": true })),
                    Err(error) => Answer::error(408, &error),
                },
                Err(error) => Answer::error(400, &error.to_string()),
            },
            ("POST", "/heard") => match serde_json::from_str::<Ask>(body) {
                Ok(ask) => match self.heard.heard(&ask) {
                    Ok(heard) => Answer::json(200, &heard),
                    Err(so_far) => Answer::json(408, &so_far),
                },
                Err(error) => Answer::error(400, &error.to_string()),
            },
            ("POST", _) => match path.strip_prefix("/step/") {
                Some(step) => match lock(&self.scenario).play(step) {
                    Ok(said) => Answer::json(200, &json!({ "said": said })),
                    Err(error) if error.starts_with("no step called") => Answer::error(404, &error),
                    Err(error) => Answer::error(500, &error),
                },
                None => Answer::error(404, "no such page"),
            },
            _ => Answer::error(404, "no such page"),
        }
    }

    /// The stand-in zirc.dev for the handle `body` names, started if it isn't yet.
    fn signin(&self, body: &str) -> Answer {
        let options: Options = match serde_json::from_str(body) {
            Ok(options) => options,
            Err(error) => return Answer::error(400, &error.to_string()),
        };
        let mut signins = lock(&self.signins);
        let known = signins.get(&options.handle).cloned();
        let web = if let Some(web) = known {
            web
        } else {
            let handle = options.handle.clone();
            match signin::start(options, self.irc_port) {
                Ok(web) => {
                    signins.insert(handle, web.clone());
                    web
                }
                Err(error) => return Answer::error(500, &error.to_string()),
            }
        };
        Answer::json(
            200,
            &json!({ "web": web, "passphrase": signin::PASSPHRASE, "irc_port": self.irc_port }),
        )
    }
}
