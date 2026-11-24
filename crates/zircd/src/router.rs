//! zircd's front: socket requests reach the router, which passes each to the daemon of the
//! organisation it's for. A machine signed in to several organisations runs a daemon per
//! organisation, each with its own IRC connection and its own repositories, so nothing
//! about a repository reaches another organisation's server: the router decides which
//! daemon hears a request, from the organisation it names or the repository it's about.
//!
//! Requests about the whole machine, such as its status or the repositories it watches,
//! are asked of every daemon and their answers put together. A watch watches through
//! every daemon, its lines saying which organisation they come from when there are
//! several.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use zirc_proto::PROGRAM;

use anyhow::{Result, bail};
use tokio::sync::{mpsc, oneshot};
use zirc_core::ipc::{Addressed, OrgStatus, RESTARTING, Repo, Request, Response, Status};
use zirc_core::update::Version;
use zirc_core::which_org::{self, Choice, Unpicked};

use crate::daemon::Input;
use crate::git;

use crate::socket::{STOPPING, WATCH_BUFFER};

/// Something for the router to handle: what the socket and the updater ask.
pub enum Call {
    /// A request from a socket client, and where to send the answer.
    Request(Box<Addressed>, oneshot::Sender<Response>),
    /// A socket client that wants every delivery and overlap from now on, watching as the
    /// session named, if any.
    Watch(mpsc::Sender<Response>, Option<String>),
    /// The binaries were updated to this release, so it's time to restart.
    Updated(Version),
}

/// One organisation's daemon.
#[derive(Clone)]
struct Org {
    name: String,
    inputs: mpsc::UnboundedSender<Input>,
    /// Whether it's zirc.dev's public server, which watches no repositories.
    public: bool,
}

pub struct Router {
    orgs: Orgs,
    /// The requests being answered, each in a task of its own: one waiting on a busy
    /// daemon, as one restoring its repositories is, holds up no other (9 Nov 2026).
    requests: tokio::task::JoinSet<()>,
    /// Set once a client has asked zircd to stop, or it's restarting.
    pub stopping: bool,
    /// Set when it's to start again from its binary, as after an update.
    pub restarting: bool,
    /// The release it was updated to, when that's why it's restarting.
    updated: Option<Version>,
}

/// What a request that every organisation answers gets when there are none, as a
/// `config.toml` can't say: rather than zircd panicking.
fn no_organisation() -> Response {
    Response::Error {
        message: "zircd serves no organisation".to_owned(),
    }
}

/// The organisations' daemons, as every request's task asks them.
#[derive(Clone, Default)]
struct Orgs(Arc<Vec<Org>>);

impl Router {
    pub fn new() -> Self {
        Self {
            orgs: Orgs::default(),
            requests: tokio::task::JoinSet::new(),
            stopping: false,
            restarting: false,
            updated: None,
        }
    }

    /// Adds the organisation `name`, whose daemon takes `inputs`.
    pub fn add(&mut self, name: String, public: bool, inputs: mpsc::UnboundedSender<Input>) {
        Arc::make_mut(&mut self.orgs.0).push(Org {
            name,
            inputs,
            public,
        });
    }

    /// Handles `call`: a request in a task of its own, unless it stops or restarts zircd.
    pub fn handle(&mut self, call: Call) {
        match call {
            Call::Updated(version) => {
                tracing::info!("updated to {PROGRAM} {version}: restarting");
                self.updated = Some(version);
                self.stopping = true;
                self.restarting = true;
            }
            Call::Watch(heard, session) => self.orgs.watch(&heard, session.as_ref()),
            Call::Request(addressed, answer) => {
                let Addressed { request, org } = *addressed;
                match request {
                    Request::Stop => {
                        tracing::info!("stopping, as asked");
                        self.stopping = true;
                        let _ = answer.send(Response::Done);
                    }
                    Request::Restart => {
                        tracing::info!("restarting, as asked");
                        self.stopping = true;
                        self.restarting = true;
                        let _ = answer.send(Response::Done);
                    }
                    request => {
                        let orgs = self.orgs.clone();
                        self.requests
                            .spawn(async move { orgs.request(request, org, answer).await });
                    }
                }
            }
        }
    }

    /// Waits for a request's task to end, or forever when none is running.
    pub async fn answered(&mut self) {
        if self.requests.join_next().await.is_none() {
            std::future::pending::<()>().await;
        }
    }

    /// Waits up to `within` for the requests being answered, as before a restart, when
    /// what's waiting gets the old zircd's answer.
    pub async fn settle(&mut self, within: Duration) {
        let _ = tokio::time::timeout(within, async {
            while self.requests.join_next().await.is_some() {}
        })
        .await;
    }

    /// Tells every daemon to stop, or to restart, and to answer what's waiting first.
    pub fn stop(&self) {
        for org in self.orgs.0.iter() {
            let input = if let Some(version) = self.updated {
                Input::Updated(version)
            } else {
                let request = if self.restarting {
                    Request::Restart
                } else {
                    Request::Stop
                };
                Input::Request(request, oneshot::channel().0)
            };
            let _ = org.inputs.send(input);
        }
    }
}

impl Orgs {
    fn several(&self) -> bool {
        self.0.len() > 1
    }

    /// Every organisation as [`zirc_core::which_org`] picks among them: the public server isn't
    /// the machine's own.
    fn choices(&self) -> Vec<Choice<'_>> {
        self.0
            .iter()
            .map(|org| Choice {
                name: &org.name,
                own: !org.public,
            })
            .collect()
    }

    async fn request(
        &self,
        request: Request,
        org: Option<String>,
        answer: oneshot::Sender<Response>,
    ) {
        let response = match request {
            Request::Status => self.status().await,
            Request::RepoList => self.repo_list().await,
            // A machine-wide setting, which every organisation's daemon keeps: who may query
            // its agents, or what a session is called.
            Request::Queries { .. } | Request::Name { .. } => {
                let mut answers = Vec::new();
                for at in 0..self.0.len() {
                    answers.push(self.ask(at, request.clone()).await);
                }
                answers
                    .iter()
                    .find(|answer| matches!(answer, Response::Error { .. }))
                    .or(answers.first())
                    .map_or_else(no_organisation, Clone::clone)
            }
            // The daemon that heard the query answers it.
            Request::Reply { .. } if org.is_none() && self.several() => {
                let mut answered = Response::Error {
                    message: "no query has that id".to_owned(),
                };
                for at in 0..self.0.len() {
                    answered = self.ask(at, request.clone()).await;
                    if !matches!(answered, Response::Error { .. }) {
                        break;
                    }
                }
                answered
            }
            request => match self.route(&request, org.as_deref()).await.and_then(|at| {
                if matches!(request, Request::RepoAdd { .. }) && self.0[at].public {
                    bail!(
                        "{} is zirc.dev's public server, where repositories aren't watched: \
                         what's said about them there would be public",
                        self.0[at].name
                    );
                }
                Ok(at)
            }) {
                Ok(at) => {
                    if self.0[at]
                        .inputs
                        .send(Input::Request(request, answer))
                        .is_err()
                    {
                        tracing::warn!("{}'s daemon is gone", self.0[at].name);
                    }
                    return;
                }
                Err(err) => Response::Error {
                    message: format!("{err:#}"),
                },
            },
        };
        let _ = answer.send(response);
    }

    /// Asks the daemon at `at`, and waits for its answer.
    async fn ask(&self, at: usize, request: Request) -> Response {
        let (answer, answered) = oneshot::channel();
        let _ = self.0[at].inputs.send(Input::Request(request, answer));
        answered.await.unwrap_or_else(|_| Response::Error {
            message: STOPPING.to_owned(),
        })
    }

    /// Which daemon a request is for, by [`zirc_core::which_org::pick`]: the organisation it
    /// names, or the one the repository it's about belongs to, or the machine's only own one.
    /// Never one the repository doesn't belong to.
    async fn route(&self, request: &Request, org: Option<&str>) -> Result<usize> {
        let choices = self.choices();
        let named = org
            .map(|name| which_org::pick(&choices, Some(name), None))
            .transpose()?;
        let repo = repo_of(request);
        let holder = match repo {
            Some(path) => self.holder(path).await,
            None => None,
        };
        if let (Some(named), Some((holder, root))) = (named, &holder)
            && named != *holder
        {
            bail!(
                "{} belongs to {}, not {}: a repository is in one organisation only",
                root.display(),
                self.0[*holder].name,
                self.0[named].name
            );
        }
        if let Some(at) = named {
            return Ok(at);
        }
        let repo_org = holder.as_ref().map(|(at, _)| self.0[*at].name.as_str());
        match which_org::pick(&choices, None, repo_org) {
            Ok(at) => Ok(at),
            Err(several @ Unpicked::Several(_)) => match (request, repo) {
                (Request::RepoAdd { path, .. }, _) => self.infer(path).await,
                (_, Some(path)) => bail!("{} isn't being watched", path.display()),
                (_, None) => Err(several.into()),
            },
            Err(unpicked) => Err(unpicked.into()),
        }
    }

    /// The daemon watching the repository containing `path`, and its root, if one is: asked
    /// as [`Input::Holds`], which a daemon answers even while it's restoring.
    async fn holder(&self, path: &Path) -> Option<(usize, std::path::PathBuf)> {
        let root = git::root(path).await.ok()?;
        for (at, org) in self.0.iter().enumerate() {
            let (answer, answered) = oneshot::channel();
            if org.inputs.send(Input::Holds(root.clone(), answer)).is_ok()
                && answered.await == Ok(true)
            {
                return Some((at, root));
            }
        }
        None
    }

    /// The organisation a repository no organisation watches yet belongs to: the one that
    /// watches another clone with the same git remote, when only one does.
    async fn infer(&self, path: &Path) -> Result<usize> {
        let root = git::root(path).await?;
        let mut orgs = Vec::new();
        if let Some(remote) = git::remote_url(&root).await {
            for repo in self.repos().await {
                if git::remote_url(&repo.path).await.as_ref() == Some(&remote)
                    && let Some(at) = self
                        .0
                        .iter()
                        .position(|it| repo.org.as_ref() == Some(&it.name))
                {
                    orgs.push(at);
                }
            }
        }
        orgs.sort_unstable();
        orgs.dedup();
        match orgs.as_slice() {
            [at] => Ok(*at),
            _ => bail!(
                "this machine is signed in to several organisations ({}): say which {} \
                 belongs to with --org",
                self.names(&which_org::implied(&self.choices())),
                root.display()
            ),
        }
    }

    /// The names of the organisations at `orgs`.
    fn names(&self, orgs: &[usize]) -> String {
        orgs.iter()
            .map(|&at| self.0[at].name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Every daemon's repositories, sorted by path.
    async fn repos(&self) -> Vec<Repo> {
        let mut repos = Vec::new();
        for at in 0..self.0.len() {
            if let Response::Repos { repos: theirs } = self.ask(at, Request::RepoList).await {
                repos.extend(theirs);
            }
        }
        repos.sort_by(|a, b| a.path.cmp(&b.path));
        repos
    }

    async fn repo_list(&self) -> Response {
        Response::Repos {
            repos: self.repos().await,
        }
    }

    /// The first organisation's status, with every organisation's connection when there
    /// are several, and every watch running.
    async fn status(&self) -> Response {
        let mut statuses = Vec::new();
        for at in 0..self.0.len() {
            match self.ask(at, Request::Status).await {
                Response::Status(status) => statuses.push((at, status)),
                other => return other,
            }
        }
        let mut watching: Vec<Option<String>> = statuses
            .iter()
            .flat_map(|(_, status)| status.watching.iter().cloned())
            .collect();
        watching.sort();
        watching.dedup();
        let orgs = if self.several() {
            statuses
                .iter()
                .map(|(at, status)| OrgStatus {
                    name: self.0[*at].name.clone(),
                    connected: status.connected,
                    server: status.server.clone(),
                    nick: status.nick.clone(),
                    channels: status.channels.clone(),
                    problem: status.problem.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        if statuses.is_empty() {
            return no_organisation();
        }
        let (_, first) = statuses.swap_remove(0);
        // The longest any organisation's daemon was busy: each has its own task's turn.
        let busy_ms = statuses
            .iter()
            .filter_map(|(_, it)| it.busy_ms)
            .chain(first.busy_ms)
            .max();
        Response::Status(Status {
            watching,
            orgs,
            busy_ms,
            ..first
        })
    }

    /// Watches through every organisation's daemon, passing on what each hears.
    fn watch(&self, heard: &mpsc::Sender<Response>, session: Option<&String>) {
        for (at, org) in self.0.iter().enumerate() {
            let (theirs, from) = mpsc::channel(WATCH_BUFFER);
            if org
                .inputs
                .send(Input::Watch(theirs, session.cloned()))
                .is_err()
            {
                continue;
            }
            let label = self.several().then(|| org.name.clone());
            tokio::spawn(forward(from, heard.clone(), label, at == 0));
        }
    }
}

/// Passes on what one organisation's daemon tells a watch, until either goes away. With
/// several organisations, `label` names this one.
async fn forward(
    mut from: mpsc::Receiver<Response>,
    to: mpsc::Sender<Response>,
    label: Option<String>,
    first: bool,
) {
    loop {
        let response = tokio::select! {
            response = from.recv() => response,
            () = to.closed() => return,
        };
        let Some(response) = response else {
            return;
        };
        if let Response::Error { message } = &response {
            // Every daemon says it's restarting: the watch hears it once.
            if message == RESTARTING && !first {
                return;
            }
            // One organisation's trouble, such as its account being revoked, doesn't end a
            // watch of the others: `zirc status` says what it is.
            if let Some(org) = &label
                && message != RESTARTING
            {
                tracing::warn!("{org}: a watch stopped hearing it: {message}");
                return;
            }
        }
        if to.send(labelled(response, label.as_ref())).await.is_err() {
            return;
        }
    }
}

/// A message or a direct message, saying which organisation's server it came from, if
/// `label` names one.
fn labelled(response: Response, label: Option<&String>) -> Response {
    match (response, label) {
        (Response::Delivery(mut delivery), Some(org)) => {
            delivery.org = Some(org.clone());
            Response::Delivery(delivery)
        }
        (Response::Direct(mut direct), Some(org)) => {
            direct.org = Some(org.clone());
            Response::Direct(direct)
        }
        (response, _) => response,
    }
}

/// The repository a request is about, if it's about one.
fn repo_of(request: &Request) -> Option<&Path> {
    match request {
        Request::RepoAdd { path, .. } | Request::RepoRemove { path, .. } => Some(path),
        Request::Post { repo, .. }
        | Request::SealedPost { repo, .. }
        | Request::Emit { repo, .. }
        | Request::Workflow { repo, .. }
        | Request::Intend { repo, .. }
        | Request::Join { repo, .. }
        | Request::Leave { repo, .. }
        | Request::Decide { repo, .. }
        | Request::SetState { repo, .. }
        | Request::Report { repo, .. }
        | Request::CloseReport { repo, .. }
        | Request::React { repo, .. } => Some(repo),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(router: &mut Router, request: Request) -> oneshot::Receiver<Response> {
        let (answer, answered) = oneshot::channel();
        let addressed = Addressed { request, org: None };
        router.handle(Call::Request(Box::new(addressed), answer));
        answered
    }

    async fn answer(router: &mut Router, request: Request) -> Response {
        ask(router, request).await.unwrap()
    }

    /// A daemon that answers how it is at once and holds everything else, as one busy
    /// restoring its repositories does.
    fn busy_daemon() -> mpsc::UnboundedSender<Input> {
        let (inputs, mut received) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Some(input) = received.recv().await {
                match input {
                    Input::Request(Request::Status, answer) => {
                        let status = serde_json::from_str(
                            r#"{"connected":true,"server":"irc.example:6697","channels":[]}"#,
                        )
                        .unwrap();
                        let _ = answer.send(Response::Status(status));
                    }
                    input => held.push(input),
                }
            }
        });
        inputs
    }

    #[tokio::test]
    async fn a_request_waiting_on_a_busy_daemon_holds_up_no_other() {
        let mut router = Router::new();
        router.add("zirc".into(), false, busy_daemon());
        router.add("acme".into(), false, busy_daemon());
        // Asked of every organisation, and held by both.
        let mut who = ask(
            &mut router,
            Request::Name {
                session: "cc-1".into(),
                name: Some("held".into()),
                tool: None,
                avatar: None,
                link: None,
                title: None,
            },
        );
        let status =
            tokio::time::timeout(Duration::from_secs(5), answer(&mut router, Request::Status))
                .await
                .expect("status answered while another request waits");
        assert!(matches!(status, Response::Status(_)), "{status:?}");
        assert!(who.try_recv().is_err(), "the held request is still waiting");
    }

    #[tokio::test]
    async fn with_no_organisation_it_answers_an_error_rather_than_panicking() {
        let mut router = Router::new();
        let no = no_organisation();
        assert_eq!(answer(&mut router, Request::Status).await, no);
        let name = serde_json::from_str(r#"{"op":"name","session":"cc-1","name":"one"}"#).unwrap();
        assert_eq!(answer(&mut router, name).await, no);
    }
}
