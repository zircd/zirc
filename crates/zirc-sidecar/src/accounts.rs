//! The tenant's account endpoint, which zirc.dev's web app uses to manage the server's IRC
//! accounts: people's and machines' (docs/accounts.md). It runs in the sidecar beside Ergo,
//! on the pod network, behind a per-tenant token (secret `ergo-accounts`) and a network
//! policy that lets in only the environment's web app.
//!
//! - `PUT /v1/accounts/<name>` `{"passphrase"}`: registers the account (201), or sets the
//!   passphrase of one that exists (200). It's in force when this answers.
//! - `DELETE /v1/accounts/<name>`: erases the account (`NickServ ERASE`): its connections are
//!   dropped, its passphrase and every record of it go, and its name is free to register
//!   again (200, also when there was no such account). zirc.dev does this when a handle
//!   changes, a member leaves and a machine is revoked: accounts are never locked
//!   (docs/accounts.md).
//! - `POST /v1/accounts/<name>/suspend`: it can't log in, and its connections are dropped
//!   (200). `DELETE` lifts that (200). For banning guests only.
//! - `PUT /v1/accounts/<name>/keys/<key>` `{"label"}`: lets a device sign in to the account
//!   with its key (`crate::signin`; `<key>` is its public key, `zirc_proto::device_key`): 201
//!   when it's new, and the person is told by a message from the server, 200 when it was
//!   there (its label set): `{"key", "active"}`. Where the server doesn't take device keys
//!   (`accounts.deviceKeys` off), 200 with `active` false, and nothing is kept. 404 for no such account, 409 `too-many` past
//!   `signin::MAX_KEYS`. `DELETE` takes it away: `{"removed"}` (200); connections it made
//!   stay until they end. `GET /v1/accounts/<name>/keys`: `{"keys": [{"key", "label",
//!   "addedAt", "lastUsed"}]}`. Erasing the account takes its keys too.
//! - `GET /v1/accounts`: `{"accounts": [{"name", "connected", "sessions", "lastSeen"}]}`,
//!   `lastSeen` when it was last seen connected, sampled every few minutes over 90 days, or
//!   `null` for never.
//! - `PUT /v1/accounts/<name>/profile` `{"name", "avatar"}`: the account's name and picture
//!   (an https URL, or `null`), as IRC METADATA `display-name` and `avatar`
//!   (`crate::profiles`): `{"applied"}`, false when it's a machine that isn't on, which gets
//!   them as it comes on.
//! - `GET /v1/messages?q=<text>&channel=<#channel>`: the newest channel messages containing
//!   the text, for the organisation's owners to find one to remove: `{"messages": [{"msgid",
//!   "channel", "time", "nick", "account", "text"}]}`. Never direct messages.
//! - `DELETE /v1/messages/<msgid>`: removes a channel message from the history, telling the
//!   clients that take `draft/message-redaction`, and scrubs Ergo's disk of it
//!   (`crate::trim`): `{"removed", "channel", "scrubbed"}` (200).
//!
//! - `POST /v1/say` `{"nick", "channel", "kind", "text", "data"}`: posts a custom event into
//!   a channel as an integration, `zirc-<name>` (`crate::say`), for what zirc.dev posts as one:
//!   `{"posted"}` (200), 400 for what can't be said, 502 when the server won't take it.
//!
//! - `GET /v1/activity?since=<nanoseconds>&after=<id>&limit=<n>`: the organisation's activity
//!   (`crate::activity`), oldest first, for zirc.dev's dashboard of how its team works
//!   together: `{"rows": [{"id", "nanotime", "kind", "channel", "account", "session", "level",
//!   "task", "data", …}]}`, metadata only, never what anyone said; page with the last `id`.
//!   With `&collapse=true`, a session's working set said again in the same hour is left out.
//!
//! - `GET /v1/push`: `{"vapidKey"}`, the key the pusher signs its pushes with
//!   (`crate::pusher`), for zirc.dev to check the pushes it relays to phones; `null` while
//!   push is off.
//! - `GET /v1/guests`: the organisation's guests (`crate::guests`), most recently there first:
//!   `{"guests": [{"name", "connected", "lastSeen", "channels"}]}`.
//! - `GET /v1/guests/unread?text=true|false`: what staff wrote to each guest with an account
//!   while they were away and that hasn't been mailed, for zirc.dev's reply emails: `{"unread":
//!   [{"guest", "lines": [{"msgid", "time", "from", "text"}]}]}`, at most 20 lines a guest, each
//!   cut at 2000 characters; with `text=false`, without the text. Never staff's lines to each
//!   other, and never logged. `POST /v1/guests/mailed` `{"msgids"}` marks lines mailed, so
//!   they're never sent twice (204).
//! - `POST /v1/guests/<name>/forget`: deletes everything of theirs from the history, both sides
//!   of their conversations, and ends their connection: `{"forgotten", "messages"}`. Fourteen
//!   days after a guest was last there, the sidecar does it by itself.
//! - `GET /v1/server`: how the server is now, for its Server tab: `{"version", "startedAt",
//!   "connections": {"people", "agents", "other"}, "storage": {"usedBytes", "sizeBytes"}}`.
//!   People and agents are those connected now; `other`, every other client: guests, and
//!   whoever has only the gate password.
//! - `GET /v1/channels`: the server's channels for its members' dashboard, busiest first:
//!   `{"channels": [{"name", "users", "messagesLastHour"}]}` (`crate::exporter::channels`).
//!   Read-only. Channels' names are served from here, never put in metrics: nothing about a
//!   conversation goes to Google.
//!
//! The same content routes, with people's own tokens rather than this endpoint's, are served at
//! the server's host name beside the guest gateway, where the server's desired state turns
//! them on ([`content`]).
//!
//! Errors are `{"error", "message"}`: 400 `invalid`, 401, 404 `not-found`, 409 `reserved`
//! (a name once unregistered rather than erased, which Ergo doesn't give out again), 502 `ergo`, 503 `starting`
//! (Ergo is still starting: try again in a moment). Passphrases are never logged or echoed.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::ergo::{Api, Operator, OperatorError};
use crate::history::{self, Conversation, Viewer};
use crate::{gateway, guests, profiles, say, signin, teams, trim, usage};
use zirc_proto::colleagues::Colleagues;
use zirc_proto::desired::Visibility;

pub mod content;

// The sidecar flattens this beside the exporter's `Options`: each needs a group of its own.
#[derive(Clone, Debug, clap::Args)]
#[group(id = "accounts")]
/// The sidecar's options for its account endpoint, flattened beside the others.
pub struct Options {
    /// Where to serve the account endpoint.
    #[arg(long, default_value = "0.0.0.0:9101")]
    pub accounts_listen: SocketAddr,

    /// The token callers must present, from the tenant's `ergo-accounts` secret.
    #[arg(long, env = "ACCOUNTS_TOKEN", hide_env_values = true)]
    pub accounts_token: String,

    /// Ergo's loopback IRC listener, for the operator.
    #[arg(long, default_value = "127.0.0.1:6668")]
    pub irc: SocketAddr,

    /// The operator's password, from the tenant's `ergo-oper` secret.
    #[arg(long, env = "OPER_PASSWORD", hide_env_values = true)]
    pub oper_password: String,

    /// The gate password, while the server has one.
    #[arg(long, env = "GATE_PASSWORD", hide_env_values = true)]
    pub gate_password: Option<String>,

    /// The token the controller presents to set the server's teams (`PUT /v1/teams`), from
    /// the tenant's `ergo-controller` secret. Without it, teams can't be set.
    #[arg(long, env = "CONTROLLER_TOKEN", hide_env_values = true)]
    pub controller_token: Option<String>,

    /// Where guests of the public channels connect (`crate::gateway`).
    #[arg(long, default_value = "0.0.0.0:9102")]
    pub guest_listen: SocketAddr,

    /// The server's host name, which devices' tokens must be for (`crate::signin`). Without
    /// it, no device key signs in.
    #[arg(long, env = "SERVER_NAME")]
    pub server_name: Option<String>,

    /// Where to answer Ergo's logins; beside its history when left out.
    #[arg(long)]
    pub auth_socket: Option<PathBuf>,

    /// Who sends people's pushes, as their VAPID signature says: zirc.dev's address, which
    /// relays them to phones. Without it, nothing is pushed (`crate::pusher`).
    #[arg(long, env = "PUSH_SUBSCRIBER")]
    pub push_subscriber: Option<String>,
}

struct Ergo {
    api: Api,
    operator: Operator,
    token: String,
    controller_token: Option<String>,
    /// The owners the controller last asked for, and the channels they have ops in.
    owners: tokio::sync::Mutex<Owners>,
    /// Whose each machine is, as zirc.dev says.
    machines: crate::machines::Machines,
    /// The invite-only team channels the controller last asked for, to let a new account in
    /// at once if it's one of theirs ([`admit`]).
    invite_only: tokio::sync::Mutex<Vec<teams::Channel>>,
    /// Ergo's loopback listener and its gate password, where integrations connect to say
    /// something (`crate::say`).
    irc: SocketAddr,
    gate: Option<String>,
    /// Posts being made now: a few at once, and the rest told to come back.
    says: tokio::sync::Semaphore,
    /// Ergo's history database.
    history: PathBuf,
    /// When each account last had a client connected (`usage`).
    seen: tokio::sync::Mutex<usage::Seen>,
    /// Who may read what, as the controller last said (`history`); until it says, each
    /// person only their own.
    visibility: tokio::sync::Mutex<Option<Visibility>>,
    /// Searches running now: a few at once, and the rest told to come back.
    searches: tokio::sync::Semaphore,
    /// The public channels' guests (`crate::gateway`).
    gateway: Arc<gateway::Gateway>,
    /// What's been mailed to guests (`crate::guests`).
    mailed: tokio::sync::Mutex<guests::Mailed>,
    /// People's and machines' names and pictures (`crate::profiles`).
    profiles: tokio::sync::Mutex<profiles::Profiles>,
    /// Devices' keys, and the login log (`crate::signin`).
    signin: Arc<signin::Signin>,
    /// The content API's settings, as the controller last said; `None` while it's off
    /// ([`content`]).
    content: tokio::sync::Mutex<Option<teams::Content>>,
    /// The keys zirc.dev signs browser logins with, as the controller last said (PEM).
    token_keys: tokio::sync::Mutex<Vec<String>>,
    /// The content API's access log ([`content::Access`]).
    access: signin::Log<content::Access>,
    /// What members' agents may do with other members and theirs, as last said in `#ops`
    /// since the sidecar started ([`tell_colleagues`]).
    colleagues: tokio::sync::Mutex<Option<Colleagues>>,
    /// The team's own hints (`crate::hints`).
    hints: crate::hints::Store,
    /// Whether the pusher runs (`crate::pusher`).
    pushing: bool,
}

#[derive(Default)]
struct Owners {
    accounts: BTreeSet<String>,
    done: BTreeSet<String>,
}

/// How often the sidecar looks for channels made since, to give the owners ops in.
const SWEEP: Duration = Duration::from_secs(30);

/// Serves the account endpoint until the process ends, and meanwhile keeps the people's
/// accounts made before it set them always-on ([`keep_people_on`]).
///
/// # Errors
///
/// When it can't listen.
pub async fn run(options: Options, api: Api, history: PathBuf) -> std::io::Result<()> {
    let listen = options.accounts_listen;
    tokio::spawn(keep_people_on(api.clone()));
    let ergo = ergo(&options, api, history);
    tokio::spawn(sweep(ergo.clone()));
    tokio::spawn(note_connected(ergo.clone()));
    tokio::spawn(forget_guests(ergo.clone()));
    tokio::spawn(keep_profiles(ergo.clone()));
    tokio::spawn(push_visitors(ergo.clone()));
    tokio::spawn(suggest(ergo.clone()));
    // The recorder of machines' activity records, beside the history (crate::recorder).
    let recorder = crate::recorder::Recorder {
        host: ergo.irc.ip().to_string(),
        port: ergo.irc.port(),
        gate: ergo.gate.clone(),
        token: ergo.token.clone(),
        history: ergo.history.clone(),
    };
    let keeper = ergo.clone();
    tokio::spawn(recorder.run(move |reset| {
        let ergo = keeper.clone();
        async move {
            integration_account(&ergo, zirc_proto::activity::RECORDER, reset)
                .await
                .is_ok()
        }
    }));
    // What reaches people's phones, decided here (crate::pusher), where pushes go somewhere.
    if let Some(subscriber) = options.push_subscriber.clone() {
        let pusher = crate::pusher::Pusher {
            host: ergo.irc.ip().to_string(),
            port: ergo.irc.port(),
            gate: ergo.gate.clone(),
            token: ergo.token.clone(),
            history: ergo.history.clone(),
            api: ergo.api.clone(),
            subscriber,
            delay: crate::pusher::DELAY,
        };
        let keeper = ergo.clone();
        tokio::spawn(pusher.run(move |reset| {
            let ergo = keeper.clone();
            async move {
                integration_account(&ergo, zirc_proto::push::PUSHER, reset)
                    .await
                    .is_ok()
            }
        }));
    }
    // The server's workflows, each as its own nick (crate::workflows), where the controller
    // turns them on. An account that exists is never changed for one: a nick that's someone
    // else's is refused.
    let workflows = crate::workflows::Host {
        host: ergo.irc.ip().to_string(),
        port: ergo.irc.port(),
        gate: ergo.gate.clone(),
        token: ergo.token.clone(),
        history: ergo.history.clone(),
        machines: ergo.machines.clone(),
        gateway: ergo.gateway.clone(),
        admins: {
            let ergo = ergo.clone();
            Box::new(move || {
                let ergo = ergo.clone();
                Box::pin(async move {
                    let mut people = ergo.owners.lock().await.accounts.clone();
                    if let Some(content) = ergo.content.lock().await.as_ref() {
                        people.extend(content.admins.iter().cloned());
                    }
                    people
                })
            })
        },
    };
    let keeper = ergo.clone();
    tokio::spawn(workflows.run(move |nick| {
        let ergo = keeper.clone();
        // Never a name the server knows as someone's: only zirc.dev registers people's and
        // machines' accounts (`zirc_core::agents::standing` rests on it).
        async move {
            !someones(&ergo, &nick).await && integration_account(&ergo, &nick, false).await.is_ok()
        }
    }));
    // Ergo's logins: when the socket can't be had, passphrases still work, as Ergo checks
    // them itself when the sidecar doesn't answer, and everything else here goes on.
    let socket = options
        .auth_socket
        .clone()
        .unwrap_or_else(|| signin::socket_beside(&ergo.history));
    let answering = ergo.signin.clone();
    tokio::spawn(async move {
        if let Err(error) = signin::serve(socket, answering).await {
            warn!(%error, "can't answer Ergo's logins");
        }
    });
    // The listener the edge sends the server's host name to: guests, and the content API.
    let guests = tokio::net::TcpListener::bind(options.guest_listen).await?;
    let public = gateway::router(ergo.gateway.clone()).merge(content::router(ergo.clone()));
    tokio::spawn(axum::serve(guests, public).into_future());
    axum::serve(tokio::net::TcpListener::bind(listen).await?, routes(ergo)).await
}

/// Every so often, tells the owners of visitors that waited out the gap (`crate::visitors`).
async fn push_visitors(ergo: Arc<Ergo>) {
    let mut every = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        every.tick().await;
        ergo.gateway.push_due();
    }
}

/// Every so often, gives the owners ops in the channels made since (`teams::sweep`).
async fn sweep(ergo: Arc<Ergo>) {
    let mut every = tokio::time::interval(SWEEP);
    loop {
        every.tick().await;
        let mut owners = ergo.owners.lock().await;
        match teams::sweep(&ergo.api, &ergo.operator, &owners.accounts, &owners.done).await {
            Ok(outcome) => {
                // Never which channel: the sidecar's log goes to Cloud Logging, and nothing
                // about a conversation goes to Google.
                for (channel, problem) in outcome {
                    match problem {
                        None => {
                            info!("owners given ops");
                            owners.done.insert(channel);
                        }
                        Some(problem) => warn!(problem, "couldn't give owners ops"),
                    }
                }
            }
            Err(failure) => warn!(%failure, "couldn't look for new channels"),
        }
    }
}

/// Every so often, notes who's connected, for the usage the controller asks for.
async fn note_connected(ergo: Arc<Ergo>) {
    let mut every = tokio::time::interval(usage::SAMPLE);
    loop {
        every.tick().await;
        match sessions(&ergo.api).await {
            Ok(accounts) => {
                let connected = accounts.iter().filter(|(_, n)| *n > 0).map(|(a, _)| a);
                let now = jiff::Timestamp::now().as_second();
                ergo.seen.lock().await.note(connected, now);
            }
            Err(failure) if failure.is_connect() => {}
            Err(failure) => warn!(%failure, "couldn't note who's connected"),
        }
    }
}

/// The endpoint's routes, `/healthz` unauthenticated.
pub fn router(options: &Options, api: Api, history: PathBuf) -> Router {
    routes(ergo(options, api, history))
}

fn ergo(options: &Options, api: Api, history: PathBuf) -> Arc<Ergo> {
    let gateway = gateway::Gateway::new(
        options.irc,
        options.gate_password.clone().filter(|p| !p.is_empty()),
        Some(history.clone()),
        Some((api.clone(), options.oper_password.clone())),
    );
    Arc::new(Ergo {
        api,
        operator: Operator {
            host: options.irc.ip().to_string(),
            port: options.irc.port(),
            server_password: options.gate_password.clone().filter(|p| !p.is_empty()),
            password: options.oper_password.clone(),
        },
        token: options.accounts_token.clone(),
        controller_token: options.controller_token.clone().filter(|t| !t.is_empty()),
        irc: options.irc,
        gate: options.gate_password.clone().filter(|p| !p.is_empty()),
        says: tokio::sync::Semaphore::new(SAYS),
        owners: tokio::sync::Mutex::default(),
        machines: crate::machines::Machines::beside(&history),
        invite_only: tokio::sync::Mutex::default(),
        seen: tokio::sync::Mutex::new(usage::Seen::beside(&history)),
        mailed: tokio::sync::Mutex::new(guests::Mailed::beside(&history)),
        profiles: tokio::sync::Mutex::new(profiles::Profiles::beside(&history)),
        signin: Arc::new(signin::Signin::beside(
            &history,
            options.server_name.clone(),
            jiff::Timestamp::now().as_second(),
        )),
        visibility: tokio::sync::Mutex::new(None),
        content: tokio::sync::Mutex::new(None),
        token_keys: tokio::sync::Mutex::default(),
        access: content::access_log(&history, jiff::Timestamp::now().as_second()),
        colleagues: tokio::sync::Mutex::new(None),
        hints: crate::hints::Store::beside(&history),
        searches: tokio::sync::Semaphore::new(SEARCHES),
        gateway,
        history,
        pushing: options.push_subscriber.is_some(),
    })
}

fn routes(ergo: Arc<Ergo>) -> Router {
    // Teams are the controller's to set, from the ZircServer's spec, and usage its to count;
    // neither is the web app's.
    let teams = Router::new()
        .route("/v1/teams", put(set_teams))
        .route("/v1/settings", put(set_settings))
        .route("/v1/usage", get(count_usage))
        .route_layer(middleware::from_fn_with_state(
            ergo.clone(),
            authorize_controller,
        ));
    Router::new()
        .route("/v1/accounts", get(list))
        .route("/v1/accounts/{name}", put(upsert).delete(erase))
        .route("/v1/accounts/{name}/profile", put(set_profile))
        .route("/v1/accounts/{name}/role", get(account_role))
        .route("/v1/accounts/{name}/keys", get(list_keys))
        .route(
            "/v1/accounts/{name}/keys/{key}",
            put(add_key).delete(remove_key),
        )
        .route(
            "/v1/accounts/{name}/suspend",
            post(suspend).delete(unsuspend),
        )
        .route("/v1/messages", get(find_messages))
        .route("/v1/activity", get(read_activity))
        .route("/v1/say", post(say))
        .route("/v1/push", get(push_key))
        .route("/v1/server", get(describe))
        .route(
            zirc_proto::workflow::LIST_PATH,
            get(list_workflows).post(deploy_workflow),
        )
        .route(zirc_proto::workflow::RUNS_PATH, get(workflow_runs))
        .route("/v1/channels", get(list_channels))
        .route("/v1/guests", get(list_guests))
        .route("/v1/guests/unread", get(unread_for_guests))
        .route("/v1/guests/mailed", post(mailed_to_guests))
        .route("/v1/guests/{name}/forget", post(forget_guest))
        .route("/v1/history", get(read_history))
        .route("/v1/conversations", get(list_conversations))
        .route(
            "/v1/messages/{msgid}",
            axum::routing::delete(remove_message),
        )
        .route_layer(middleware::from_fn_with_state(ergo.clone(), authorize))
        .merge(teams)
        .route("/healthz", get(|| async { "ok" }))
        .with_state(ergo)
}

/// The person `name` is, a machine's being its owner's, and their role by the owners and admins
/// the server was last told of: for the server's own services, such as a self-hosted server's
/// files (who may remove one). 404 for an account that's nobody's, such as a guest's.
async fn account_role(State(ergo): State<Arc<Ergo>>, Path(name): Path<String>) -> Response {
    let Some(person) = content::person_of(&name, &ergo.machines) else {
        return error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no person has that account",
        );
    };
    let owners = ergo.owners.lock().await.accounts.clone();
    let role = match ergo.content.lock().await.as_ref() {
        Some(told) => content::role_of(&person, &owners, told),
        None if owners.contains(&person) => content::Role::Owner,
        None => content::Role::Member,
    };
    Json(json!({"person": person, "role": role})).into_response()
}

async fn authorize(State(ergo): State<Arc<Ergo>>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match presented {
        Some(token) if same(token.as_bytes(), ergo.token.as_bytes()) => next.run(request).await,
        _ => error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is needed",
        ),
    }
}

async fn authorize_controller(
    State(ergo): State<Arc<Ergo>>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match (presented, &ergo.controller_token) {
        (Some(token), Some(expected)) if same(token.as_bytes(), expected.as_bytes()) => {
            next.run(request).await
        }
        _ => error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "only the controller sets teams",
        ),
    }
}

/// Makes the server's team channels as the controller asks, and says how each came out:
/// `{"channels": {"#name": null | "what went wrong"}}`.
async fn set_teams(
    State(ergo): State<Arc<Ergo>>,
    Json(mut desired): Json<teams::Desired>,
) -> Response {
    let mut owners = ergo.owners.lock().await;
    *ergo.invite_only.lock().await = desired
        .channels
        .iter()
        .filter(|c| c.invite_only)
        .cloned()
        .collect();
    let was = ergo.visibility.lock().await.replace(desired.visibility);
    *ergo.content.lock().await = desired.content.clone();
    *ergo.token_keys.lock().await = if desired.public.keys.is_empty() {
        desired.public.key.iter().cloned().collect()
    } else {
        desired.public.keys.clone()
    };
    ergo.gateway.set(desired.public.clone());
    ergo.gateway
        .set_owners(&desired.owners.iter().cloned().collect::<Vec<_>>());
    // Whose machines are, as zirc.dev says now; a controller from before them says nothing, and
    // what was said before stands.
    match &desired.machines {
        Some(machines) => ergo.machines.set(machines),
        None => desired.machines = ergo.machines.owners(),
    }
    if let Some(clients) = &desired.clients {
        ergo.machines.set_clients(clients);
    }
    if was.is_some_and(|was| was != desired.visibility) {
        announce(&ergo, desired.visibility).await;
    }
    match teams::reconcile(&ergo.api, &ergo.operator, &desired).await {
        Ok(outcome) => {
            info!(channels = outcome.len(), "teams set");
            // Public channels hide their guests from each other (+u).
            for channel in &desired.public.channels {
                if let Err(failure) = ergo.operator.modes(channel, "+u").await {
                    warn!(%failure, "couldn't set a public channel's +u");
                }
            }
            *owners = Owners {
                accounts: desired.owners.clone(),
                done: outcome
                    .iter()
                    .filter(|(_, problem)| problem.is_none())
                    .map(|(channel, _)| channel.clone())
                    .collect(),
            };
            Json(json!({"channels": outcome})).into_response()
        }
        Err(failure) => ergo_failed(failure),
    }
}

/// How many history searches run at once; one more is told to come back shortly.
const SEARCHES: usize = 2;

/// Posts by integrations (`crate::say`) made at once.
const SAYS: usize = 4;

/// Tells #general, if there is one, who may now read what, when an owner changes it.
async fn announce(ergo: &Ergo, visibility: Visibility) {
    let covers = match visibility {
        Visibility::Participant => "their own channels and direct messages only",
        Visibility::Channels => {
            "every channel on this server, private ones too; direct messages stay with the people in them"
        }
        Visibility::Everything => {
            "every channel and every direct message on this server, between anyone"
        }
    };
    let text = format!(
        "Who may read what here has changed: members, and the agents they use through zirc.dev, \
         can now search {covers}."
    );
    match ergo.operator.notice("#general", &text).await {
        Ok(()) => info!(?visibility, "visibility announced"),
        Err(failure) => warn!(%failure, "couldn't announce the visibility"),
    }
}

/// The history page someone asks for: `X-Zirc-As` is who, as the web app says, which only
/// it can (its bearer token).
#[derive(Deserialize)]
struct HistoryQuery {
    conversation: Option<String>,
    sender: Option<String>,
    q: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
    /// `messages` (the default: what people and agents say) or `all` (zircd's events too).
    kinds: Option<String>,
    /// `new` (the default: newest first) or `best` (the best matches first, for text the
    /// search index can find; otherwise newest first).
    sort: Option<String>,
    /// `1` for how many messages the text finds in each conversation the viewer may read,
    /// beside the page (`history::counts`).
    counts: Option<String>,
    /// Comma-separated message ids: those the viewer may read, oldest first, whatever else
    /// is asked (`history::by_ids`), for the lines behind a number on zirc.dev's Insights.
    ids: Option<String>,
}

/// The longest a page may read back, beyond which Ergo's retention has usually gone anyway.
const REACH: std::time::Duration = std::time::Duration::from_hours(24 * 400);

/// A request refused, with the answer to give (boxed, as a `Response` is large).
type Refused = Box<Response>;

/// Who's asking, with the channels Ergo has them or their machines in, and what they may
/// see; or why not.
async fn viewer(ergo: &Ergo, headers: &axum::http::HeaderMap) -> Result<Viewer, Refused> {
    #[derive(Deserialize)]
    struct Whois {
        #[serde(default)]
        present: bool,
        // null for an account that isn't on.
        #[serde(default)]
        channels: Option<Vec<Joined>>,
    }
    #[derive(Deserialize)]
    struct Joined {
        name: String,
    }
    let account = headers
        .get("x-zirc-as")
        .and_then(|v| v.to_str().ok())
        .map(str::to_lowercase)
        .filter(|a| valid_name(a))
        .ok_or_else(|| {
            Box::new(error(
                StatusCode::BAD_REQUEST,
                "caller",
                "X-Zirc-As must name the account asking",
            ))
        })?;
    // Their machines, as the server was told whose each is.
    let machines = ergo.machines.owners().unwrap_or_default();
    let machines = machines
        .into_iter()
        .filter(|(_, owner)| owner.eq_ignore_ascii_case(&account))
        .map(|(machine, _)| machine)
        .collect();
    let mut viewer = Viewer {
        account,
        machines,
        channels: BTreeSet::new(),
        policy: ergo.visibility.lock().await.unwrap_or_default(),
        guests: ergo.gateway.has_guests(),
    };
    // Their channels, and their machines', whose agents act for them.
    let accounts = sessions(&ergo.api)
        .await
        .map_err(|failure| Box::new(api_failed(failure)))?;
    let theirs: Vec<String> = accounts
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| viewer.owns(name))
        .collect();
    for name in theirs {
        // One account Ergo can't say about doesn't stop the rest.
        match ergo
            .api
            .call::<Whois>("whois", json!({"nickname": name}))
            .await
        {
            Ok(whois) if whois.present => {
                let names = whois.channels.into_iter().flatten();
                viewer.channels.extend(names.map(|c| c.name.to_lowercase()));
            }
            Ok(_) => {}
            Err(failure) => warn!(account = name, %failure, "couldn't ask Ergo whose channels"),
        }
    }
    Ok(viewer)
}

/// The query a request asks for, or why it can't be one.
fn history_query(
    asked: &HistoryQuery,
    viewer: &Viewer,
    limit: usize,
) -> Result<history::Query, Refused> {
    let bad = |what: &str| Box::new(error(StatusCode::BAD_REQUEST, "query", what.to_owned()));
    let time = |text: &Option<String>| -> Result<Option<jiff::Timestamp>, Refused> {
        text.as_deref()
            .map(|t| {
                t.parse()
                    .map_err(|_| bad("since and until are RFC 3339 times"))
            })
            .transpose()
    };
    let until = time(&asked.until)?.unwrap_or_else(jiff::Timestamp::now);
    let since = time(&asked.since)?.unwrap_or_else(|| {
        until
            .checked_sub(std::time::Duration::from_hours(24))
            .unwrap_or(until)
    });
    let reach = until.checked_sub(REACH).unwrap_or(since);
    let nanos = |t: jiff::Timestamp| i64::try_from(t.as_nanosecond()).unwrap_or(i64::MAX);
    let conversation = match &asked.conversation {
        None => None,
        Some(text) => Some(
            Conversation::parse(text, &viewer.account)
                .ok_or_else(|| bad("conversation is #channel, @account or @a,@b"))?,
        ),
    };
    let best = match asked.sort.as_deref() {
        None | Some("new") => false,
        Some("best") => true,
        Some(_) => return Err(bad("sort is new or best")),
    };
    // A best-first page goes on from how many it skipped; newest first, from a point in time.
    let (before, skip) = match asked.cursor.as_deref() {
        None => (None, 0),
        Some(text) => match text.strip_prefix('b') {
            Some(skip) if best => (
                None,
                skip.parse()
                    .map_err(|_| bad("cursor isn't one this gave"))?,
            ),
            _ => (
                Some(
                    text.parse()
                        .map_err(|()| bad("cursor isn't one this gave"))?,
                ),
                0,
            ),
        },
    };
    let events = match asked.kinds.as_deref() {
        None | Some("messages") => false,
        Some("all") => true,
        Some(_) => return Err(bad("kinds is messages or all")),
    };
    Ok(history::Query {
        conversation,
        sender: asked.sender.clone(),
        text: asked.q.clone().filter(|q| !q.trim().is_empty()),
        since: nanos(since.max(reach)),
        until: nanos(until),
        limit,
        before,
        events,
        best,
        skip,
    })
}

async fn read_history(
    State(ergo): State<Arc<Ergo>>,
    headers: axum::http::HeaderMap,
    Query(asked): Query<HistoryQuery>,
) -> Response {
    let Ok(_running) = ergo.searches.try_acquire() else {
        return busy();
    };
    let viewer = match viewer(&ergo, &headers).await {
        Ok(viewer) => viewer,
        Err(response) => return *response,
    };
    if let Some(ids) = &asked.ids {
        let ids: Vec<String> = ids.split(',').map(|id| id.trim().to_owned()).collect();
        let (path, policy) = (ergo.history.clone(), viewer.policy);
        return match tokio::task::spawn_blocking(move || history::by_ids(&path, &viewer, &ids))
            .await
        {
            Ok(Ok(messages)) => {
                Json(json!({ "policy": policy, "messages": messages })).into_response()
            }
            Ok(Err(failure)) => ergo_failed(failure),
            Err(failure) => ergo_failed(failure),
        };
    }
    let limit = asked.limit.unwrap_or(50).clamp(1, 200);
    let query = match history_query(&asked, &viewer, limit) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    if let Some(conversation) = &query.conversation
        && !viewer.may_see(conversation)
    {
        return error(
            StatusCode::FORBIDDEN,
            "visibility",
            format!("{conversation} isn't one you may read here"),
        );
    }
    let path = ergo.history.clone();
    let policy = viewer.policy;
    let with_counts = asked.counts.as_deref() == Some("1");
    let found = tokio::task::spawn_blocking(move || {
        let page = history::search(&path, &viewer, &query)?;
        let counts = if with_counts {
            history::counts(&path, &viewer, &query)?
        } else {
            None
        };
        Ok::<_, rusqlite::Error>((page, counts))
    });
    match found.await {
        Ok(Ok((page, counts))) => Json(json!({
            "policy": policy,
            "messages": page.messages,
            "next": page.next,
            "indexed": page.indexed,
            "counts": counts,
        }))
        .into_response(),
        Ok(Err(failure)) => ergo_failed(failure),
        Err(failure) => ergo_failed(failure),
    }
}

/// The conversations someone may read, each with its newest message's time and how many
/// messages it had since `since`: a catch-up's overview.
/// The team's gotchas the person may read (`GET /gotchas` on the content API): those kept in
/// the gotchas room and not retired since ([`zirc_proto::gotchas::kept`]), each
/// repository's apart, as a [`zirc_proto::gotchas::Listing`], newest first.
async fn list_gotchas(State(ergo): State<Arc<Ergo>>, headers: axum::http::HeaderMap) -> Response {
    use zirc_proto::gotchas::{self, Gotcha, Listed, Listing, Said};
    let Ok(_running) = ergo.searches.try_acquire() else {
        return busy();
    };
    let viewer = match viewer(&ergo, &headers).await {
        Ok(viewer) => viewer,
        Err(response) => return *response,
    };
    let rooms: Vec<String> = match crate::exporter::channels(&ergo.api, ergo.history.clone()).await
    {
        Ok(channels) => channels
            .into_iter()
            .map(|channel| channel.name)
            .filter(|name| gotchas::is_room(name))
            .collect(),
        Err(failure) => return api_failed(failure),
    };
    let now = jiff::Timestamp::now();
    let nanos = |t: jiff::Timestamp| i64::try_from(t.as_nanosecond()).unwrap_or(i64::MAX);
    let since = nanos(now.checked_sub(REACH).unwrap_or(now));
    let path = ergo.history.clone();
    let read = tokio::task::spawn_blocking(move || {
        let mut found: Vec<Listed> = Vec::new();
        for room in rooms {
            let Some(conversation) = history::Conversation::parse(&room, &viewer.account) else {
                continue;
            };
            let query = history::Query {
                conversation: Some(conversation),
                since,
                until: nanos(now),
                limit: gotchas::READ,
                events: true,
                ..history::Query::default()
            };
            let page = history::search(&path, &viewer, &query)?;
            // Oldest first, as they were said, by the repository each says.
            let mut by_repo: BTreeMap<String, Vec<Said>> = BTreeMap::new();
            for message in page.messages.into_iter().rev() {
                let Some(zirc) = message.zirc else { continue };
                let Some(repo) = zirc
                    .get("repo")
                    .and_then(Value::as_str)
                    .filter(|repo| !repo.is_empty())
                    .map(str::to_owned)
                else {
                    continue;
                };
                // Who learnt it: its session, as zirc's own lines name it, or the nick.
                let session = zirc
                    .get("session")
                    .and_then(Value::as_str)
                    .filter(|session| !session.is_empty())
                    .map(str::to_owned);
                let Ok(event) = serde_json::from_value::<zirc_proto::Event>(zirc) else {
                    continue;
                };
                let said = if let Some(retired) = gotchas::retiring(&event) {
                    Said::Retired(retired)
                } else if let Some((paths, text)) = gotchas::lesson(&event) {
                    Said::Kept(Gotcha {
                        paths,
                        text,
                        who: session.unwrap_or(message.sender.nick),
                        at: Some(message.time),
                        id: Some(message.msgid),
                    })
                } else {
                    continue;
                };
                by_repo.entry(repo).or_default().push(said);
            }
            for (repo, said) in by_repo {
                found.extend(
                    gotchas::kept(said)
                        .into_iter()
                        .map(|gotcha| Listed::new(&repo, gotcha)),
                );
            }
        }
        found.sort_by(|a, b| b.gotcha.at.cmp(&a.gotcha.at));
        Ok::<_, rusqlite::Error>(Listing { gotchas: found })
    })
    .await;
    match read {
        Ok(Ok(listing)) => Json(listing).into_response(),
        Ok(Err(failure)) => ergo_failed(failure),
        Err(failure) => ergo_failed(failure),
    }
}

/// How many days back the Playbook page's counts go.
const PLAYBOOK_DAYS: u32 = 90;

/// How often each hint was shown and each gotcha told in the last [`PLAYBOOK_DAYS`] (`GET
/// /playbook/usage` on the content API), from the activity store: a
/// [`zirc_proto::gotchas::Usage`], which says the days it counts.
async fn playbook_usage(State(ergo): State<Arc<Ergo>>) -> Response {
    use zirc_proto::gotchas::{Usage, Used};
    let window = std::time::Duration::from_hours(24 * u64::from(PLAYBOOK_DAYS));
    let now = jiff::Timestamp::now();
    let since =
        i64::try_from(now.checked_sub(window).unwrap_or(now).as_nanosecond()).unwrap_or_default();
    let path = crate::activity::path_for(&ergo.history);
    let read = tokio::task::spawn_blocking(move || {
        let connection = crate::activity::open(&path)?;
        crate::activity::playbook_usage(&connection, since)
    })
    .await;
    let said = |used: std::collections::BTreeMap<String, crate::activity::Used>| {
        used.into_iter()
            .map(|(id, used)| {
                (
                    id,
                    Used {
                        count: used.count,
                        last: rfc3339(used.last),
                    },
                )
            })
            .collect()
    };
    match read {
        Ok(Ok((hints, gotchas))) => Json(Usage {
            days: PLAYBOOK_DAYS,
            hints: said(hints),
            gotchas: said(gotchas),
        })
        .into_response(),
        Ok(Err(failure)) => ergo_failed(failure),
        Err(failure) => ergo_failed(failure),
    }
}

/// A time in nanoseconds since the Unix epoch, in RFC 3339.
fn rfc3339(nanotime: i64) -> String {
    jiff::Timestamp::from_nanosecond(i128::from(nanotime))
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// How many of an item's uses [`playbook_uses`] gives a page.
const PLAYBOOK_USES_PAGE: usize = 50;

/// Which item `GET /playbook/uses` lists: `hint=<id>` or `gotcha=<message id>`, and the page
/// before `before`, which a page's `older` gives.
#[derive(Deserialize)]
struct UsesQuery {
    hint: Option<String>,
    gotcha: Option<String>,
    before: Option<String>,
}

/// One hint's or gotcha's uses in the last [`PLAYBOOK_DAYS`] (`GET /playbook/uses` on the
/// content API), from the activity store: how many each day, and a page of them, newest first,
/// as a [`zirc_proto::gotchas::Uses`].
async fn playbook_uses(State(ergo): State<Arc<Ergo>>, Query(asked): Query<UsesQuery>) -> Response {
    use zirc_proto::gotchas::{Use, Uses};
    let bad = |what: &str| error(StatusCode::BAD_REQUEST, "query", what.to_owned());
    let (hint, gotcha) = match (asked.hint, asked.gotcha) {
        (Some(hint), None) => (Some(hint), None),
        (None, Some(gotcha)) => (None, Some(gotcha)),
        _ => return bad("name one hint or one gotcha"),
    };
    let before = match asked.before.as_deref().map(str::parse::<i64>) {
        None => None,
        Some(Ok(before)) => Some(before),
        Some(Err(_)) => return bad("before is what a page's older says"),
    };
    let now = i64::try_from(jiff::Timestamp::now().as_nanosecond()).unwrap_or(i64::MAX);
    let path = crate::activity::path_for(&ergo.history);
    let read = tokio::task::spawn_blocking(move || {
        let connection = crate::activity::open(&path)?;
        let item = match (&hint, &gotcha) {
            (Some(hint), _) => crate::activity::Item::Hint(hint),
            (_, gotcha) => crate::activity::Item::Gotcha(gotcha.as_deref().unwrap_or_default()),
        };
        // One more than a page, to know whether there are older ones.
        crate::activity::playbook_uses(
            &connection,
            item,
            now,
            PLAYBOOK_DAYS,
            before,
            PLAYBOOK_USES_PAGE + 1,
        )
    })
    .await;
    match read {
        Ok(Ok((daily, mut uses))) => {
            let more = uses.len() > PLAYBOOK_USES_PAGE;
            uses.truncate(PLAYBOOK_USES_PAGE);
            let older = more
                .then(|| uses.last().map(|last| last.nanotime.to_string()))
                .flatten();
            Json(Uses {
                days: PLAYBOOK_DAYS,
                daily,
                uses: uses
                    .into_iter()
                    .map(|used| Use {
                        at: rfc3339(used.nanotime),
                        machine: used.account,
                        session: used.session,
                        name: used.name,
                        tool: used.tool,
                        how: used.how,
                    })
                    .collect(),
                older,
            })
            .into_response()
        }
        Ok(Err(failure)) => ergo_failed(failure),
        Err(failure) => ergo_failed(failure),
    }
}

async fn list_conversations(
    State(ergo): State<Arc<Ergo>>,
    headers: axum::http::HeaderMap,
    Query(asked): Query<HistoryQuery>,
) -> Response {
    let Ok(_running) = ergo.searches.try_acquire() else {
        return busy();
    };
    let viewer = match viewer(&ergo, &headers).await {
        Ok(viewer) => viewer,
        Err(response) => return *response,
    };
    let query = match history_query(&asked, &viewer, history::SCAN) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    let (path, policy) = (ergo.history.clone(), viewer.policy);
    let page =
        match tokio::task::spawn_blocking(move || history::search(&path, &viewer, &query)).await {
            Ok(Ok(page)) => page,
            Ok(Err(failure)) => return ergo_failed(failure),
            Err(failure) => return ergo_failed(failure),
        };
    // Newest first, so a conversation's first message met is its newest.
    let mut seen: Vec<(String, String, usize)> = Vec::new();
    for message in page.messages {
        match seen.iter_mut().find(|(c, _, _)| *c == message.conversation) {
            Some((_, _, count)) => *count += 1,
            None => seen.push((message.conversation, message.time, 1)),
        }
    }
    let conversations: Vec<Value> = seen
        .into_iter()
        .map(|(conversation, last, count)| {
            let kind = if conversation.starts_with('#') {
                "channel"
            } else {
                "dm"
            };
            json!({"conversation": conversation, "kind": kind, "last": last, "count": count})
        })
        .collect();
    Json(json!({
        "policy": policy,
        "conversations": conversations,
        "complete": page.next.is_none(),
    }))
    .into_response()
}

/// Too many searches at once: come back in a moment.
fn busy() -> Response {
    let mut response = error(
        StatusCode::TOO_MANY_REQUESTS,
        "busy",
        "other searches are running; try again shortly",
    );
    response.headers_mut().insert(
        header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("2"),
    );
    response
}

/// How many messages a search returns at most.
const FOUND: usize = 50;

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    q: String,
    channel: Option<String>,
}

async fn find_messages(State(ergo): State<Arc<Ergo>>, Query(search): Query<Search>) -> Response {
    let history = ergo.history.clone();
    let channel = search.channel.filter(|c| !c.is_empty());
    let found = tokio::task::spawn_blocking(move || {
        trim::find(&history, channel.as_deref(), &search.q, FOUND)
    })
    .await;
    match found {
        Ok(Ok(messages)) => Json(json!({"messages": messages})).into_response(),
        Ok(Err(failure)) => history_failed(failure),
        Err(failure) => history_failed(failure),
    }
}

/// What `GET /v1/workflows/<name>/runs` is asked.
#[derive(Deserialize)]
struct RunsQuery {
    /// Where the page starts: the page before's `before`.
    #[serde(default)]
    before: Option<String>,
}

/// `GET /v1/workflows/<name>/runs`: a page of the workflow's ended runs, read back from the
/// history of its channel ([`crate::workflows::runs`]).
async fn workflow_runs(
    State(ergo): State<Arc<Ergo>>,
    Path(name): Path<String>,
    Query(query): Query<RunsQuery>,
) -> Response {
    let before = match query.before.as_deref().map(str::parse::<i64>) {
        None => None,
        Some(Ok(before)) => Some(before),
        Some(Err(_)) => {
            return error(
                StatusCode::BAD_REQUEST,
                "before",
                "before is a page's cursor",
            );
        }
    };
    let (workflows, _) = crate::workflows::listed();
    let Some(workflow) = workflows.into_iter().find(|w| w.name == name) else {
        return error(
            StatusCode::NOT_FOUND,
            "workflow",
            format!("no workflow {name}"),
        );
    };
    let history = ergo.history.clone();
    let page = tokio::task::spawn_blocking(move || {
        crate::workflows::runs::page(&history, &workflow.channel, &workflow.nick, before)
    })
    .await;
    match page {
        Ok(Ok(page)) => Json(page).into_response(),
        Ok(Err(failure)) => history_failed(failure),
        Err(failure) => history_failed(failure),
    }
}

/// What `GET /v1/activity` is asked.
#[derive(Deserialize)]
struct ActivityQuery {
    /// From when, in nanoseconds since the Unix epoch.
    #[serde(default)]
    since: i64,
    /// After which row, when paging.
    #[serde(default)]
    after: i64,
    /// How many rows at most.
    #[serde(default)]
    limit: Option<usize>,
    /// Whether to leave out a session's working sets that repeat one in the same hour
    /// ([`crate::activity::rows`]); `?collapse=true`. A reader that doesn't ask gets every row.
    #[serde(default)]
    collapse: bool,
}

async fn read_activity(
    State(ergo): State<Arc<Ergo>>,
    Query(query): Query<ActivityQuery>,
) -> Response {
    let store = crate::activity::path_for(&ergo.history);
    let read = tokio::task::spawn_blocking(move || {
        let connection = crate::activity::open(&store)?;
        crate::activity::rows(
            &connection,
            query.since,
            query.after,
            query.limit.unwrap_or(crate::activity::MOST),
            query.collapse,
        )
    })
    .await;
    match read {
        Ok(Ok(rows)) => Json(json!({"rows": rows})).into_response(),
        Ok(Err(failure)) => history_failed(failure),
        Err(failure) => history_failed(failure),
    }
}

async fn remove_message(State(ergo): State<Arc<Ergo>>, Path(msgid): Path<String>) -> Response {
    let history = ergo.history.clone();
    let lookup = msgid.clone();
    let channel =
        match tokio::task::spawn_blocking(move || trim::channel_of(&history, &lookup)).await {
            Ok(Ok(Some(channel))) => channel,
            Ok(Ok(None)) => {
                return error(
                    StatusCode::NOT_FOUND,
                    "not-found",
                    "no channel message has that id",
                );
            }
            Ok(Err(failure)) => return history_failed(failure),
            Err(failure) => return history_failed(failure),
        };
    // Out of the search index first, so a crash before Ergo's redaction leaves the message
    // unfindable rather than findable (`crate::index`).
    let index = crate::index::path_for(&ergo.history);
    let forgotten = msgid.clone();
    match tokio::task::spawn_blocking(move || {
        crate::index::open(&index)
            .and_then(|mut index| crate::index::forget(&mut index, &forgotten))
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(failure)) => return history_failed(failure),
        Err(failure) => return history_failed(failure),
    }
    match ergo.operator.redact(&channel, &msgid).await {
        Ok(()) => {}
        Err(OperatorError::Failed { code, .. }) if code == "UNKNOWN_MSGID" => {
            return error(
                StatusCode::NOT_FOUND,
                "not-found",
                "no channel message has that id",
            );
        }
        Err(failure) => return ergo_failed(failure),
    }
    info!(msgid, "message removed");
    // At once, rather than at the next round of `trim::keep_scrubbed`, which would also catch it.
    let history = ergo.history.clone();
    let scrubbed = match tokio::task::spawn_blocking(move || trim::scrub(&history)).await {
        Ok(Ok(())) => true,
        Ok(Err(failure)) => {
            warn!(error = %failure, "couldn't scrub Ergo's history; it's tried again shortly");
            false
        }
        Err(failure) => {
            warn!(error = %failure, "the scrub didn't finish; it's tried again shortly");
            false
        }
    };
    Json(json!({"removed": msgid, "channel": channel, "scrubbed": scrubbed})).into_response()
}

fn history_failed(failure: impl std::fmt::Display) -> Response {
    warn!(error = %failure, "couldn't read Ergo's history");
    error(StatusCode::BAD_GATEWAY, "history", failure.to_string())
}

use crate::secrets::same;

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
        .into_response()
}

fn ergo_failed(failure: impl std::fmt::Display) -> Response {
    warn!(error = %failure, "Ergo failed");
    error(StatusCode::BAD_GATEWAY, "ergo", failure.to_string())
}

/// Ergo's API didn't answer as asked. While Ergo is still starting (the sidecar starts
/// beside it, and callers don't wait), that's expected: 503, and no warning in the log.
fn api_failed(failure: reqwest::Error) -> Response {
    if failure.is_connect() {
        info!("Ergo isn't up yet");
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "starting",
            "the server is starting; try again in a moment",
        );
    }
    ergo_failed(failure)
}

/// Account names Ergo will take as nicks, as zirc-proto says them.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    zirc_proto::names::is_account_name(name)
}

#[derive(Deserialize)]
struct Registering {
    passphrase: String,
    /// Whose machine the account is, its owner's handle, when it's a machine's: zirc.dev says
    /// so as it's made, before the controller next does ([`crate::machines`]).
    #[serde(default)]
    owner: Option<String>,
    /// Whether it's no machine but `owner`'s own client under another name, such as their
    /// IRC client: its host then says so (`zirc_proto::names::client_host`).
    #[serde(default)]
    client: bool,
}

#[derive(Deserialize)]
struct Outcome {
    success: bool,
    #[serde(default, rename = "errorCode")]
    error_code: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Whether the server knows `name` as a person's or a machine's, as zirc.dev last told it: an
/// owner, an admin, a machine or client, or the owner of one. A workflow's nick is never one
/// of them: deployed as one, it would take that account before zirc.dev registers it.
async fn someones(ergo: &Ergo, name: &str) -> bool {
    let name = name.to_lowercase();
    if ergo.owners.lock().await.accounts.contains(&name)
        || ergo
            .content
            .lock()
            .await
            .as_ref()
            .is_some_and(|content| content.admins.contains(&name))
        || ergo.machines.clients().contains(&name)
    {
        return true;
    }
    ergo.machines.owners().is_some_and(|owners| {
        owners.contains_key(&name) || owners.values().any(|owner| *owner == name)
    })
}

/// Makes sure the integration `nick` has an account whose passphrase is the one derived from
/// the endpoint's token, which it logs in with. An account that exists is left as it is, as
/// its passphrase is the derived one already, unless `reset` says it isn't (the token changed):
/// then it's set again.
async fn integration_account(ergo: &Ergo, nick: &str, reset: bool) -> Result<(), Box<Response>> {
    let request = json!({"accountName": nick, "passphrase": say::passphrase(&ergo.token, nick)});
    let registered: Outcome = ergo
        .api
        .call("ns/saregister", request.clone())
        .await
        .map_err(|e| Box::new(api_failed(e)))?;
    if registered.success {
        return Ok(());
    }
    if registered.error_code.as_deref() != Some("ACCOUNT_EXISTS") {
        return Err(Box::new(ergo_failed(
            registered
                .error
                .unwrap_or_else(|| "registration failed".into()),
        )));
    }
    if !reset {
        return Ok(());
    }
    let changed: Outcome = ergo
        .api
        .call("ns/passwd", request)
        .await
        .map_err(|e| Box::new(api_failed(e)))?;
    if changed.success {
        Ok(())
    } else {
        Err(Box::new(ergo_failed(changed.error.unwrap_or_else(|| {
            "changing the passphrase failed".into()
        }))))
    }
}

/// `GET /v1/workflows`: the workflows the server runs and their runs that aren't finished
/// ([`crate::workflows::listed`]), and whether it runs them at all.
async fn list_workflows(State(ergo): State<Arc<Ergo>>) -> Response {
    let on = *ergo.gateway.runs_workflows().borrow();
    let (workflows, refused) = crate::workflows::listed();
    Json(zirc_proto::workflow::Hosting {
        on,
        workflows,
        refused,
    })
    .into_response()
}

/// `POST /v1/workflows`: a deploy from zirc.dev's dashboard, for the person signed in there.
/// The file is checked here, then said in `#workflows` as zirc's own account, where the
/// workflows' host hears it as it hears any deploy and holds it to the same rules: whose it
/// is to replace, and never under a run. Whether it took it shows in the list.
async fn deploy_workflow(
    State(ergo): State<Arc<Ergo>>,
    Json(body): Json<zirc_proto::workflow::Deploying>,
) -> Response {
    use zirc_proto::workflow::{CHANNEL, Deploy, Sent};
    if !*ergo.gateway.runs_workflows().borrow() {
        return error(
            StatusCode::CONFLICT,
            "off",
            "workflows aren't turned on for this organisation",
        );
    }
    if !zirc_proto::names::is_account_name(&body.by) {
        return error(StatusCode::BAD_REQUEST, "invalid", "by isn't an account");
    }
    let workflow = match zirc_workflows::parse(&body.source) {
        Ok(workflow) => workflow,
        Err(why) => return error(StatusCode::BAD_REQUEST, "invalid", why),
    };
    let sent = Sent {
        name: workflow.name.clone(),
        version: zirc_workflows::version(&body.source),
    };
    let deploy = Deploy {
        name: workflow.name,
        nick: workflow.nick,
        source: body.source,
        by: Some(body.by),
    };
    let nick = zirc_proto::names::SYSTEM;
    if let Err(response) = integration_account(&ergo, nick, false).await {
        return *response;
    }
    let config = say::config(
        &ergo.irc.ip().to_string(),
        ergo.irc.port(),
        ergo.gate.as_deref(),
        &ergo.token,
        nick,
    );
    let message = zirc_proto::Message::new("dashboard", deploy.event());
    match say::post(&config, CHANNEL, &message).await {
        Ok(()) => {
            info!(workflow = sent.name, "the dashboard deployed a workflow");
            (StatusCode::ACCEPTED, Json(sent)).into_response()
        }
        Err(failure) => {
            warn!(workflow = sent.name, %failure, "the dashboard's deploy couldn't be said");
            error(StatusCode::BAD_GATEWAY, "say", failure.to_string())
        }
    }
}

/// `POST /v1/say`: an integration's post, as `crate::say` says.
async fn say(State(ergo): State<Arc<Ergo>>, Json(body): Json<say::Say>) -> Response {
    let message = match body.message() {
        Ok(message) => message,
        Err(why) => return error(StatusCode::BAD_REQUEST, "invalid", why),
    };
    let Ok(_permit) = ergo.says.try_acquire() else {
        return error(StatusCode::TOO_MANY_REQUESTS, "busy", "try again shortly");
    };
    if let Err(response) = integration_account(&ergo, &body.nick, false).await {
        return *response;
    }
    let config = say::config(
        &ergo.irc.ip().to_string(),
        ergo.irc.port(),
        ergo.gate.as_deref(),
        &ergo.token,
        &body.nick,
    );
    let mut posted = say::post(&config, &body.channel, &message).await;
    if matches!(posted, Err(say::Failed::Auth(_))) {
        // The login was refused, as when the token changed: set the passphrase, once more. A
        // timeout or another failure never touches the account.
        if let Err(response) = integration_account(&ergo, &body.nick, true).await {
            return *response;
        }
        posted = say::post(&config, &body.channel, &message).await;
    }
    match posted {
        Ok(()) => {
            info!(
                nick = body.nick,
                kind = body.kind,
                "an integration said something"
            );
            (StatusCode::OK, Json(json!({"posted": true}))).into_response()
        }
        Err(failure) => {
            warn!(nick = body.nick, kind = body.kind, %failure, "an integration couldn't say it");
            error(StatusCode::BAD_GATEWAY, "say", failure.to_string())
        }
    }
}

/// Why zirc.dev may not set an account called `name`, if it may not.
fn unsettable(name: &str) -> Option<Response> {
    if !valid_name(name) {
        return Some(error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "names are [a-z][a-z0-9_-], at most 32 characters",
        ));
    }
    // A name a workflow runs as is the workflow's account, made by the server: never taken for
    // a person or a machine, whose messages would then be the workflow's and whose sign-ins
    // would join its connection. Its workflow goes first.
    crate::workflows::is_zircs_own(name).then(|| {
        error(
            StatusCode::CONFLICT,
            "taken",
            "a workflow runs as that name: remove it first",
        )
    })
}

async fn upsert(
    State(ergo): State<Arc<Ergo>>,
    Path(name): Path<String>,
    Json(body): Json<Registering>,
) -> Response {
    if let Some(refused) = unsettable(&name) {
        return refused;
    }
    // Whose it is, first: what lets it into its owner's team channels (`admit`), and what it
    // may read, go by it.
    if let Some(owner) = body
        .owner
        .as_deref()
        .filter(|o| zirc_proto::names::is_handle(o))
    {
        if body.client {
            ergo.machines.add_client(&name, owner);
        } else {
            ergo.machines.add(&name, owner);
        }
    }
    let request = json!({"accountName": name, "passphrase": body.passphrase});
    let mut registered: Outcome = match ergo.api.call("ns/saregister", request.clone()).await {
        Ok(outcome) => outcome,
        Err(failure) => return api_failed(failure),
    };
    // A name Ergo has but can't load is suspended or unregistered: left from when zirc.dev
    // locked accounts, before it freed names (docs/accounts.md). zirc.dev sets only names its
    // records give to this person or machine, so what's left of it is erased, and it's
    // registered afresh. Never a guest's: a banned guest stays banned.
    if registered.error_code.as_deref() == Some("ACCOUNT_EXISTS") && !name.starts_with("guest-") {
        match exists(&ergo, &name).await {
            Ok(true) => {}
            Ok(false) => {
                if let Err(failure) = ergo.operator.erase(&name).await {
                    return ergo_failed(failure);
                }
                info!(account = name, "erased what was left of it");
                registered = match ergo.api.call("ns/saregister", request.clone()).await {
                    Ok(outcome) => outcome,
                    Err(failure) => return api_failed(failure),
                };
            }
            Err(response) => return *response,
        }
    }
    if registered.success {
        if let Err(response) = stays_on(&ergo, &name).await {
            return *response;
        }
        admit(&ergo, &name).await;
        host_machines(&ergo).await;
        info!(account = name, "registered");
        return (StatusCode::CREATED, Json(json!({"name": name}))).into_response();
    }
    match registered.error_code.as_deref() {
        Some("ACCOUNT_EXISTS") => {}
        Some("INVALID_PASSPHRASE") => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid",
                "Ergo refused the passphrase",
            );
        }
        _ => {
            return ergo_failed(
                registered
                    .error
                    .unwrap_or_else(|| "registration failed".into()),
            );
        }
    }
    let changed: Outcome = match ergo.api.call("ns/passwd", request).await {
        Ok(outcome) => outcome,
        Err(failure) => return api_failed(failure),
    };
    match (changed.success, changed.error_code.as_deref()) {
        (true, _) => {
            if let Err(response) = stays_on(&ergo, &name).await {
                return *response;
            }
            info!(account = name, "passphrase set");
            (StatusCode::OK, Json(json!({"name": name}))).into_response()
        }
        // A guest's, taken once and unregistered: Ergo keeps the name from everyone.
        (false, Some("ACCOUNT_DOES_NOT_EXIST")) => error(
            StatusCode::CONFLICT,
            "reserved",
            "the name was registered once and can't be again",
        ),
        (false, Some("INVALID_PASSPHRASE")) => error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "Ergo refused the passphrase",
        ),
        (false, _) => ergo_failed(
            changed
                .error
                .unwrap_or_else(|| "changing the passphrase failed".into()),
        ),
    }
}

/// Lets a new account into the invite-only team channels it belongs in, a member's or one of
/// their machines, before it's told it exists, so it can join them at once rather than after
/// the controller next sets the teams. Anything that goes wrong waits for that.
async fn admit(ergo: &Ergo, name: &str) {
    let handle = ergo.machines.owner(name).unwrap_or_else(|| name.to_owned());
    let channels: Vec<teams::Channel> = ergo
        .invite_only
        .lock()
        .await
        .iter()
        .filter(|c| c.accounts.contains(&handle))
        .cloned()
        .collect();
    if channels.is_empty() {
        return;
    }
    let desired = teams::Desired {
        channels,
        machines: ergo.machines.owners(),
        ..teams::Desired::default()
    };
    match teams::reconcile(&ergo.api, &ergo.operator, &desired).await {
        Ok(outcome) => {
            for (_, problem) in outcome {
                if let Some(problem) = problem {
                    warn!(account = name, problem, "couldn't let in");
                }
            }
        }
        Err(failure) => warn!(account = name, %failure, "couldn't let into its team channels"),
    }
}

/// Keeps a person's account on the server while all their apps are closed, marked away, so
/// how far they've read (`draft/read-marker`) and what's said to them are there when they
/// come back. A machine's account (`<handle>-<machine>`: handles have no `-`) comes and goes
/// with its zircd, so it shows only while it's there.
async fn stays_on(ergo: &Ergo, name: &str) -> Result<(), Box<Response>> {
    if !is_person(name) {
        return Ok(());
    }
    match set_always_on(&ergo.api, name).await {
        Ok(None) => Ok(()),
        Ok(Some(failure)) => Err(Box::new(ergo_failed(failure))),
        Err(failure) => Err(Box::new(api_failed(failure))),
    }
}

/// Whether `name` is a person's account: not a machine's (`<handle>-<machine>`; handles
/// have no `-`), nor the one team channels are registered to.
fn is_person(name: &str) -> bool {
    !name.contains('-') && name != teams::FOUNDER
}

/// Makes `name` always-on and auto-away. What Ergo said went wrong, if it refused.
pub(crate) async fn set_always_on(api: &Api, name: &str) -> reqwest::Result<Option<String>> {
    let set: Outcome = api
        .call(
            "ns/saset",
            json!({"accountName": name, "alwaysOn": "on", "autoAway": "on"}),
        )
        .await?;
    Ok((!set.success).then(|| {
        set.error
            .or(set.error_code)
            .unwrap_or_else(|| "keeping the account on failed".into())
    }))
}

/// Makes every person's account always-on and auto-away, as [`stays_on`] does for each one
/// the endpoint sets: for those registered before it did. Waits for Ergo, which starts
/// beside the sidecar; harmless to repeat, so it runs each time the sidecar starts. Returns
/// how many it set.
pub async fn keep_people_on(api: Api) -> usize {
    #[derive(Deserialize)]
    struct Listed {
        accounts: Vec<Account>,
    }
    #[derive(Deserialize)]
    struct Account {
        #[serde(rename = "accountName")]
        name: String,
    }
    let listed: Listed = loop {
        match api.call("ns/list", json!({})).await {
            Ok(listed) => break listed,
            Err(failure) if failure.is_connect() => {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(failure) => {
                warn!(error = %failure, "couldn't list accounts to keep people's on");
                return 0;
            }
        }
    };
    let mut set = 0;
    for name in listed
        .accounts
        .into_iter()
        .map(|a| a.name)
        .filter(|n| is_person(n))
    {
        match set_always_on(&api, &name).await {
            Ok(None) => set += 1,
            Ok(Some(failure)) => warn!(account = name, error = failure, "not kept on"),
            Err(failure) => warn!(account = name, error = %failure, "not kept on"),
        }
    }
    info!(people = set, "people's accounts kept on");
    set
}

async fn exists(ergo: &Ergo, name: &str) -> Result<bool, Box<Response>> {
    let info: Outcome = ergo
        .api
        .call("ns/info", json!({"accountName": name}))
        .await
        .map_err(|failure| Box::new(api_failed(failure)))?;
    Ok(info.success)
}

async fn push_key(State(ergo): State<Arc<Ergo>>) -> Response {
    if !ergo.pushing {
        return Json(json!({"vapidKey": null})).into_response();
    }
    let history = ergo.history.clone();
    let key = tokio::task::spawn_blocking(move || crate::pusher::public_key(&history))
        .await
        .map_err(|error| error.to_string())
        .and_then(|key| key);
    match key {
        Ok(key) => Json(json!({"vapidKey": key})).into_response(),
        Err(why) => {
            warn!(%why, "the pusher's key can't be read");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "starting",
                "the pusher's key can't be read",
            )
        }
    }
}

/// Erases an account, which frees its name. Never the founder, which the server's channels
/// need.
async fn erase(State(ergo): State<Arc<Ergo>>, Path(name): Path<String>) -> Response {
    if !valid_name(&name) || name == teams::FOUNDER {
        return error(StatusCode::BAD_REQUEST, "invalid", "not an account name");
    }
    match ergo.operator.erase(&name).await {
        Ok(()) => {
            ergo.signin.keys.lock().await.forget(&name);
            info!(account = name, "erased");
            (StatusCode::OK, Json(json!({"name": name}))).into_response()
        }
        Err(failure) => ergo_failed(failure),
    }
}

#[derive(Deserialize)]
struct Label {
    label: String,
}

/// What the person is told when a device can sign in as them.
#[must_use]
pub fn new_device(label: &str) -> String {
    format!(
        "A new device, {label}, can now sign in as you. If it isn't yours, tell your \
         organisation's owners at once."
    )
}

async fn add_key(
    State(ergo): State<Arc<Ergo>>,
    Path((name, key)): Path<(String, String)>,
    Json(body): Json<Label>,
) -> Response {
    if !valid_name(&name) {
        return error(StatusCode::BAD_REQUEST, "invalid", "not an account name");
    }
    match exists(&ergo, &name).await {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::NOT_FOUND, "not-found", "no such account"),
        Err(response) => return *response,
    }
    // Where the server doesn't take device keys, none is kept, and nobody's told of one that
    // can't sign in: zirc.dev registers a device's keys again each time it gives it
    // credentials, so they come, and are told of, once it does.
    if ergo.signin.server_name.is_none() {
        return Json(json!({"key": key, "active": false})).into_response();
    }
    let active = true;
    let now = jiff::Timestamp::now().as_second();
    let added = ergo
        .signin
        .keys
        .lock()
        .await
        .add(&name, &key, &body.label, now);
    match added {
        Ok(true) => {
            info!(account = name, "device key added");
            // Told whether or not the person's on now: their account is always-on, and the
            // message reaches their phones as a push. A failure to tell doesn't undo the key.
            let text = new_device(body.label.trim());
            if let Err(failure) = ergo
                .operator
                .direct(crate::visitors::FROM, std::slice::from_ref(&name), &text)
                .await
            {
                warn!(%failure, account = name, "couldn't tell of a new device");
            }
            (
                StatusCode::CREATED,
                Json(json!({"key": key, "active": active})),
            )
                .into_response()
        }
        Ok(false) => (StatusCode::OK, Json(json!({"key": key, "active": active}))).into_response(),
        Err(signin::Refused::NotAKey) => error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "not a P-256 public key (base64url of its 65-byte point)",
        ),
        Err(signin::Refused::Label) => error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "a label is 1 to 64 printable characters",
        ),
        Err(signin::Refused::TooMany) => error(
            StatusCode::CONFLICT,
            "too-many",
            "the account has as many device keys as it may",
        ),
    }
}

async fn remove_key(
    State(ergo): State<Arc<Ergo>>,
    Path((name, key)): Path<(String, String)>,
) -> Response {
    let removed = ergo.signin.keys.lock().await.remove(&name, &key);
    if removed {
        info!(account = name, "device key removed");
    }
    Json(json!({"removed": removed})).into_response()
}

async fn list_keys(State(ergo): State<Arc<Ergo>>, Path(name): Path<String>) -> Response {
    let keys: Vec<Value> = ergo
        .signin
        .keys
        .lock()
        .await
        .of(&name)
        .into_iter()
        .map(|(key, k)| {
            json!({"key": key, "label": k.label, "addedAt": k.added_at, "lastUsed": k.last_used})
        })
        .collect();
    Json(json!({"keys": keys})).into_response()
}

async fn suspend(State(ergo): State<Arc<Ergo>>, Path(name): Path<String>) -> Response {
    nickserv_on(&ergo, &name, "SUSPEND ADD", "suspended").await
}

async fn unsuspend(State(ergo): State<Arc<Ergo>>, Path(name): Path<String>) -> Response {
    nickserv_on(&ergo, &name, "SUSPEND DEL", "unsuspended").await
}

/// Runs an operator's `NickServ` command on an account that exists.
async fn nickserv_on(ergo: &Ergo, name: &str, command: &str, done: &str) -> Response {
    if !valid_name(name) {
        return error(StatusCode::BAD_REQUEST, "invalid", "not an account name");
    }
    // Ergo's API doesn't find a suspended account, so only suspending asks it first; lifting
    // goes by NickServ's own answer.
    if command == "SUSPEND ADD" {
        match exists(ergo, name).await {
            Ok(true) => {}
            Ok(false) => return error(StatusCode::NOT_FOUND, "not-found", "no such account"),
            Err(response) => return *response,
        }
    }
    match ergo.operator.nickserv(&format!("{command} {name}")).await {
        Ok(reply)
            if reply.to_lowercase().contains("no such")
                || reply.to_lowercase().contains("not exist") =>
        {
            error(StatusCode::NOT_FOUND, "not-found", "no such account")
        }
        // "Successfully suspended account …", or that it already was (or wasn't).
        Ok(reply)
            if reply.contains("uccessfully")
                || reply.contains("not suspended")
                || reply.contains("already") =>
        {
            info!(account = name, "{done}");
            (StatusCode::OK, Json(json!({"name": name}))).into_response()
        }
        Ok(reply) => ergo_failed(reply),
        Err(failure) => ergo_failed(failure),
    }
}

async fn list(State(ergo): State<Arc<Ergo>>) -> Response {
    match sessions(&ergo.api).await {
        Ok(accounts) => {
            let last = ergo.seen.lock().await.last();
            let accounts: Vec<Value> = accounts
                .into_iter()
                .map(|(name, sessions)| {
                    // When it was last seen connected, sampled every few minutes; null never.
                    let seen = last
                        .get(&name)
                        .and_then(|s| jiff::Timestamp::from_second(*s).ok())
                        .map(|t| t.to_string());
                    json!({"name": name, "connected": sessions > 0, "sessions": sessions, "lastSeen": seen})
                })
                .collect();
            Json(json!({"accounts": accounts})).into_response()
        }
        Err(failure) => api_failed(failure),
    }
}

/// Puts the server's config file and MOTD in force (`crate::config::apply`), as the controller
/// asks: `{"config", "motd"}`, answered with `{"pending": [field]}`, the settings that wait
/// for a restart.
async fn set_settings(State(ergo): State<Arc<Ergo>>, Json(given): Json<Settings>) -> Response {
    let dir = ergo.history.parent().unwrap_or(std::path::Path::new("."));
    match crate::config::apply(dir, &ergo.operator, &given.config, &given.motd).await {
        Ok(pending) => {
            info!(?pending, "settings in force");
            if let Some(colleagues) = given.colleagues {
                tell_colleagues(&ergo, colleagues).await;
            }
            Json(json!({"pending": pending})).into_response()
        }
        Err(failure) => ergo_failed(failure),
    }
}

#[derive(Deserialize)]
struct Settings {
    config: String,
    motd: String,
    /// From a controller since there were these.
    #[serde(default)]
    colleagues: Option<Colleagues>,
}

/// Says in `#ops`, as zirc's own account, what members' agents may do with other members
/// and theirs, for each machine's zircd to take: once after the sidecar starts, as what was
/// said before may have been, and whenever it changes. A failure is logged and said again
/// with the next settings the controller gives.
async fn tell_colleagues(ergo: &Ergo, colleagues: Colleagues) {
    let mut said = ergo.colleagues.lock().await;
    if *said == Some(colleagues) {
        return;
    }
    match say_as_zirc(ergo, zirc_proto::state::CHANNEL, colleagues.event(), None).await {
        Ok(()) => {
            info!(?colleagues, "said what colleagues' agents may do");
            *said = Some(colleagues);
        }
        Err(failure) => warn!(%failure, "couldn't say what colleagues' agents may do"),
    }
}

/// Keeps the playbook's suggestions going ([`crate::suggestions`]): what the history shows of
/// replies and reactions to them every [`crate::suggestions::EVERY`], what the activity store
/// shows worth suggesting every [`crate::suggestions::LOOK`], each step said as zirc's own
/// account. A person here is a person's account or one of their own clients, never a machine.
async fn suggest(ergo: Arc<Ergo>) {
    use crate::suggestions::{EVERY, LOOK, round};
    let mut every = tokio::time::interval(EVERY);
    let mut looked: Option<tokio::time::Instant> = None;
    loop {
        every.tick().await;
        let look = looked.is_none_or(|at| at.elapsed() >= LOOK);
        if look {
            looked = Some(tokio::time::Instant::now());
        }
        let (history, clients) = (ergo.history.clone(), ergo.machines.clients());
        let now = u64::try_from(jiff::Timestamp::now().as_second()).unwrap_or_default();
        let person = move |account: &str| is_person(account) || clients.contains(account);
        let says = tokio::task::spawn_blocking(move || round(&history, person, look, now)).await;
        match says {
            Ok(Ok(says)) => {
                for say in says {
                    if let Err(failure) =
                        say_as_zirc(&ergo, &say.channel, say.event, say.repo).await
                    {
                        warn!(%failure, "couldn't say a suggestion");
                    }
                }
            }
            Ok(Err(error)) => warn!(%error, "couldn't read for suggestions"),
            Err(_) => {}
        }
    }
}

/// Says `event` in `channel` as zirc's own account ([`zirc_proto::names::SYSTEM`]), which no
/// person or machine holds: what each machine's zircd takes as the server's word.
async fn say_as_zirc(
    ergo: &Ergo,
    channel: &str,
    event: zirc_proto::Event,
    repo: Option<String>,
) -> Result<(), String> {
    let nick = zirc_proto::names::SYSTEM;
    if integration_account(ergo, nick, false).await.is_err() {
        return Err("zirc's own account couldn't be made".to_owned());
    }
    let config = say::config(
        &ergo.irc.ip().to_string(),
        ergo.irc.port(),
        ergo.gate.as_deref(),
        &ergo.token,
        nick,
    );
    let message = zirc_proto::Message {
        repo,
        ..zirc_proto::Message::new("server", event)
    };
    say::post(&config, channel, &message)
        .await
        .map_err(|failure| failure.to_string())
}

/// Says a change to the team's hints, as zirc's own account, in the repository's channel, or
/// `#ops` for the organisation's: what each machine's zircd takes to read them again, and what
/// people see of who changed what. A failure is logged; machines read them again when they
/// next connect.
async fn tell_hints_changed(ergo: Arc<Ergo>, change: zirc_proto::hints::Change) {
    let channel = if change.hint.scope == zirc_proto::hints::ORG {
        zirc_proto::state::CHANNEL
    } else {
        change.hint.scope.as_str()
    };
    let changed = zirc_proto::hints::Changed::of_change(&change);
    if let Err(failure) = say_as_zirc(&ergo, channel, changed.event(&change), None).await {
        warn!(%failure, channel, "couldn't say the hints changed");
    }
}

/// Sets an account's name and picture (`PUT /v1/accounts/<name>/profile`).
async fn set_profile(
    State(ergo): State<Arc<Ergo>>,
    Path(name): Path<String>,
    Json(profile): Json<profiles::Profile>,
) -> Response {
    let name = name.to_lowercase();
    if !valid_name(&name) {
        return error(StatusCode::BAD_REQUEST, "invalid", "not an account's name");
    }
    if let Some(why) = profile.invalid() {
        return error(StatusCode::BAD_REQUEST, "invalid", why);
    }
    let mut profiles = ergo.profiles.lock().await;
    if let Err(failure) = profiles.keep(&name, profile.clone()) {
        return ergo_failed(failure);
    }
    let keys = vec![(name.clone(), profile.keys().to_vec())];
    match ergo.operator.set_metadata(&keys).await {
        Ok(set) => {
            let applied = set.contains(&name);
            profiles.set.extend(set);
            Json(json!({"applied": applied})).into_response()
        }
        Err(failure) => ergo_failed(failure),
    }
}

/// How many [`profiles::EVERY`]s go by between setting every machine's again, for those that
/// came back on between two looks, whose new client hasn't them.
const PROFILES_AGAIN: u32 = 10;

/// Every so often, gives accounts that have come on what zirc.dev last said of them: a
/// machine's as it connects, and anything that couldn't be set before. Who's on is Ergo's
/// API's to say, which opens no IRC session; setting them takes the operator's one.
async fn keep_profiles(ergo: Arc<Ergo>) {
    #[derive(Deserialize)]
    struct Whois {
        #[serde(default)]
        present: bool,
    }
    let mut every = tokio::time::interval(profiles::EVERY);
    let mut ticks: u32 = 0;
    loop {
        every.tick().await;
        ticks = ticks.wrapping_add(1);
        let again = ticks.is_multiple_of(PROFILES_AGAIN);
        let (said, set) = {
            let profiles = ergo.profiles.lock().await;
            (profiles.said.clone(), profiles.set.clone())
        };
        let mut due = Vec::new();
        for (account, profile) in said {
            // A person's always-on account keeps them once set; a machine's goes when it does.
            if set.contains(&account) && is_person(&account) {
                continue;
            }
            let present = ergo
                .api
                .call::<Whois>("whois", json!({"nickname": account}))
                .await
                .is_ok_and(|w| w.present);
            if !present {
                ergo.profiles.lock().await.set.remove(&account);
            } else if again || !set.contains(&account) {
                due.push((account, profile));
            }
        }
        let keys: Vec<(String, crate::ergo::Metadata<'_>)> = due
            .iter()
            .map(|(account, profile)| (account.clone(), profile.keys().to_vec()))
            .collect();
        match ergo.operator.set_metadata(&keys).await {
            Ok(set) => ergo.profiles.lock().await.set.extend(set),
            Err(failure) => warn!(%failure, "couldn't set profiles"),
        }
        host_machines(&ergo).await;
    }
}

/// Gives each machine whose host doesn't say whose it is yet its host, `machine/<owner>`, or
/// `client/<owner>` for a person's own client (`crate::machines`), as Ergo's operator: what
/// tells the apps and zircd what it is and whose. One whose account isn't made yet is tried
/// again next time.
async fn host_machines(ergo: &Ergo) {
    let due = ergo.machines.unhosted();
    if due.is_empty() {
        return;
    }
    let commands: Vec<String> = due
        .iter()
        .map(|(account, host)| format!("SET {account} {host}"))
        .collect();
    match ergo.operator.hostserv(&commands).await {
        Ok(answers) => {
            for ((account, host), answer) in due.iter().zip(answers) {
                if hosts_set(&answer) {
                    ergo.machines.hosted(account, host);
                } else {
                    debug!(account, answer, "couldn't set a machine's host yet");
                }
            }
        }
        Err(failure) => warn!(%failure, "couldn't set machines' hosts"),
    }
}

/// Whether `HostServ`'s answer to `SET` says it set the host.
fn hosts_set(answer: &str) -> bool {
    answer.to_ascii_lowercase().contains("success")
}

/// The organisation's guests, from the history and the gateway, most recently there first.
async fn guests(ergo: &Ergo) -> Result<Vec<guests::Guest>, String> {
    let path = ergo.history.clone();
    let known = tokio::task::spawn_blocking(move || guests::in_history(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let (connected, seen) = ergo.gateway.presence();
    let mut all: std::collections::BTreeMap<String, (i64, BTreeSet<String>)> = known;
    // Guests with accounts who haven't said anything yet, and when each was last on.
    let accounts = sessions(&ergo.api).await.map_err(|e| e.to_string())?;
    let last_on = ergo.seen.lock().await.last();
    for (name, _) in accounts.iter().filter(|(name, _)| guests::is_guest(name)) {
        let entry = all.entry(name.clone()).or_default();
        if let Some(at) = last_on.get(name) {
            entry.0 = entry.0.max(at.saturating_mul(1_000_000_000));
        }
    }
    for (name, (at, channel)) in &seen {
        let entry = all.entry(name.clone()).or_default();
        let at = i64::try_from(at.as_nanosecond()).unwrap_or(i64::MAX);
        entry.0 = entry.0.max(at);
        entry.1.insert(channel.clone());
    }
    let mut list: Vec<guests::Guest> = all
        .into_iter()
        .map(|(name, (at, channels))| guests::Guest {
            connected: connected.contains(&name),
            last_seen: (at > 0)
                .then(|| jiff::Timestamp::from_nanosecond(i128::from(at)).ok())
                .flatten()
                .map(|t| t.to_string()),
            channels,
            name,
        })
        .collect();
    list.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.name.cmp(&b.name)));
    Ok(list)
}

async fn list_guests(State(ergo): State<Arc<Ergo>>) -> Response {
    match guests(&ergo).await {
        Ok(list) => Json(json!({"guests": list})).into_response(),
        Err(failure) => ergo_failed(failure),
    }
}

/// A guest's name from the path, or why it isn't one.
fn guest_named(name: &str) -> Result<String, Box<Response>> {
    let name = name.to_lowercase();
    if guests::is_guest(&name) {
        Ok(name)
    } else {
        Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "not a guest's name",
        )))
    }
}

#[derive(Deserialize)]
struct UnreadQuery {
    text: Option<bool>,
}

/// What staff wrote to guests while they were away (`GET /v1/guests/unread`).
async fn unread_for_guests(
    State(ergo): State<Arc<Ergo>>,
    Query(asked): Query<UnreadQuery>,
) -> Response {
    let accounts = match sessions(&ergo.api).await {
        Ok(accounts) => accounts,
        Err(failure) => return api_failed(failure),
    };
    let (connected, here) = ergo.gateway.presence();
    let last_on = ergo.seen.lock().await.last();
    let mailed = ergo.mailed.lock().await.ids();
    let with_text = asked.text.unwrap_or(true);
    let mut unread = Vec::new();
    for (guest, sessions) in accounts {
        if !guests::is_guest(&guest) || sessions > 0 || connected.contains(&guest) {
            continue;
        }
        // Since they were last on, as the sidecar saw: every few minutes, and as they left.
        let mut since = last_on
            .get(&guest)
            .map_or(0, |s| s.saturating_mul(1_000_000_000));
        if let Some((at, _)) = here.get(&guest) {
            since = since.max(i64::try_from(at.as_nanosecond()).unwrap_or(i64::MAX));
        }
        let (path, who, mailed) = (ergo.history.clone(), guest.clone(), mailed.clone());
        let lines = tokio::task::spawn_blocking(move || {
            guests::unread(&path, &who, since, &mailed, with_text)
        })
        .await;
        match lines {
            Ok(Ok(lines)) if !lines.is_empty() => {
                unread.push(json!({"guest": guest, "lines": lines}));
            }
            Ok(Ok(_)) => {}
            Ok(Err(failure)) => return history_failed(failure),
            Err(failure) => return history_failed(failure),
        }
    }
    Json(json!({"unread": unread})).into_response()
}

#[derive(Deserialize)]
struct MailedLines {
    msgids: Vec<String>,
}

/// Lines zirc.dev mailed (`POST /v1/guests/mailed`).
async fn mailed_to_guests(
    State(ergo): State<Arc<Ergo>>,
    Json(mailed): Json<MailedLines>,
) -> Response {
    let now = jiff::Timestamp::now().as_second();
    match ergo.mailed.lock().await.note(mailed.msgids, now) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => ergo_failed(failure),
    }
}

async fn forget_guest(State(ergo): State<Arc<Ergo>>, Path(name): Path<String>) -> Response {
    let name = match guest_named(&name) {
        Ok(name) => name,
        Err(response) => return *response,
    };
    match forget(&ergo, &name).await {
        Ok(messages) => Json(json!({"forgotten": name, "messages": messages})).into_response(),
        Err(failure) => ergo_failed(failure),
    }
}

/// Forgets `guest`: ends their connection, deletes every message they're party to through
/// Ergo, and who they talked with. Returns how many messages went.
async fn forget(ergo: &Ergo, guest: &str) -> Result<usize, String> {
    ergo.gateway.forget(guest);
    let (path, who) = (ergo.history.clone(), guest.to_owned());
    let messages = tokio::task::spawn_blocking(move || guests::messages(&path, &who))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let deleted = ergo
        .operator
        .delete_messages(&messages)
        .await
        .map_err(|e| e.to_string())?;
    let (path, who) = (ergo.history.clone(), guest.to_owned());
    tokio::task::spawn_blocking(move || guests::drop_correspondents(&path, &who))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    // Their account too, if they had one: its always-on client goes with it.
    if exists(ergo, guest)
        .await
        .map_err(|_| "couldn't ask Ergo about the account".to_owned())?
    {
        ergo.operator
            .erase(guest)
            .await
            .map_err(|e| e.to_string())?;
    }
    info!(guest, deleted, "forgotten");
    Ok(deleted)
}

/// Every [`guests::SWEEP`], forgets the guests last there [`guests::KEPT`] or longer ago.
async fn forget_guests(ergo: Arc<Ergo>) {
    loop {
        tokio::time::sleep(guests::SWEEP).await;
        let list = match guests(&ergo).await {
            Ok(list) => list,
            Err(failure) => {
                warn!(%failure, "couldn't list the guests to forget");
                continue;
            }
        };
        for guest in guests::due(&list, jiff::Timestamp::now()) {
            if let Err(failure) = forget(&ergo, &guest).await {
                warn!(guest, %failure, "couldn't forget a guest");
            }
        }
    }
}

/// How the server is now (`GET /v1/server`).
async fn describe(State(ergo): State<Arc<Ergo>>) -> Response {
    #[derive(Deserialize)]
    struct Status {
        version: String,
        start_time: String,
        users: Users,
    }
    #[derive(Deserialize)]
    struct Users {
        total: i64,
    }
    let status: Status = match ergo.api.call("status", json!({})).await {
        Ok(status) => status,
        Err(failure) => return api_failed(failure),
    };
    let accounts = match sessions(&ergo.api).await {
        Ok(accounts) => accounts,
        Err(failure) => return api_failed(failure),
    };
    let on = |person: bool| -> i64 {
        accounts
            .iter()
            .filter(|(name, sessions)| *sessions > 0 && is_person(name) == person)
            .count()
            .try_into()
            .unwrap_or(i64::MAX)
    };
    let (people, agents) = (on(true), on(false));
    // Every person's account keeps a client while they're away (always-on), so Ergo's count
    // has them all; the rest are guests' and the gate's.
    let kept: i64 = accounts
        .iter()
        .filter(|(name, _)| is_person(name))
        .count()
        .try_into()
        .unwrap_or(i64::MAX);
    let other = (status.users.total - kept - agents).max(0);
    let dir = ergo.history.parent().unwrap_or(std::path::Path::new("."));
    Json(json!({
        "version": status.version,
        "startedAt": status.start_time,
        "connections": {"people": people, "agents": agents, "other": other},
        "storage": {
            "usedBytes": used(dir),
            "sizeBytes": u64::from(crate::limits::DISK_GIB) << 30,
        },
    }))
    .into_response()
}

/// The server's channels, for its dashboard (`GET /v1/channels`).
async fn list_channels(State(ergo): State<Arc<Ergo>>) -> Response {
    match crate::exporter::channels(&ergo.api, ergo.history.clone()).await {
        Ok(channels) => Json(json!({"channels": channels})).into_response(),
        Err(failure) => api_failed(failure),
    }
}

/// The bytes the files in `dir` take: Ergo's databases, its config and the sidecar's own.
fn used(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|metadata| metadata.len())
            .sum()
    })
}

/// The server's usage, in counts only (`usage`), for the controller.
async fn count_usage(State(ergo): State<Arc<Ergo>>) -> Response {
    match sessions(&ergo.api).await {
        Ok(accounts) => {
            let seen = ergo.seen.lock().await;
            let now = jiff::Timestamp::now();
            Json(usage::usage(&accounts, &seen, &ergo.history, now)).into_response()
        }
        Err(failure) => api_failed(failure),
    }
}

/// Every account on the server but the recorder's ([`crate::recorder`], no one's member), by
/// its lower-case name, with how many clients it has connected now (none if it's away, or only
/// kept on by always-on).
///
/// # Errors
///
/// When Ergo's API doesn't answer.
pub(crate) async fn sessions(api: &Api) -> Result<Vec<(String, i64)>, reqwest::Error> {
    #[derive(Deserialize)]
    struct Listed {
        accounts: Vec<Account>,
    }
    #[derive(Deserialize)]
    struct Account {
        #[serde(rename = "accountName")]
        name: String,
    }
    #[derive(Deserialize)]
    struct Whois {
        #[serde(default)]
        present: bool,
        #[serde(default)]
        account: Option<String>,
        #[serde(default)]
        session_count: Option<i64>,
    }
    let listed: Listed = api.call("ns/list", json!({})).await?;
    // A nick is its account's (force-nick-equals-account), so whois by name says who's on.
    let lookups = listed
        .accounts
        .into_iter()
        // zirc's own, always connected, are nobody: the recorder, and the workflows'.
        .filter(|account| !crate::workflows::is_zircs_own(&account.name))
        .map(|account| async move {
            let whois: Option<Whois> = api
                .call("whois", json!({"nickname": account.name}))
                .await
                .ok();
            let sessions = whois
                .filter(|w| {
                    w.present
                        && w.account
                            .as_deref()
                            .is_some_and(|a| a.eq_ignore_ascii_case(&account.name))
                })
                .and_then(|w| w.session_count)
                .unwrap_or(0);
            (account.name.to_lowercase(), sessions)
        });
    Ok(futures::future::join_all(lookups).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_ergo_takes() {
        for good in ["mattias", "mattias-mbp", "a_b-c9", &"a".repeat(32)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in [
            "",
            "Mattias",
            "9lives",
            "-x",
            "a b",
            "a.b",
            "é",
            &"a".repeat(33),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn tokens_compare_whole() {
        assert!(same(b"secret", b"secret"));
        assert!(!same(b"secret", b"secreT"));
        assert!(!same(b"secret", b"secret2"));
        assert!(!same(b"", b"x"));
    }
}
