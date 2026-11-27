//! The server's HTTPS side: what a hosted server's edge routes to its sidecar, on one listener.
//!
//! - `/api/…`: the content API, where apps read history with a short token zirc.dev gave them
//!   (`zirc_proto::login::ContentAccess`), so what people said never passes through zirc.dev.
//! - `/api/media…` and `/m/…`: the server's own files ([`crate::media`]), never passed on, and
//!   `/s/…`, links to their bytes on the server's disk ([`crate::store`]).
//! - `/sidecar/…` ([`zirc_proto::join::SIDECAR_PATH`]): the sidecar's own endpoint, which
//!   zirc.dev and its tenant controller manage the server through, with the tokens the server
//!   gave when it joined. A hosted server's is on the cluster's network alone.
//!
//! Each is passed on to the sidecar on loopback as it came, but for the hop's own headers.
//! `/healthz` says when the server started, as zirc.dev's controller reads it: what it put
//! in force before then, such as settings, it puts in force again.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use serde_json::json;
use zirc_proto::join::SIDECAR_PATH;

use crate::run::Inside;

/// The most a request may send: settings and teams are small.
const LARGEST_BODY: usize = 4 * 1024 * 1024;

/// The headers passed on each way: what the sidecar reads and answers with, never the hop's.
const PASSED: [header::HeaderName; 16] = [
    header::AUTHORIZATION,
    header::CONTENT_TYPE,
    header::ACCEPT,
    header::CACHE_CONTROL,
    header::ETAG,
    // A browser's ask across origins, zirc.dev's pages', and the sidecar's answer to it
    // (zirc-sidecar's accounts::content): without them every preflight was refused.
    header::ORIGIN,
    header::ACCESS_CONTROL_REQUEST_METHOD,
    header::ACCESS_CONTROL_REQUEST_HEADERS,
    header::ACCESS_CONTROL_ALLOW_ORIGIN,
    header::ACCESS_CONTROL_ALLOW_METHODS,
    header::ACCESS_CONTROL_ALLOW_HEADERS,
    header::ACCESS_CONTROL_EXPOSE_HEADERS,
    header::ACCESS_CONTROL_MAX_AGE,
    header::VARY,
    header::RETRY_AFTER,
    header::CONTENT_DISPOSITION,
];

struct Backends {
    content: String,
    sidecar: String,
    http: reqwest::Client,
}

/// The routes, to the sidecar's listeners in `inside`, and the server's own files' (`media`,
/// with their links in `store`), which take uploads larger than what's passed on.
pub fn router(
    inside: &Inside,
    media: Arc<crate::media::Media>,
    store: Arc<crate::store::Store>,
) -> Router {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let backends = Arc::new(Backends {
        content: format!("http://{}", inside.content_listen),
        sidecar: format!("http://{}", inside.accounts_listen),
        http: zirc_sidecar::http(),
    });
    Router::new()
        .route(
            "/healthz",
            get(move || async move { Json(json!({"started": started})) }),
        )
        .route("/api/{*rest}", any(content))
        .route(&format!("{SIDECAR_PATH}/{{*rest}}"), any(sidecar))
        .layer(axum::extract::DefaultBodyLimit::max(LARGEST_BODY))
        .with_state(backends)
        .merge(crate::media::router(media))
        .merge(crate::store::router(store))
}

async fn content(
    State(backends): State<Arc<Backends>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    pass(&backends, &backends.content, path, method, &headers, body).await
}

async fn sidecar(
    State(backends): State<Arc<Backends>>,
    Path(rest): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let path = format!("/{rest}{query}");
    pass(&backends, &backends.sidecar, &path, method, &headers, body).await
}

/// Passes the request on to `base` at `path`, and its answer back.
async fn pass(
    backends: &Backends,
    base: &str,
    path: &str,
    method: Method,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let mut request = backends
        .http
        .request(method, format!("{base}{path}"))
        .body(body);
    for name in &PASSED {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    let answer = match request.send().await {
        Ok(answer) => answer,
        Err(e) => {
            tracing::warn!("the sidecar didn't answer: {e}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let mut response = Response::builder().status(answer.status());
    for name in &PASSED {
        if let Some(value) = answer.headers().get(name) {
            response = response.header(name, value);
        }
    }
    match answer.bytes().await {
        Ok(bytes) => response
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}
