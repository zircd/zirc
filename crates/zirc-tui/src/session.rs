//! The connection behind the [`App`]: zirc-core's [`Supervisor`] keeps it up, as in zirc's
//! other apps, and this hands the app what the server sends, sends what the app asks for,
//! and tells it where the connection is.

use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{KeyEvent, MouseEventKind};
use tokio::sync::mpsc;
use zirc_core::chat::GuestRequest;
use zirc_core::connection::{self, Event, Nudge, Supervisor};
use zirc_core::ipc::{Request, Response};
use zirc_core::login::Client;
use zirc_irc::{Config, Error, Line};
use zirc_proto::login::Account;

use crate::app::{Action, App, Target};

/// How long a `QUIT` gets to reach the server before the client exits anyway.
const QUIT_GRACE: Duration = Duration::from_secs(1);

/// The app and its connection.
pub struct Session {
    /// What's shown.
    pub app: App,
    supervisor: Supervisor,
    /// What was said while not connected: it goes once the connection's up, as zirc-app's
    /// connection holds lines for the other apps (the user's choice, 20 Nov 2026).
    outbox: Vec<Line>,
    /// This machine's zircd's socket, which settings such as who may query its agents are
    /// changed through, if there's one to ask.
    pub zircd: Option<PathBuf>,
    /// Its answers, as they come.
    answers: (
        mpsc::UnboundedSender<Answer>,
        mpsc::UnboundedReceiver<Answer>,
    ),
    /// What zircd says every session is doing, and when, as it comes.
    sessions: (
        mpsc::UnboundedSender<Sessions>,
        mpsc::UnboundedReceiver<Sessions>,
    ),
    /// The organisation's zirc.dev and our account there, for what admins do through it.
    web: Option<(Client, Account)>,
    /// What zirc.dev answers, as it comes.
    web_answers: (mpsc::UnboundedSender<Web>, mpsc::UnboundedReceiver<Web>),
    /// The person's name as zirc.dev gave it, to say with `SETNAME` on connecting: an
    /// account that stays connected keeps the name it first had otherwise.
    realname: Option<String>,
}

/// What zirc.dev answered.
enum Web {
    /// Our role in the organisation.
    Role(zirc_proto::role::Role),
    /// A request of the Guests API was made, and what to say of it, or why it wasn't.
    Done(Result<String, String>),
    /// A card's button was pressed, and what the integration said, or why it couldn't be.
    Pressed(zirc_proto::card::Press, Result<Option<String>, String>),
    /// Our account is this handle now: changed elsewhere, as zirc.dev said once the server
    /// refused the old one.
    Renamed(String),
    /// The guests the organisation keeps, for an owner or admin: whose rooms the channels
    /// list.
    Kept(Vec<zirc_core::chat::KeptGuest>),
}

/// What zircd said every session is doing, or why it couldn't say, and when.
type Sessions = (Result<Vec<zirc_core::sessions::Session>, String>, u64);

/// What zircd answered, or why it couldn't be asked.
type Answer = Result<Response, String>;

/// What a key comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Carry on.
    Carry,
    /// Stop.
    Quit,
    /// Show the organisation named, or the next one.
    Org(Option<String>),
}

impl Session {
    /// Connects to the server in `config` for `app`.
    ///
    /// # Errors
    ///
    /// When the first connection fails, so a wrong server or password is said plainly
    /// rather than retried.
    #[must_use]
    pub fn connect(
        app: App,
        config: Config,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self, Error>> + Send>> {
        // Boxed: it holds the whole chat, which awaiting it in place would put in the
        // caller's future (clippy's large_futures).
        Box::pin(Self::connecting(app, config))
    }

    async fn connecting(app: App, config: Config) -> Result<Self, Error> {
        // Admins act through zirc.dev as the account we connect with.
        let web = app
            .site
            .as_ref()
            .zip(config.sasl.as_ref())
            .and_then(|(site, sasl)| {
                let account = Account {
                    name: sasl.account.clone(),
                    password: sasl.password.clone(),
                    device_key: false,
                };
                Some((Client::new(&site.web).ok()?, account))
            });
        // The person's name, when zirc.dev gave one, rather than the client's.
        let realname = (config.realname != "zirc-tui" && config.realname != config.nick)
            .then(|| config.realname.clone());
        let mut session = Self::with(app, Supervisor::new(config)).await?;
        session.web = web;
        session.realname = realname;
        let setname = session
            .supervisor
            .client()
            .is_some_and(|client| client.has_cap(zirc_core::chat::SETNAME_CAP));
        session.setname(setname);
        session.learn_role();
        Ok(session)
    }

    /// Connects for `app` as `supervisor` does, as [`Session::connect`].
    ///
    /// # Errors
    ///
    /// As [`Session::connect`].
    pub async fn with(app: App, mut supervisor: Supervisor) -> Result<Self, Error> {
        let client = supervisor.first().await?;
        let mut session = Self {
            app,
            supervisor,
            outbox: Vec::new(),
            zircd: None,
            answers: mpsc::unbounded_channel(),
            sessions: mpsc::unbounded_channel(),
            web: None,
            web_answers: mpsc::unbounded_channel(),
            realname: None,
        };
        session.up(&client);
        Ok(session)
    }

    /// Something to tell the connection that the network changed or the machine woke.
    #[must_use]
    pub fn nudge(&self) -> Nudge {
        self.supervisor.nudge()
    }

    /// Takes in a key, saying what comes next.
    pub fn key(&mut self, key: KeyEvent) -> Step {
        match self.app.key(key) {
            Action::Send(lines) => {
                self.send(lines);
                Step::Carry
            }
            Action::Quit(lines) => {
                self.send(lines);
                Step::Quit
            }
            Action::Zircd(request) => {
                self.ask_zircd(request);
                Step::Carry
            }
            Action::Org(name) => Step::Org(name),
            Action::Sessions => {
                self.ask_sessions();
                Step::Carry
            }
            Action::Guests { request, done } => {
                self.ask_web(request, done);
                Step::Carry
            }
            Action::Open(url) => {
                self.open(&url);
                Step::Carry
            }
            Action::Press(press) => {
                self.press(press);
                Step::Carry
            }
        }
    }

    /// Takes in a click or a turn of the wheel at `target`.
    pub fn pointer(&mut self, kind: MouseEventKind, target: Option<Target>) {
        match self.app.pointer(kind, target) {
            Action::Send(lines) | Action::Quit(lines) => self.send(lines),
            Action::Zircd(request) => self.ask_zircd(request),
            Action::Sessions => self.ask_sessions(),
            Action::Guests { request, done } => self.ask_web(request, done),
            Action::Open(url) => self.open(&url),
            Action::Press(press) => self.press(press),
            Action::Org(_) => {}
        }
    }

    /// Waits for the next thing from the network, a line or the connection dropping or
    /// coming back, and takes it in. Safe to cancel, as in `select!`.
    pub async fn network(&mut self) {
        let event = tokio::select! {
            event = self.supervisor.next() => event,
            Some(answer) = self.answers.1.recv() => {
                self.app.zircd_answered(answer);
                return;
            }
            Some((listed, at)) = self.sessions.1.recv() => {
                self.app.sessions_answered(listed, at);
                return;
            }
            Some(answer) = self.web_answers.1.recv() => {
                self.app.changed();
                match answer {
                    Web::Role(role) => self.app.chat.set_role(role.as_str()),
                    Web::Kept(guests) => self.app.chat.set_kept_guests(guests),
                    Web::Done(answer) => self.app.guests_answered(answer),
                    Web::Pressed(press, answer) => self.app.press_answered(&press, answer),
                    Web::Renamed(handle) => {
                        self.follow(&handle);
                        self.app.renamed_elsewhere(&handle);
                    }
                }
                return;
            }
        };
        match event {
            Event::Connecting => self.app.connecting(),
            Event::Connected(client) => self.up(&client),
            Event::Line(line) => {
                let mut lines = vec![line];
                // Take in what else has arrived before drawing again.
                lines.extend(std::iter::from_fn(|| self.supervisor.try_line()));
                for line in lines {
                    let replies = self.app.handle(line);
                    self.send(replies);
                }
                self.keep_received_keys();
                // Scrolled back to the top of what's held, a page of history that came in
                // short of two screens asks for the one before it.
                let older = self.app.older_if_near_top();
                self.send(older);
            }
            Event::Offline(why) => self.app.disconnected(&why),
            Event::Refused(why) => {
                self.app.disconnected(&why);
                self.renew();
            }
        }
        self.app.connection = self.supervisor.status().clone();
    }

    /// Whether another organisation is shown instead of this one, as [`App::hide`] says.
    pub fn hide(&mut self, hidden: bool) {
        let lines = self.app.hide(hidden);
        self.send(lines);
    }

    /// Leaves the channel called `channel`, as zirc-chat's [`zirc_core::chat::Chat::part`]
    /// does.
    pub fn leave(&mut self, channel: &str) {
        let lines = self.app.chat.part(channel, None);
        self.send(lines);
    }

    /// Waits a moment for the server to take a `QUIT`, so it's seen as leaving rather than
    /// timing out.
    pub async fn finish(self) {
        self.supervisor.finish(QUIT_GRACE).await;
    }

    fn up(&mut self, client: &zirc_irc::Client) {
        let mut lines = self.app.connected(connection::link(client));
        self.app.connection = self.supervisor.status().clone();
        // What waited while offline goes after registering again.
        lines.append(&mut self.outbox);
        self.send(lines);
        self.setname(client.has_cap(zirc_core::chat::SETNAME_CAP));
    }

    /// Opens `url` in the person's browser, saying so.
    fn open(&mut self, url: &str) {
        zirc_core::login::open_browser(url);
        self.app.note(format!("opened {url} in your browser"));
    }

    /// Says the person's name with `SETNAME`, if the server takes it (`allowed`).
    fn setname(&mut self, allowed: bool) {
        if let Some(realname) = self.realname.clone().filter(|_| allowed) {
            self.send(vec![Line::new("SETNAME", [realname])]);
        }
    }

    /// Asks zircd which repositories are checked out here, so files named in messages open
    /// in them ([`zirc_core::editor::checkouts`]). Quietly: without a zircd, files link to
    /// their forge.
    pub fn ask_checkouts(&self) {
        let Some(socket) = self.zircd.clone() else {
            return;
        };
        let answers = self.answers.0.clone();
        tokio::spawn(async move {
            if let Ok(answer @ Response::Repos { .. }) =
                zirc_core::ipc::request(&socket, &Request::RepoList).await
            {
                let _ = answers.send(Ok(answer));
            }
        });
    }

    /// Asks zircd `request` without waiting: [`Session::network`] hands the app its answer.
    fn ask_zircd(&mut self, request: Request) {
        let Some(socket) = self.zircd.clone() else {
            self.app
                .zircd_answered(Err("there's no zircd on this machine to ask".into()));
            return;
        };
        let answers = self.answers.0.clone();
        tokio::spawn(async move {
            let answer = zirc_core::ipc::request(&socket, &request)
                .await
                .map_err(|err| err.to_string());
            let _ = answers.send(answer);
        });
    }

    /// Asks zirc.dev our role in the organisation, without waiting: [`Session::network`]
    /// tells the app whether we're an admin.
    fn learn_role(&self) {
        let (Some((client, account)), Some(site)) = (self.web.clone(), self.app.site.clone())
        else {
            return;
        };
        let answers = self.web_answers.0.clone();
        tokio::spawn(async move {
            let role = client
                .roles(&account)
                .await
                .ok()
                .flatten()
                .and_then(|roles| {
                    roles
                        .into_iter()
                        .find(|role| role.slug == site.slug)
                        .map(|role| role.role)
                });
            let Some(role) = role else {
                return;
            };
            let _ = answers.send(Web::Role(role));
            // An owner or admin sees every kept guest's room: asked now, and again every
            // few minutes while the app runs.
            while role.is_admin() {
                // Unanswered, the list stays as it was.
                if let Ok(guests) = client.kept_guests(&account, &site.slug).await
                    && answers.send(Web::Kept(guests)).is_err()
                {
                    return;
                }
                tokio::time::sleep(zirc_core::chat::KEPT_GUESTS_EVERY).await;
            }
        });
    }

    /// Makes `request` of zirc.dev's Guests API without waiting: [`Session::network`] hands
    /// the app `done`, or why it wasn't.
    /// Presses a card's button through zirc.dev without waiting: [`Session::network`] hands
    /// the app what came of it.
    fn press(&mut self, press: zirc_proto::card::Press) {
        let (Some((client, account)), Some(site)) = (self.web.clone(), self.app.site.clone())
        else {
            self.app.press_answered(
                &press,
                Err("there's no zirc.dev account to press it as".into()),
            );
            return;
        };
        let answers = self.web_answers.0.clone();
        tokio::spawn(async move {
            let answer = client
                .press(&account, &site.slug, &press)
                .await
                .map_err(|err| err.to_string());
            let _ = answers.send(Web::Pressed(press, answer));
        });
    }

    /// Asks zirc.dev, without waiting, what our account is now, after the server refused it:
    /// when the person changed their handle elsewhere, the sign-in kept here is renamed and
    /// [`Session::network`] connects as it. The sign-in may already have been renamed here.
    /// Keeps a key a teammate's device handed this one, in the store `zirc team-key` and the
    /// other apps read, for the next time too.
    fn keep_received_keys(&mut self) {
        let received = self.app.chat.take_received_keys();
        let Some(home) = self.app.site.as_ref().and_then(|site| site.home.clone()) else {
            return;
        };
        if received.is_empty() {
            return;
        }
        let secrets = zirc_core::secrets::system(&home);
        for key in received {
            let _ = zirc_core::sealed::Keys::new(secrets.as_ref()).add(&key);
        }
    }

    fn renew(&mut self) {
        let (Some(site), Some(sasl)) =
            (self.app.site.clone(), self.supervisor.config().sasl.clone())
        else {
            return;
        };
        let Some(home) = site.home else {
            return;
        };
        let answers = self.web_answers.0.clone();
        tokio::spawn(async move {
            let renewed = async {
                let path = crate::signin::path(&home);
                let Some(signin) = crate::signin::load_all(&path)?.get(&site.slug).cloned() else {
                    return Ok(None);
                };
                if signin.handle != sasl.account {
                    return Ok(Some(signin.handle));
                }
                let secrets = zirc_core::secrets::system(&home);
                let Some(later) = signin.renew(secrets.as_ref()).await? else {
                    return Ok(None);
                };
                crate::signin::save(&later, &path)?;
                anyhow::Ok(Some(later.handle))
            }
            .await;
            // Otherwise the connection keeps retrying as it was, and the next refusal asks again.
            if let Ok(Some(handle)) = renewed {
                let _ = answers.send(Web::Renamed(handle));
            }
        });
    }

    /// Connects as `handle` from now on, the account's new name: the nick follows when it was
    /// the account's, and zirc.dev is asked as it.
    fn follow(&mut self, handle: &str) {
        let mut config = self.supervisor.config().clone();
        let Some(sasl) = &mut config.sasl else {
            return;
        };
        if sasl.account == handle {
            return;
        }
        if config.nick == sasl.account {
            handle.clone_into(&mut config.nick);
        }
        handle.clone_into(&mut sasl.account);
        if let Some((_, account)) = &mut self.web {
            handle.clone_into(&mut account.name);
        }
        self.supervisor.set_config(config);
    }

    fn ask_web(&mut self, request: GuestRequest, done: String) {
        let Some((client, account)) = self.web.clone() else {
            self.app
                .guests_answered(Err("there's no zirc.dev account to ask it as".into()));
            return;
        };
        let answers = self.web_answers.0.clone();
        tokio::spawn(async move {
            let answer = client
                .send(&account, &request)
                .await
                .map(|()| done)
                .map_err(|err| err.to_string());
            let _ = answers.send(Web::Done(answer));
        });
    }

    /// Asks zircd what every session is doing, for the sessions panel.
    fn ask_sessions(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let Some(socket) = self.zircd.clone() else {
            let none = Err("there's no zircd on this machine to ask".to_owned());
            self.app.sessions_answered(none, now);
            return;
        };
        let org = self.app.org.clone();
        let answers = self.sessions.0.clone();
        tokio::spawn(async move {
            let listed = zirc_core::sessions::fetch(&socket, org.as_deref(), now)
                .await
                .map_err(|err| err.to_string());
            let _ = answers.send((listed, now));
        });
    }

    fn send(&mut self, lines: Vec<Line>) {
        let Some(client) = self.supervisor.client() else {
            // Offline: it waits, and goes once the connection's up again.
            self.outbox.extend(lines);
            return;
        };
        let mut failed = Vec::new();
        for line in lines {
            if let Err(err) = client.send(line) {
                failed.push(err.to_string());
            }
        }
        if let Some(err) = failed.first() {
            self.app.unsent(err);
        }
    }
}

impl AsRef<zirc_core::chat::Chat> for Session {
    fn as_ref(&self) -> &zirc_core::chat::Chat {
        &self.app.chat
    }
}
