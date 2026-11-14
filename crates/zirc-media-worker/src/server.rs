//! The worker's HTTP side, for zirc.dev alone (its network policy lets nothing else in), or a
//! self-hosted server beside it (`oss/crates/zirc-server`, which nothing else reaches):
//!
//! - `POST /probe` `{"source": url}`: 200 with what the video is, or 422 for something that
//!   isn't one.
//! - `POST /jobs` with a [`crate::Job`]: 202 `{"started": bool}` at once, false when it's
//!   already running or waiting; what's made appears where the job said to put it.
//! - `GET /healthz`: 200.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::{Job, Worker};

#[derive(Deserialize)]
struct ProbeRequest {
    source: String,
}

/// The worker's routes.
pub fn router(worker: Worker) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/probe", post(probe))
        .route("/jobs", post(start))
        .with_state(worker)
}

async fn probe(Json(asked): Json<ProbeRequest>) -> (StatusCode, Json<serde_json::Value>) {
    match crate::probe(&asked.source).await {
        Ok(Some(probed)) => (StatusCode::OK, Json(json!(probed))),
        Ok(None) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "not a video"})),
        ),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": error.to_string()})),
        ),
    }
}

async fn start(
    State(worker): State<Worker>,
    Json(job): Json<Job>,
) -> (StatusCode, Json<serde_json::Value>) {
    let started = worker.start(job);
    (StatusCode::ACCEPTED, Json(json!({"started": started})))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    use super::*;

    #[tokio::test]
    async fn a_job_asked_for_twice_starts_once() {
        let worker = Worker::new(1, &std::env::temp_dir());
        let job = r#"{"id":"same/480","source":"http://127.0.0.1:9/none","make":{"kind":"copy","height":480},"put":{"url":"http://127.0.0.1:9/none"}}"#;
        let ask = || {
            Request::post("/jobs")
                .header("content-type", "application/json")
                .body(Body::from(job))
                .unwrap()
        };
        let app = router(worker);
        let first = app.clone().oneshot(ask()).await.unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(first.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], br#"{"started":true}"#);
        let second = app.oneshot(ask()).await.unwrap();
        let body = axum::body::to_bytes(second.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], br#"{"started":false}"#);
    }

    #[tokio::test]
    async fn says_it_is_up() {
        let response = router(Worker::new(1, &std::env::temp_dir()))
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
