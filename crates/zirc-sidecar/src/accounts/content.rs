//! The content API: the server answers its people's requests for content itself, at its own
//! host name (`https://<host>/api/v1/…`), so none of it passes through zirc.dev. It's on only
//! where the server's desired state says so (`DesiredState::content_api`), as the controller
//! tells the sidecar with the teams (`teams::Content`); elsewhere every route is a 404.
//!
//! Each request carries the person's own token, `Authorization: Bearer <token>`, and nothing
//! shared with zirc.dev opens it:
//!
//! - a device's (`zirc_proto::device_key`, `ES256`), checked as its logins are
//!   (`crate::signin`): against the account's keys, for this server, in date, each `jti` once;
//! - or a browser login's, which zirc.dev signs for Ergo's `jwt-auth` (`EdDSA`, one of the
//!   server's `tokenKeys`), and only one that names this server as its audience.
//!
//! A machine's token (`<handle>-<machine>`) asks as its person, as zirc.dev's proxies did.
//! Guests and integrations are refused. What the person may read is the history's own rule
//! (`history::Viewer`), with the organisation's visibility; what they may change, their role:
//!
//! - anyone: `GET /me` (`{"account", "person", "role", "server"}`), `GET /history` and `GET
//!   /conversations`, as `/v1/history` and `/v1/conversations` take them; and the team's
//!   hints (`crate::hints`): `GET /hints` (a `zirc_proto::hints::Listing`), `GET /hints/log`
//!   (`{"changes"}`, newest first), `POST /hints` (`{"scope", "hint", "session"}`, answered
//!   with the change) and `DELETE /hints/<id>?session=`, a machine's asking for its agent
//!   session. Each change is said, as zirc's own account, in the repository's channel or, for
//!   the organisation's, `#ops`, which tells every machine to read them again. `GET /gotchas`
//!   lists the gotchas kept in the repositories' gotchas rooms the person may read, and `GET
//!   /playbook/usage` how often each hint and gotcha was used in the last 90 days, by id.
//! - owners and admins: `GET /guests` and `POST /guests/<name>/forget`.
//! - owners: `GET /messages?q=`, `DELETE /messages/<msgid>`, `GET /export` (newline-delimited
//!   JSON: a header, every message the owner may read, newest first, and an end line) and `GET
//!   /log` (`{"logins", "access"}`: the login log and this API's access log, newest first).
//!
//! Every change, and every export, goes in the access log, beside Ergo's history on the
//! server's own disk ([`Access`]), which owners read at `GET /log`. Reads don't.
//!
//! zirc.dev's pages ask from the person's browser, across origins: the server answers CORS for
//! zirc.dev's own origin alone (`teams::Content::origins`), with the token in the
//! `Authorization` header, never a cookie. Any other origin gets no CORS headers, so a browser
//! keeps the answer from its page.
//!
//! Errors are as the account endpoint's: 401 `unauthorized` without a good token, 403
//! `role` for a route the person's role doesn't reach, 404 `not-found` where it's off.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    Ergo, error, find_messages, forget_guest, list_conversations, list_gotchas, list_guests,
    playbook_usage, read_history, remove_message, valid_name, viewer,
};
use crate::{gateway, history, signin, teams};

/// Where the routes are, on the server's host name.
pub const PREFIX: &str = "/api/v1";

/// Who's asking, as their token says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// The account the token is for: the person's, or one of their machines'.
    pub account: String,
    /// The person: the account's handle.
    pub person: String,
    /// Their role in the organisation.
    pub role: Role,
}

/// A person's role, as zirc.dev's desired state gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Anyone in the organisation.
    Member,
    /// An admin: guests too.
    Admin,
    /// An owner: everything.
    Owner,
}

/// One entry of the access log: who changed or took what, through this API.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    /// When, in seconds since the epoch.
    pub at: i64,
    /// The account whose token it was.
    pub account: String,
    /// The request: its method and path, such as `DELETE /api/v1/messages/<msgid>`.
    pub request: String,
    /// The status it was answered with.
    pub status: u16,
}

impl signin::Dated for Access {
    fn at(&self) -> i64 {
        self.at
    }
}

/// The access log beside Ergo's history.
#[must_use]
pub fn access_log(history: &std::path::Path, now: i64) -> signin::Log<Access> {
    signin::Log::at(history.with_file_name("zirc-access.jsonl"), now)
}

/// The account a browser login's token is for, if one of `keys` (PEM) signed it for `server`
/// and it's in date at `now`. The same tokens Ergo's `jwt-auth` takes (`irc-tokens.ts`), but
/// only those with this server as their audience.
///
/// # Errors
///
/// Why it isn't one.
pub fn browser_account(
    token: &str,
    keys: &[String],
    server: &str,
    now: u64,
) -> Result<String, &'static str> {
    #[derive(Deserialize)]
    struct Claims {
        account: String,
        #[serde(default)]
        aud: Option<String>,
        exp: u64,
    }
    let mut parts = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("not a token");
    };
    let header: serde_json::Value = URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|h| serde_json::from_slice(&h).ok())
        .ok_or("not a token")?;
    if header["alg"] != "EdDSA" {
        return Err("not a token");
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "not a token")?;
    let signed = &token[..token.len() - token.rsplit('.').next().unwrap_or_default().len() - 1];
    let good = keys
        .iter()
        .filter_map(|k| gateway::public_key(k))
        .any(|key| {
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
                .verify(signed.as_bytes(), &signature)
                .is_ok()
        });
    if !good {
        return Err("not signed for this server");
    }
    let claims: Claims = URL_SAFE_NO_PAD
        .decode(claims)
        .ok()
        .and_then(|c| serde_json::from_slice(&c).ok())
        .ok_or("not a token")?;
    if !claims
        .aud
        .as_deref()
        .is_some_and(|aud| aud.eq_ignore_ascii_case(server))
    {
        return Err("for another server");
    }
    if claims.exp <= now {
        return Err("expired");
    }
    Ok(claims.account.to_lowercase())
}

/// The person an account belongs to, if it's a person's or one of the `machines`': never a
/// guest's or an integration's.
#[must_use]
pub fn person_of(account: &str, machines: &crate::machines::Machines) -> Option<String> {
    if !valid_name(account)
        || zirc_proto::names::is_guest_account(account)
        || zirc_proto::names::is_integration_nick(account)
    {
        return None;
    }
    if let Some(owner) = machines.owner(account) {
        return Some(owner);
    }
    // Anyone else's is theirs, if it could be a handle.
    zirc_proto::names::is_handle(&account.to_lowercase()).then(|| account.to_lowercase())
}

/// The role `person` has, by the owners and admins the server was last told of.
#[must_use]
pub fn role_of(
    person: &str,
    owners: &std::collections::BTreeSet<String>,
    content: &teams::Content,
) -> Role {
    if owners.contains(person) {
        Role::Owner
    } else if content.admins.contains(person) {
        Role::Admin
    } else {
        Role::Member
    }
}

/// The content API's routes, under [`PREFIX`].
pub(super) fn router(ergo: Arc<Ergo>) -> Router {
    let at = |path: &str| format!("{PREFIX}{path}");
    let anyone = Router::new()
        .route(&at("/me"), get(me))
        .route(&at("/history"), get(read_history))
        .route(&at("/conversations"), get(list_conversations))
        .route(&at("/hints"), get(list_hints).post(add_hint))
        .route(&at("/hints/log"), get(hints_log))
        .route(&at("/hints/{id}"), delete(remove_hint))
        .route(&at("/gotchas"), get(list_gotchas))
        .route(&at("/playbook/usage"), get(playbook_usage));
    let staff = Router::new()
        .route(&at("/guests"), get(list_guests))
        .route(&at("/guests/{name}/forget"), post(forget_guest))
        .route_layer(middleware::from_fn(|r, n| at_least(Role::Admin, r, n)));
    let owners = Router::new()
        .route(&at("/messages"), get(find_messages))
        .route(&at("/messages/{msgid}"), delete(remove_message))
        .route(&at("/export"), get(export))
        .route(&at("/log"), get(log))
        .route_layer(middleware::from_fn(|r, n| at_least(Role::Owner, r, n)));
    anyone
        .merge(staff)
        .merge(owners)
        .route_layer(middleware::from_fn_with_state(ergo.clone(), audit))
        .route_layer(middleware::from_fn_with_state(ergo.clone(), authenticate))
        .layer(middleware::from_fn_with_state(ergo.clone(), cors))
        .with_state(ergo)
}

/// What a browser may send and read across origins.
const ALLOW_METHODS: &str = "GET, POST, DELETE";
const ALLOW_HEADERS: &str = "Authorization, Content-Type";
const EXPOSE_HEADERS: &str = "Retry-After, Content-Disposition";

/// Whether a browser at `origin` may ask, by the origins the server was told of.
#[must_use]
pub fn allowed_origin(origin: &str, content: Option<&teams::Content>) -> bool {
    content.is_some_and(|c| c.origins.contains(origin))
}

/// CORS for zirc.dev's pages: a preflight answered here, and the answer marked readable by the
/// page that asked, when it's zirc.dev's.
async fn cors(State(ergo): State<Arc<Ergo>>, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .map(str::to_owned);
    let allowed = match &origin {
        Some(origin) => allowed_origin(origin, ergo.content.lock().await.as_ref()),
        None => false,
    };
    let preflight = request.method() == Method::OPTIONS;
    let mut response = if preflight {
        if allowed {
            StatusCode::NO_CONTENT.into_response()
        } else {
            error(
                StatusCode::FORBIDDEN,
                "origin",
                "this origin may not ask here",
            )
        }
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if let (true, Some(origin)) = (allowed, origin)
        && let Ok(origin) = HeaderValue::from_str(&origin)
    {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(EXPOSE_HEADERS),
        );
        if preflight {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static(ALLOW_METHODS),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static(ALLOW_HEADERS),
            );
            headers.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_static("600"),
            );
        }
    }
    response
}

fn now() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_second()).unwrap_or_default()
}

/// `GET /hints`: the team's hints.
async fn list_hints(State(ergo): State<Arc<Ergo>>) -> Response {
    Json(ergo.hints.listing().await).into_response()
}

/// `GET /hints/log`: the changes to them, newest first.
async fn hints_log(State(ergo): State<Arc<Ergo>>) -> Response {
    Json(json!({"changes": ergo.hints.log().await})).into_response()
}

/// What `POST /hints` is given.
#[derive(Deserialize)]
struct NewHint {
    scope: String,
    hint: zirc_proto::hints::Hint,
    /// The agent session asking, when a machine is.
    #[serde(default)]
    session: Option<String>,
}

/// What `DELETE /hints/<id>` is asked.
#[derive(Deserialize)]
struct Asking {
    #[serde(default)]
    session: Option<String>,
}

/// Who `caller` is to the hints: a machine's account, whose agents may be asking, unless it's
/// one of the person's own clients.
fn hints_who(ergo: &Ergo, caller: &Caller, session: Option<String>) -> crate::hints::Who {
    let machine =
        caller.account != caller.person && !ergo.machines.clients().contains(&caller.account);
    crate::hints::Who {
        account: caller.account.clone(),
        machine,
        session: session.filter(|session| {
            machine
                && (1..=64).contains(&session.len())
                && session
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        }),
    }
}

/// `POST /hints`: adds one.
async fn add_hint(
    State(ergo): State<Arc<Ergo>>,
    Extension(caller): Extension<Caller>,
    Json(given): Json<NewHint>,
) -> Response {
    let who = hints_who(&ergo, &caller, given.session);
    let at = jiff::Timestamp::now().to_string();
    let scope = given.scope.to_lowercase();
    let made = ergo
        .hints
        .change(|kept| kept.add(&scope, given.hint, &who, &at))
        .await;
    changed(&ergo, made, StatusCode::CREATED)
}

/// `DELETE /hints/<id>`: removes one.
async fn remove_hint(
    State(ergo): State<Arc<Ergo>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Query(asking): Query<Asking>,
) -> Response {
    let who = hints_who(&ergo, &caller, asking.session);
    let at = jiff::Timestamp::now().to_string();
    let made = ergo.hints.change(|kept| kept.remove(&id, &who, &at)).await;
    changed(&ergo, made, StatusCode::OK)
}

/// The answer to a change of the hints, and, made, the change said for every machine.
fn changed(
    ergo: &Arc<Ergo>,
    made: Result<zirc_proto::hints::Change, crate::hints::Refusal>,
    status: StatusCode,
) -> Response {
    use crate::hints::Refusal;
    match made {
        Ok(change) => {
            tokio::spawn(super::tell_hints_changed(ergo.clone(), change.clone()));
            (status, Json(change)).into_response()
        }
        Err(Refusal::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(Refusal::NotFound) => error(StatusCode::NOT_FOUND, "not-found", "no hint has that id"),
        Err(Refusal::Exists) => error(StatusCode::CONFLICT, "exists", "that hint is there already"),
    }
}

/// Who the token says is asking; then the request goes on as theirs (`X-Zirc-As`, as the
/// handlers zirc.dev called read it), whatever the request said.
async fn authenticate(State(ergo): State<Arc<Ergo>>, mut request: Request, next: Next) -> Response {
    let Some(content) = ergo.content.lock().await.clone() else {
        return error(
            StatusCode::NOT_FOUND,
            "not-found",
            "this server doesn't serve content here",
        );
    };
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let refused = |why: &str| {
        error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            format!("a token of yours for this server is needed: {why}"),
        )
    };
    let Some(token) = token else {
        return refused("there's none");
    };
    let account = if zirc_proto::device_key::parse(&token).is_ok() {
        match ergo.signin.check_for(&token, &content.server, now()).await {
            Ok((account, _, _)) => account,
            Err((_, why)) => return refused(why),
        }
    } else {
        let keys = ergo.token_keys.lock().await.clone();
        match browser_account(&token, &keys, &content.server, now()) {
            Ok(account) => account,
            Err(why) => return refused(why),
        }
    };
    let Some(person) = person_of(&account, &ergo.machines) else {
        return error(
            StatusCode::FORBIDDEN,
            "role",
            "only the organisation's people and their machines ask here",
        );
    };
    let role = role_of(&person, &ergo.owners.lock().await.accounts, &content);
    let Ok(as_person) = HeaderValue::from_str(&person) else {
        return refused("not an account");
    };
    request.headers_mut().insert("x-zirc-as", as_person);
    request.extensions_mut().insert(Caller {
        account,
        person,
        role,
    });
    next.run(request).await
}

/// Lets the request on only for `role` or above.
async fn at_least(role: Role, request: Request, next: Next) -> Response {
    let reaches = request
        .extensions()
        .get::<Caller>()
        .is_some_and(|caller| caller.role >= role);
    if reaches {
        next.run(request).await
    } else {
        let who = match role {
            Role::Owner => "the organisation's owners",
            Role::Admin | Role::Member => "the organisation's owners and admins",
        };
        error(StatusCode::FORBIDDEN, "role", format!("only {who} may"))
    }
}

/// Writes every change, and every export, in the access log, with how it was answered.
async fn audit(State(ergo): State<Arc<Ergo>>, request: Request, next: Next) -> Response {
    let logged = request.method() != Method::GET || request.uri().path().ends_with("/export");
    let entry = request.extensions().get::<Caller>().map(|caller| {
        (
            caller.account.clone(),
            format!("{} {}", request.method(), request.uri().path()),
        )
    });
    let response = next.run(request).await;
    if logged && let Some((account, request)) = entry {
        ergo.access.add(&Access {
            at: i64::try_from(now()).unwrap_or(i64::MAX),
            account,
            request,
            status: response.status().as_u16(),
        });
    }
    response
}

async fn me(State(ergo): State<Arc<Ergo>>, Extension(caller): Extension<Caller>) -> Response {
    let server = ergo.content.lock().await.as_ref().map(|c| c.server.clone());
    Json(json!({
        "account": caller.account,
        "person": caller.person,
        "role": caller.role,
        "server": server,
    }))
    .into_response()
}

/// The login log and the access log, newest first.
async fn log(State(ergo): State<Arc<Ergo>>) -> Response {
    let mut logins = ergo.signin.log.read();
    logins.reverse();
    let mut access = ergo.access.read();
    access.reverse();
    Json(json!({"logins": logins, "access": access})).into_response()
}

/// How many messages the export reads at a time.
const EXPORT_PAGE: usize = 200;

/// Everything the owner may read, as newline-delimited JSON, streamed as it's read: never
/// kept anywhere.
async fn export(
    State(ergo): State<Arc<Ergo>>,
    headers: axum::http::HeaderMap,
    Extension(caller): Extension<Caller>,
) -> Response {
    let viewer = match viewer(&ergo, &headers).await {
        Ok(viewer) => viewer,
        Err(response) => return *response,
    };
    let server = ergo.content.lock().await.as_ref().map(|c| c.server.clone());
    let (lines, out) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(4);
    let path = ergo.history.clone();
    let header = json!({
        "type": "export",
        "server": server,
        "by": caller.person,
        "exportedAt": jiff::Timestamp::now().to_string(),
    });
    tokio::task::spawn_blocking(move || export_lines(&path, &viewer, &header, &lines));
    let body = Body::from_stream(futures::stream::unfold(out, |mut out| async move {
        out.recv().await.map(|line| (line, out))
    }));
    (
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"export.ndjson\"",
            ),
        ],
        body,
    )
        .into_response()
}

/// Sends the export's lines down `lines`, page by page, until it's done or nobody's reading.
fn export_lines(
    path: &std::path::Path,
    viewer: &history::Viewer,
    header: &serde_json::Value,
    lines: &tokio::sync::mpsc::Sender<Result<String, std::io::Error>>,
) {
    let send = |line: String| lines.blocking_send(Ok(line + "\n")).is_ok();
    if !send(header.to_string()) {
        return;
    }
    let mut query = history::Query {
        since: 0,
        until: i64::MAX,
        limit: EXPORT_PAGE,
        ..history::Query::default()
    };
    loop {
        let page = match history::search(path, viewer, &query) {
            Ok(page) => page,
            Err(failure) => {
                let _ = lines.blocking_send(Err(std::io::Error::other(failure.to_string())));
                return;
            }
        };
        for message in &page.messages {
            let mut line = serde_json::to_value(message).unwrap_or_default();
            line["type"] = json!("message");
            if !send(line.to_string()) {
                return;
            }
        }
        match page.next.as_deref().map(str::parse) {
            Some(Ok(cursor)) => query.before = Some(cursor),
            _ => break,
        }
    }
    send(json!({"type": "end"}).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn pair() -> Ed25519KeyPair {
        let rng = ring::rand::SystemRandom::new();
        Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref()).unwrap()
    }

    fn pem(key: &Ed25519KeyPair) -> String {
        use base64::engine::general_purpose::STANDARD;
        let mut der = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        der.extend_from_slice(key.public_key().as_ref());
        format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(der)
        )
    }

    fn token(key: &Ed25519KeyPair, claims: &serde_json::Value) -> String {
        let part = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
        let data = format!(
            "{}.{}",
            part(&json!({"alg": "EdDSA", "typ": "JWT"})),
            part(claims)
        );
        let signature = key.sign(data.as_bytes());
        format!("{data}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
    }

    const SERVER: &str = "acme.irc.zirc.dev";

    #[test]
    fn a_browser_login_for_this_server_names_its_account() {
        let (old, new) = (pair(), pair());
        let keys = vec![pem(&new), pem(&old)];
        let good = json!({"account": "Ada", "aud": SERVER, "exp": 200, "jti": "x"});
        assert_eq!(
            browser_account(&token(&old, &good), &keys, SERVER, 100),
            Ok("ada".into())
        );
        assert_eq!(
            browser_account(&token(&new, &good), &keys, "ACME.irc.zirc.dev", 100),
            Ok("ada".into())
        );
    }

    #[test]
    fn a_browser_login_for_anywhere_else_or_out_of_date_or_unsigned_isnt() {
        let key = pair();
        let keys = vec![pem(&key)];
        let claims = |aud: Option<&str>, exp: u64| {
            let mut c = json!({"account": "ada", "exp": exp});
            if let Some(aud) = aud {
                c["aud"] = json!(aud);
            }
            c
        };
        let refused = |t: &str| browser_account(t, &keys, SERVER, 100).unwrap_err();
        assert_eq!(
            refused(&token(&key, &claims(None, 200))),
            "for another server"
        );
        assert_eq!(
            refused(&token(&key, &claims(Some("beta.irc.zirc.dev"), 200))),
            "for another server"
        );
        assert_eq!(refused(&token(&key, &claims(Some(SERVER), 100))), "expired");
        assert_eq!(
            refused(&token(&pair(), &claims(Some(SERVER), 200))),
            "not signed for this server"
        );
        assert_eq!(refused("a.b"), "not a token");
        // A guest's token has no account.
        assert_eq!(
            refused(&token(
                &key,
                &json!({"guest": "guest-1", "channel": "#help", "aud": SERVER, "exp": 200})
            )),
            "not a token"
        );
    }

    #[test]
    fn machines_ask_as_their_person_and_guests_and_integrations_not_at_all() {
        let none = crate::machines::Machines::default();
        assert_eq!(person_of("ada", &none), Some("ada".into()));
        // A machine is its owner's as the server was told, never by its name.
        assert_eq!(person_of("ada-laptop", &none), None);
        let told = crate::machines::Machines::default();
        told.add("m3-mac", "ada");
        assert_eq!(person_of("m3-mac", &told), Some("ada".into()));
        assert_eq!(person_of("guest-21042", &none), None);
        assert_eq!(person_of("zirc-github", &none), None);
        assert_eq!(person_of("Not A Name", &none), None);
    }

    #[test]
    fn roles_come_from_the_owners_and_admins() {
        let owners = ["ada".to_owned()].into();
        let content = teams::Content {
            server: SERVER.into(),
            admins: ["grace".to_owned()].into(),
            origins: ["https://zirc.dev".to_owned()].into(),
        };
        assert_eq!(role_of("ada", &owners, &content), Role::Owner);
        assert_eq!(role_of("grace", &owners, &content), Role::Admin);
        assert_eq!(role_of("linus", &owners, &content), Role::Member);
        assert!(Role::Owner > Role::Admin && Role::Admin > Role::Member);
    }

    #[test]
    fn only_zirc_devs_own_origin_may_ask_from_a_browser() {
        let content = teams::Content {
            server: SERVER.into(),
            admins: std::collections::BTreeSet::new(),
            origins: ["https://zirc.dev".to_owned()].into(),
        };
        assert!(allowed_origin("https://zirc.dev", Some(&content)));
        assert!(!allowed_origin("https://evil.example", Some(&content)));
        assert!(!allowed_origin(
            "https://zirc.dev.evil.example",
            Some(&content)
        ));
        assert!(!allowed_origin("null", Some(&content)));
        // Nobody while the content API is off.
        assert!(!allowed_origin("https://zirc.dev", None));
    }

    #[test]
    fn the_access_log_keeps_what_was_changed_and_forgets_it_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("ergo_history.db");
        let log = access_log(&history, 0);
        let entry = Access {
            at: 10,
            account: "ada".into(),
            request: "DELETE /api/v1/messages/abc".into(),
            status: 200,
        };
        log.add(&entry);
        assert_eq!(access_log(&history, 20).read(), vec![entry]);
        assert!(access_log(&history, 10 + signin::KEPT).read().is_empty());
    }
}
