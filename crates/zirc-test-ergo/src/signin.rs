//! A stand-in for zirc.dev that signs a person's app in for real, for end-to-end runs of an
//! app against a throwaway Ergo, such as the Android app's on an emulator. Where [`crate::web`]
//! answers what a test wrote, this one answers as zirc.dev does, in `zirc_proto`'s own shapes:
//!
//! - `POST /api/cli/login` starts a login ([`zirc_proto::login`]), and `GET
//!   /api/cli/login/<id>` says pending a few times, then registers the person's account on
//!   the Ergo, as zirc.dev would through the controller, and answers what to connect with:
//!   so the app's SASL login is real.
//! - `GET /api/me` is the person's profile ([`zirc_proto::profile`]) in the one organisation,
//!   and a `PATCH` keeps what they share of it.
//! - With [`Options::steps`], `GET /api/me/steps` is getting started as it stands for them
//!   ([`zirc_proto::onboarding::progress`]), so an app shows the next step's prompt.
//! - With a directory of media (`demo.mp4`, `demo-poster.png`, and any `<id>.gif`), a file at
//!   `/m/<org>/<id>` is served as zirc.dev serves one to an app's account: its picture for
//!   `?w=` (the `<id>.gif` if there is one, animated, else the poster), and for `?r=` "being
//!   made" (202) once, then a redirect to the copy at `/v/<id>.mp4`, which needs no account.
//!
//! `serve` starts one when asked on its standard input ([`answer`]) or over its control
//! port ([`crate::control`]).

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use zirc_proto::login::{
    Account, Credentials, LoginKind, LoginRequest, LoginStarted, LoginStatus, START_PATH, Server,
};
use zirc_proto::profile::{ME_PATH, Me, MeOrg};

use crate::http::{self, Answer};

/// What every account is registered with. No spaces: `NickServ REGISTER` would take a
/// second word as an email address.
pub const PASSPHRASE: &str = "correct-horse-battery-staple";

/// How the stand-in is set up: what `serve` is asked under `web` ([`answer`]).
#[derive(Debug, Clone, Deserialize)]
pub struct Options {
    /// The loopback port to listen on, or 0 for a free one.
    #[serde(default)]
    pub port: u16,
    /// This machine as the app reaches it: `10.0.2.2` from Android's emulator. It names the
    /// Ergo in the credentials, the stand-in's own pages, and the organisation's slug, as an
    /// app names the one it signed in to by its server's host.
    #[serde(default = "loopback")]
    pub host: String,
    /// Whom a login signs in as, whose account is registered on the Ergo.
    pub handle: String,
    /// How many polls answer pending before the login is done.
    #[serde(default = "two")]
    pub pending: u32,
    /// The directory of media to serve, if any.
    #[serde(default)]
    pub media: Option<PathBuf>,
    /// The steps of getting started the person has reached, by number, which
    /// [`zirc_proto::onboarding::STEPS_PATH`] answers with; none, and it's not found.
    #[serde(default)]
    pub steps: Option<Vec<u8>>,
}

fn loopback() -> String {
    Ipv4Addr::LOCALHOST.to_string()
}

fn two() -> u32 {
    2
}

/// What the stand-in keeps between requests.
struct State {
    options: Options,
    /// Its own port, as it listens.
    port: u16,
    /// The Ergo's plaintext port.
    irc_port: u16,
    /// Each login started and how often it was polled.
    polls: Mutex<HashMap<String, u32>>,
    /// How many logins were started, for their ids.
    logins: Mutex<u32>,
    /// The person's profile, as they last left it.
    me: Mutex<Me>,
    /// The files asked to play once, whose copy is made by the next ask.
    making: Mutex<HashSet<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the stand-in for the Ergo listening on `irc_port`, and answers its address, such
/// as `http://127.0.0.1:8787`. It answers each request on a thread of its own, as an app asks
/// for pictures and a video at once, until the process ends.
///
/// # Errors
///
/// When its port can't be listened on, as when something else has it.
pub fn start(options: Options, irc_port: u16) -> std::io::Result<String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, options.port))?;
    let port = listener.local_addr()?.port();
    let me = profile(&options);
    let state = Arc::new(State {
        options,
        port,
        irc_port,
        polls: Mutex::default(),
        logins: Mutex::default(),
        me: Mutex::new(me),
        making: Mutex::default(),
    });
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = Arc::clone(&state);
            std::thread::spawn(move || serve(&state, stream));
        }
    });
    Ok(format!("http://{}:{port}", Ipv4Addr::LOCALHOST))
}

/// `serve`'s answer to a line asking for the stand-in, or `None` when the line asks for
/// something else: `{"id": 1, "web": {"handle": "dana", …}}`, the rest as [`Options`], is
/// answered `{"id": 1, "web": "http://127.0.0.1:8787", "passphrase": "…"}`, the passphrase
/// being what it registers accounts with ([`PASSPHRASE`]), for a harness signing in as the
/// same person from elsewhere; or with the `error` that stopped it.
#[must_use]
pub fn answer(ask: &str, irc_port: u16) -> Option<String> {
    let mut asked: Value = serde_json::from_str(ask).ok()?;
    let options = asked.as_object_mut()?.remove("web")?;
    let id = asked.get("id").cloned().unwrap_or_default();
    let started = serde_json::from_value::<Options>(options)
        .map_err(|error| error.to_string())
        .and_then(|options| start(options, irc_port).map_err(|error| error.to_string()));
    Some(
        match started {
            Ok(address) => json!({ "id": id, "web": address, "passphrase": PASSPHRASE }),
            Err(error) => json!({ "id": id, "error": error }),
        }
        .to_string(),
    )
}

/// The person's profile in the one organisation the app signs in to.
fn profile(options: &Options) -> Me {
    Me {
        email: format!("{}@acme.example", options.handle),
        name: Some("Dana Reyes".to_owned()),
        avatar: None,
        orgs: vec![MeOrg {
            slug: options.host.clone(),
            name: "Acme".to_owned(),
            handle: options.handle.clone(),
            role: zirc_proto::role::Role::Owner,
        }],
    }
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
    let signed_in = head.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value.trim().starts_with("Basic ")
        })
    });
    http::respond(&mut stream, state.answer(method, target, &body, signed_in));
}

impl State {
    /// This stand-in as the app reaches it.
    fn address(&self) -> String {
        format!("http://{}:{}", self.options.host, self.port)
    }

    fn answer(&self, method: &str, target: &str, body: &str, signed_in: bool) -> Answer {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match method {
            "POST" if path == START_PATH => self.start_login(body),
            "GET" if path == ME_PATH => Answer::json(200, &*lock(&self.me)),
            "GET" if path == zirc_proto::onboarding::STEPS_PATH => match &self.options.steps {
                Some(reached) => Answer::json(200, &zirc_proto::onboarding::progress(reached)),
                None => Answer::error(404, "no such page"),
            },
            // Nobody here has a picture: the organisation shows their initials.
            "GET" if path.starts_with("/api/media/avatar/") => Answer::error(404, "no picture"),
            "GET" if path.starts_with("/v/") || path.starts_with("/m/") => {
                self.media(path, query, signed_in)
            }
            "GET" => match path
                .strip_prefix(START_PATH)
                .and_then(|id| id.strip_prefix('/'))
            {
                Some(id) => self.poll(id),
                None => Answer::error(404, "no such page"),
            },
            _ => Answer::error(404, "no such page"),
        }
    }

    fn start_login(&self, body: &str) -> Answer {
        let request: LoginRequest = match serde_json::from_str(body) {
            Ok(request) => request,
            Err(error) => return Answer::error(400, &error.to_string()),
        };
        if request.kind != Some(LoginKind::Person) {
            return Answer::error(400, "only a person's app signs in here");
        }
        let id = {
            let mut logins = lock(&self.logins);
            *logins += 1;
            format!("l{logins}")
        };
        lock(&self.polls).insert(id.clone(), 0);
        Answer::json(
            200,
            &LoginStarted {
                id,
                code: "WXYZ-1234".to_owned(),
                url: format!("{}/login", self.address()),
                interval: 1,
                expires_in: 120,
            },
        )
    }

    /// Where the login `id` stands: pending as often as asked for, then done, once, with the
    /// account registered on the Ergo.
    fn poll(&self, id: &str) -> Answer {
        {
            let mut polls = lock(&self.polls);
            let Some(asked) = polls.get_mut(id) else {
                return Answer::error(404, "no such login");
            };
            *asked += 1;
            if *asked <= self.options.pending {
                return Answer::json(200, &LoginStatus::Pending);
            }
            polls.remove(id);
        }
        let handle = &self.options.handle;
        // Registered already, as by an earlier login: the same passphrase signs in.
        let _ = register(self.irc_port, handle, PASSPHRASE);
        // The fields zirc.dev always says, read as zirc-proto reads them: the rest of
        // [`Credentials`] is what newer servers add, and left out as an older one leaves it.
        let credentials = json!({
            "server": Server {
                host: self.options.host.clone(),
                port: Some(self.irc_port),
                tls: Some(false),
                password: None,
            },
            "account": Account {
                name: handle.clone(),
                password: PASSPHRASE.to_owned(),
                device_key: false,
            },
            "nick": handle,
            "owner": handle,
        });
        match serde_json::from_value::<Credentials>(credentials) {
            Ok(credentials) => Answer::json(200, &LoginStatus::Done(credentials)),
            Err(error) => Answer::error(500, &error.to_string()),
        }
    }

    /// A shared file, as zirc.dev serves one.
    fn media(&self, path: &str, query: &str, signed_in: bool) -> Answer {
        let Some(dir) = &self.options.media else {
            return Answer::error(404, "no media here");
        };
        let read = |name: &str, kind| match std::fs::read(dir.join(name)) {
            Ok(bytes) => Answer::file(kind, bytes),
            Err(_) => Answer::error(404, "no such file"),
        };
        let id = path.rsplit('/').next().unwrap_or_default();
        // A file's name alone: nothing above the directory is read.
        if id.is_empty() || id.contains("..") {
            return Answer::error(404, "no such file");
        }
        if path.starts_with("/v/") {
            return read("demo.mp4", "video/mp4");
        }
        if !signed_in {
            return Answer::error(401, "sign in first");
        }
        let asks = |key: &str| {
            query
                .split('&')
                .any(|pair| pair.split('=').next() == Some(key))
        };
        if asks("w") {
            let gif = format!("{id}.gif");
            return if dir.join(&gif).exists() {
                read(&gif, "image/gif")
            } else {
                read("demo-poster.png", "image/png")
            };
        }
        // Being made at first, as zirc.dev says while its worker makes the copy.
        if lock(&self.making).insert(path.to_owned()) {
            return Answer::json(202, &json!({ "processing": true }));
        }
        Answer {
            status: 302,
            kind: "application/json",
            location: Some(format!("{}/v/{id}.mp4", self.address())),
            body: Vec::new(),
        }
    }
}

/// Registers `handle` on the Ergo at `port` with `NickServ`, as zirc.dev's controller would.
///
/// # Errors
///
/// When the server can't be reached or doesn't answer in a few seconds, as when the nick is
/// an account's already: then no welcome comes for a connection that hasn't signed in.
pub fn register(port: u16, handle: &str, passphrase: &str) -> std::io::Result<()> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(stream, "NICK {handle}\r\nUSER {handle} 0 * :{handle}\r\n")?;
    // Registering an account needs a registered connection: the welcome first.
    let mut seen = String::new();
    hear(&mut stream, &mut seen, " 001 ")?;
    seen.clear();
    write!(stream, "PRIVMSG NickServ :REGISTER {passphrase}\r\n")?;
    hear(&mut stream, &mut seen, ":NickServ")?;
    write!(stream, "QUIT\r\n")
}

/// Says `body` in `channel` on the Ergo at `port` as the account `account`, which
/// [`register`] made with `passphrase`: signed in, so the server says whose the line is (`account-tag`),
/// as it does of an integration's such as CI's `zirc-ci`. For tests without an async client
/// of their own; the others use zirc-test-irc's `integration`.
///
/// # Errors
///
/// When the server can't be reached, doesn't let the account in, or doesn't answer in a few
/// seconds.
pub fn say_as(
    port: u16,
    account: &str,
    passphrase: &str,
    channel: &str,
    body: &str,
) -> std::io::Result<()> {
    use base64::Engine as _;
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut seen = String::new();
    write!(
        stream,
        "CAP REQ :sasl\r\nNICK {account}\r\nUSER {account} 0 * :{account}\r\nAUTHENTICATE PLAIN\r\n"
    )?;
    hear(&mut stream, &mut seen, "AUTHENTICATE +")?;
    let plain =
        base64::engine::general_purpose::STANDARD.encode(format!("\0{account}\0{passphrase}"));
    write!(stream, "AUTHENTICATE {plain}\r\n")?;
    hear(&mut stream, &mut seen, " 903 ")?;
    write!(stream, "CAP END\r\n")?;
    hear(&mut stream, &mut seen, " 001 ")?;
    seen.clear();
    write!(stream, "JOIN {channel}\r\n")?;
    hear(&mut stream, &mut seen, " 366 ")?;
    // Heard back, the server has taken the line before the connection goes.
    write!(stream, "PRIVMSG {channel} :{body}\r\nPING :said\r\n")?;
    hear(&mut stream, &mut seen, "PONG")?;
    write!(stream, "QUIT\r\n")
}

/// Reads from `stream` into `seen` until it has `needle`. A read a signal interrupts is
/// read again: on Linux a socket with a read timeout isn't restarted after one (signal(7)),
/// and tokio's handler for its children's `SIGCHLD` runs in the tests that call this.
fn hear(stream: &mut TcpStream, seen: &mut String, needle: &str) -> std::io::Result<()> {
    let mut buf = [0; 4096];
    while !seen.contains(needle) {
        let n = match stream.read(&mut buf) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            read => read?,
        };
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    Ok(())
}
