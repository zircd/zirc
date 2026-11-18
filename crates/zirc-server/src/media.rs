//! A self-hosted server's files: what people share in chat, kept on the server's own disk or
//! in the organisation's own bucket ([`crate::store`]), so they never reach zirc.dev. The API is zirc.dev's (`zirc_proto::media`),
//! at the server's own address, and so are its rules (`zirc_proto::uploads`, `files`,
//! `images`): what an upload may be, what it becomes once its bytes are in, and what a text
//! file's preview shows. People's pictures stay on zirc.dev.
//!
//! An app signs in as it does to zirc.dev, with its account's name and password over HTTP
//! Basic: Ergo checks them (`/v1/check_auth`), and the sidecar says whose account it is and
//! their role. The bytes go to the store through a link that lasts
//! [`uploads::UPLOAD_SECONDS`]; `/m/<id>` sends anyone with an account to a link for them.
//!
//! Under `<data>/media`, whichever store has the bytes: each file's record as `<id>.json`,
//! and the organisation's settings as `settings.json`, from zirc.dev:
//! in its answer as the server joins, then at [`zirc_proto::media::SETTINGS_PATH`] under
//! the sidecar's path, with the accounts token, when its owners change them.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path as Id, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;
use zirc_proto::File;
use zirc_proto::files::{self, PREVIEW_BYTES};
use zirc_proto::media::{Preview, Purpose, UPLOAD_PATH, UploadRequest, UploadStarted};
use zirc_proto::role::Role;
use zirc_proto::uploads::{self, Arrived, Refused, Settings, Stored};

use crate::secrets::Secrets;
use crate::store::{For, Served, Store};
use zirc_media_worker::{Job, Make};

/// How long a checked password is taken without asking Ergo again: a chat's pictures load at
/// once, and Ergo's check is a bcrypt.
const CHECKED_FOR: Duration = Duration::from_secs(300);

/// How many wrong passwords an account takes in [`MISSED_FOR`] before the files' service stops
/// asking Ergo for it: anyone can try, and each try is a bcrypt.
const MISSES: u32 = 10;

/// How long wrong passwords count against an account.
const MISSED_FOR: Duration = Duration::from_secs(600);

/// How much of a file is read to tell what it is.
const START_BYTES: usize = 64 * 1024;

/// How long a link to a file's bytes lasts.
const LINK_SECONDS: u32 = 300;

/// How long imgproxy has to read an original: a large one takes a while.
const SOURCE_SECONDS: u32 = 600;

/// How long the media worker has to read a video and put what it makes from it.
const MAKING_SECONDS: u32 = 3600;

/// How long a link to play a video lasts, seeking included.
const PLAY_SECONDS: u32 = 1800;

/// The services beside the server that make what's shown from what's shared, each reached at
/// its address, if it's there: without one, the original is served.
#[derive(Debug, Clone, Default)]
pub struct Beside {
    /// imgproxy, which resizes images.
    pub imgproxy: Option<String>,
    /// The media worker (`zirc-media-worker`), which reads videos and makes their posters
    /// and the copies people play. Without it, video is shared as a file.
    pub worker: Option<String>,
}

/// The files' service: where they're kept and how the server's own parts are reached.
pub struct Media {
    /// Where their bytes are.
    store: Arc<Store>,
    /// What makes what's shown from them.
    beside: Beside,
    /// `<data>/media`, where their records are.
    dir: PathBuf,
    /// The data directory, for the server's tokens, which joining zirc.dev changes.
    data: PathBuf,
    /// The server's address, `https://<host>`, where files are linked from.
    base: String,
    /// Ergo's API, for checking passwords.
    ergo_api: String,
    /// The sidecar's account endpoint, for whose an account is.
    accounts: String,
    http: reqwest::Client,
    random: SystemRandom,
    /// When Ergo last said a password was right, by a digest of the account and password. Only
    /// that is kept: whose an account is and their role are asked each time, so a new admin
    /// is one at once.
    checked: Mutex<HashMap<[u8; 32], Instant>>,
    /// Wrong passwords lately, by account (lower case): how many, since when ([`MISSES`]).
    missed: Mutex<HashMap<String, (u32, Instant)>>,
}

impl Media {
    /// The files' service for the server at `host`, with their bytes in `store` and their
    /// records in `data`, what's shown made by `beside`, reaching Ergo's API and the
    /// sidecar's account endpoint at those addresses.
    #[must_use]
    pub fn new(
        store: Arc<Store>,
        beside: Beside,
        host: &str,
        data: &Path,
        ergo_api: String,
        accounts: String,
    ) -> Media {
        Media {
            store,
            beside,
            dir: data.join("media"),
            data: data.to_owned(),
            base: format!("https://{host}"),
            ergo_api,
            accounts,
            http: zirc_sidecar::http(),
            random: SystemRandom::new(),
            checked: Mutex::default(),
            missed: Mutex::default(),
        }
    }
}

/// Someone signed in: their account, the person it's of, and their role.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Who {
    #[serde(skip)]
    account: String,
    person: String,
    role: Role,
}

/// A file's record, beside its bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    /// The person who uploaded it.
    uploader: String,
    stored: Stored,
    name: String,
    mime: String,
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel: Option<String>,
    /// When it was started, in seconds.
    created: u64,
    /// While its bytes are being sent: until when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sending: Option<Sending>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preview: Option<Preview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sending {
    until: u64,
}

/// Keeps `settings`, the organisation's files settings as zirc.dev sent them, in `data`: the
/// settings they come to, never anything else that was sent.
///
/// # Errors
///
/// When they can't be written.
pub fn keep_settings(data: &Path, settings: &serde_json::Value) -> std::io::Result<()> {
    let dir = data.join("media");
    std::fs::create_dir_all(&dir)?;
    let settled = serde_json::to_vec(&Settings::from_stored(settings))?;
    let part = dir.join("settings.json.part");
    std::fs::write(&part, settled)?;
    std::fs::rename(part, dir.join("settings.json"))
}

/// The routes, at zirc.dev's paths, `/m/<id>` for downloads, and where zirc.dev sends the
/// settings.
pub fn router(media: Arc<Media>) -> Router {
    Router::new()
        .route(
            &format!(
                "{}{}",
                zirc_proto::join::SIDECAR_PATH,
                zirc_proto::media::SETTINGS_PATH
            ),
            put(set_settings),
        )
        .route(UPLOAD_PATH, post(start))
        .route(&format!("{UPLOAD_PATH}/{{id}}/done"), post(done))
        .route(&format!("{UPLOAD_PATH}/{{id}}/preview"), get(preview))
        .route(
            &format!("{UPLOAD_PATH}/{{id}}"),
            axum::routing::delete(remove),
        )
        .route("/m/{id}", get(download))
        .with_state(media)
}

/// Why someone isn't let in: the status and what they're told.
type Denied = (u16, &'static str);

const fn denied(status: u16, why: &'static str) -> Denied {
    (status, why)
}

/// An answer saying what's wrong, as zirc.dev's do.
fn refused(status: u16, why: &str) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
    (status, Json(json!({ "error": why }))).into_response()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Media {
    fn record_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// The record of the file `id`, if it could be one and is there.
    async fn record(&self, id: &str) -> Option<Record> {
        if !uploads::plausible_id(id) {
            return None;
        }
        let text = tokio::fs::read(self.record_path(id)).await.ok()?;
        serde_json::from_slice(&text).ok()
    }

    async fn keep(&self, id: &str, record: &Record) -> std::io::Result<()> {
        let path = self.record_path(id);
        let part = path.with_extension("json.part");
        tokio::fs::write(&part, serde_json::to_vec(record)?).await?;
        tokio::fs::rename(part, path).await
    }

    /// Removes the file `id` and everything of it.
    async fn forget(&self, id: &str) {
        self.store.remove(&bytes_key(id)).await;
        self.store.remove(&format!("cache/{id}/")).await;
        let _ = tokio::fs::remove_file(self.record_path(id)).await;
    }

    /// The organisation's settings, as zirc.dev last sent them, or the defaults.
    async fn settings(&self) -> Settings {
        let stored = tokio::fs::read(self.dir.join("settings.json"))
            .await
            .ok()
            .and_then(|text| serde_json::from_slice(&text).ok())
            .unwrap_or_default();
        Settings::from_stored(&stored)
    }

    fn random(&self, bytes: usize) -> Option<String> {
        let mut buffer = vec![0; bytes];
        self.random.fill(&mut buffer).ok()?;
        Some(URL_SAFE_NO_PAD.encode(buffer))
    }

    fn file(&self, id: &str, record: &Record) -> File {
        File {
            url: format!("{}/m/{id}", self.base),
            name: record.name.clone(),
            mime: record.mime.clone(),
            size: record.size,
            width: record.width,
            height: record.height,
            duration_ms: record.duration_ms,
        }
    }

    /// How many wrong passwords `account` has had in [`MISSED_FOR`].
    fn misses(&self, account: &str) -> u32 {
        self.missed
            .lock()
            .ok()
            .and_then(|missed| missed.get(account).copied())
            .filter(|(_, since)| since.elapsed() < MISSED_FOR)
            .map_or(0, |(count, _)| count)
    }

    /// Counts a wrong password for `account`, forgetting what's older than [`MISSED_FOR`].
    fn missed(&self, account: String) {
        if let Ok(mut missed) = self.missed.lock() {
            missed.retain(|_, (_, since)| since.elapsed() < MISSED_FOR);
            missed
                .entry(account)
                .and_modify(|(count, _)| *count += 1)
                .or_insert((1, Instant::now()));
        }
    }

    /// Who `headers` sign in as, with an account's name and password over HTTP Basic.
    async fn who(&self, headers: &HeaderMap) -> Result<Who, Denied> {
        let unknown = || denied(401, "Sign in with your account's name and password.");
        let (account, password) = basic(headers).ok_or_else(unknown)?;
        let key = digest(&account, &password);
        let secrets =
            Secrets::read(&self.data).map_err(|_| denied(503, "The server isn't ready."))?;
        let fresh = self
            .checked
            .lock()
            .ok()
            .and_then(|checked| checked.get(&key).copied())
            .is_some_and(|at| at.elapsed() < CHECKED_FOR);
        let named = account.to_lowercase();
        if !fresh && self.misses(&named) >= MISSES {
            return Err(denied(
                429,
                "Too many wrong passwords for that account: try again in a few minutes.",
            ));
        }
        if !fresh {
            let checked: CheckAuth = self
                .http
                .post(format!("{}/v1/check_auth", self.ergo_api))
                .bearer_auth(&secrets.api_token)
                .json(&json!({"accountName": account, "passphrase": password}))
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|_| denied(503, "The server couldn't check that."))?
                .json()
                .await
                .map_err(|_| denied(503, "The server couldn't check that."))?;
            if !checked.success {
                self.missed(named);
                return Err(unknown());
            }
            if let Ok(mut checked) = self.checked.lock() {
                checked.retain(|_, at| at.elapsed() < CHECKED_FOR);
                checked.insert(key, Instant::now());
            }
        }
        let answer = self
            .http
            .get(format!("{}/v1/accounts/{account}/role", self.accounts))
            .bearer_auth(&secrets.accounts_token)
            .send()
            .await
            .map_err(|_| denied(503, "The server couldn't check that."))?;
        if answer.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(denied(
                403,
                "Only the organisation's people and their machines share files.",
            ));
        }
        let mut who: Who = answer
            .error_for_status()
            .map_err(|_| denied(503, "The server couldn't check that."))?
            .json()
            .await
            .map_err(|_| denied(503, "The server couldn't check that."))?;
        who.account = account;
        Ok(who)
    }
}

/// The organisation's files settings from zirc.dev, with the accounts token: 204.
async fn set_settings(
    State(media): State<Arc<Media>>,
    headers: HeaderMap,
    Json(settings): Json<serde_json::Value>,
) -> Response {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Ok(secrets) = Secrets::read(&media.data) else {
        return refused(503, "The server isn't ready.");
    };
    if !presented.is_some_and(|token| {
        zirc_sidecar::secrets::same(token.as_bytes(), secrets.accounts_token.as_bytes())
    }) {
        return refused(401, "A valid bearer token is needed.");
    }
    match keep_settings(&media.data, &settings) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => refused(503, "The server couldn't keep them."),
    }
}

#[derive(Deserialize)]
struct CheckAuth {
    success: bool,
}

/// The account and password HTTP Basic says.
fn basic(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let (account, password) = decoded.split_once(':')?;
    (!account.is_empty() && !password.is_empty())
        .then(|| (account.to_lowercase(), password.to_owned()))
}

fn digest(account: &str, password: &str) -> [u8; 32] {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    context.update(account.as_bytes());
    context.update(&[0]);
    context.update(password.as_bytes());
    let mut key = [0; 32];
    key.copy_from_slice(context.finish().as_ref());
    key
}

/// Starts an upload: its bytes then go to the address answered.
async fn start(
    State(media): State<Arc<Media>>,
    headers: HeaderMap,
    Json(asked): Json<UploadRequest>,
) -> Response {
    let who = match media.who(&headers).await {
        Ok(who) => who,
        Err((status, why)) => return refused(status, why),
    };
    if matches!(asked.purpose, Purpose::Avatar | Purpose::AccountAvatar) {
        return refused(400, "Pictures are kept on zirc.dev: upload them there.");
    }
    let mut settings = media.settings().await;
    settings.video &= media.beside.worker.is_some();
    let starting = match uploads::start(
        &settings,
        asked.purpose,
        &asked.name,
        &asked.mime,
        asked.size,
    ) {
        Ok(starting) => starting,
        Err(Refused { status, why }) => return refused(status, &why),
    };
    let Some(id) = media.random(uploads::ID_BYTES) else {
        return refused(503, "The server couldn't start that.");
    };
    let record = Record {
        uploader: who.person,
        stored: starting.stored,
        name: starting.name,
        mime: starting.mime.clone(),
        size: asked.size,
        channel: asked.channel,
        created: now(),
        sending: Some(Sending {
            until: now() + u64::from(uploads::UPLOAD_SECONDS),
        }),
        width: None,
        height: None,
        duration_ms: None,
        preview: None,
    };
    if media.keep(&id, &record).await.is_err() {
        return refused(503, "The server couldn't keep that.");
    }
    let put = media.store.put_link(
        &bytes_key(&id),
        &starting.mime,
        asked.size,
        uploads::UPLOAD_SECONDS,
        For::People,
    );
    Json(UploadStarted {
        url: put.url,
        id,
        headers: put.headers,
        expires_in: uploads::UPLOAD_SECONDS,
    })
    .into_response()
}

/// Where the file `id`'s bytes are kept.
fn bytes_key(id: &str) -> String {
    format!("files/{id}")
}

/// Finishes an upload: checks its bytes are what it said and the organisation takes them,
/// reads an image's size, previews text, and answers the file to share.
async fn done(State(media): State<Arc<Media>>, Id(id): Id<String>, headers: HeaderMap) -> Response {
    let who = match media.who(&headers).await {
        Ok(who) => who,
        Err((status, why)) => return refused(status, why),
    };
    let Some(mut record) = media.record(&id).await.filter(|r| r.uploader == who.person) else {
        return refused(404, "No such upload.");
    };
    if record.sending.is_none() {
        return Json(media.file(&id, &record)).into_response();
    }
    let key = bytes_key(&id);
    let Some(sent) = media.store.size(&key).await else {
        return refused(
            409,
            "Nothing was uploaded: send the file, then say it's done.",
        );
    };
    if let Some(Refused { status, why }) = uploads::wrong_size(sent, record.size) {
        media.forget(&id).await;
        return refused(status, &why);
    }
    let Some(start) = media.store.start(&key, START_BYTES).await else {
        return refused(503, "The server couldn't read that.");
    };
    let settings = media.settings().await;
    let arrived = Arrived {
        stored: record.stored,
        name: &record.name,
        mime: &record.mime,
        start: &start,
        video: if record.stored == Stored::Video {
            media.probed(&key).await
        } else {
            None
        },
    };
    let settled = match uploads::settle(&settings, &arrived) {
        Ok(settled) => settled,
        Err(Refused { status, why }) => {
            media.forget(&id).await;
            return refused(status, &why);
        }
    };
    if settled.text && settings.previews {
        let shown = &start[..start.len().min(PREVIEW_BYTES + 1)];
        let text = files::preview_text(&record.name, shown);
        record.preview = Some(Preview {
            html: format!("<pre><code>{}</code></pre>", escape(&text.text)),
            kind: text.kind,
            language: text.language,
            text: text.text,
            truncated: text.truncated,
        });
    }
    record.stored = settled.stored;
    record.mime = settled.mime;
    record.width = settled.width;
    record.height = settled.height;
    record.duration_ms = settled.duration_ms;
    record.sending = None;
    if media.keep(&id, &record).await.is_err() {
        return refused(503, "The server couldn't keep that.");
    }
    Json(media.file(&id, &record)).into_response()
}

/// `text` safe inside HTML.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A text or Markdown file's preview, for anyone signed in: 404 for a file without one.
async fn preview(
    State(media): State<Arc<Media>>,
    Id(id): Id<String>,
    headers: HeaderMap,
) -> Response {
    if let Err((status, why)) = media.who(&headers).await {
        return refused(status, why);
    }
    match media
        .record(&id)
        .await
        .filter(|r| r.sending.is_none())
        .and_then(|r| r.preview)
    {
        Some(preview) => Json(preview).into_response(),
        None => refused(404, "No such preview."),
    }
}

/// Removes a file and everything of it, for its uploader and the organisation's owners and
/// admins: 204, or 404.
async fn remove(
    State(media): State<Arc<Media>>,
    Id(id): Id<String>,
    headers: HeaderMap,
) -> Response {
    let who = match media.who(&headers).await {
        Ok(who) => who,
        Err((status, why)) => return refused(status, why),
    };
    match media.record(&id).await {
        Some(record) if record.uploader == who.person || who.role != Role::Member => {
            media.forget(&id).await;
            tracing::info!(id, by = %who.account, "removed a file");
            StatusCode::NO_CONTENT.into_response()
        }
        _ => refused(404, "No such file."),
    }
}

/// What's asked of a file: an image at a width (`w`), as a format (`f`), or the `original`.
#[derive(Debug, Default, Deserialize)]
struct Asked {
    w: Option<u32>,
    f: Option<String>,
    /// A video's copy to play, about this many lines high.
    r: Option<u32>,
    original: Option<String>,
}

/// A file's bytes, for anyone signed in: an image at the width asked for, else a link to
/// them, an image shown, anything else downloaded, a file whose type isn't one that's shown
/// as `application/octet-stream`.
async fn download(
    State(media): State<Arc<Media>>,
    Id(id): Id<String>,
    Query(asked): Query<Asked>,
    headers: HeaderMap,
) -> Response {
    if let Err((status, why)) = media.who(&headers).await {
        return refused(status, why);
    }
    let Some(record) = media.record(&id).await.filter(|r| r.sending.is_none()) else {
        return refused(404, "No such file.");
    };
    if record.stored == Stored::Image
        && asked.original.is_none()
        && let Some(width) = asked.w
        && let Some(resized) = media
            .resized(&bytes_key(&id), width, asked.f.as_deref())
            .await
    {
        return resized;
    }
    if record.stored == Stored::Video
        && asked.original.is_none()
        && let Some(answer) = media.video(&id, &record, &asked).await
    {
        return answer;
    }
    let served = match record.stored {
        Stored::Image => Served {
            mime: Some(record.mime),
            disposition: Some("inline".to_owned()),
        },
        Stored::Video => Served {
            mime: Some(record.mime),
            disposition: Some(attachment(&record.name)),
        },
        Stored::File | Stored::Avatar => Served {
            mime: Some("application/octet-stream".to_owned()),
            disposition: Some(attachment(&record.name)),
        },
    };
    redirect(
        &media
            .store
            .link(&bytes_key(&id), LINK_SECONDS, For::People, &served),
    )
}

impl Media {
    /// The image at `key`, `width` pixels wide at most (the next of [`uploads::WIDTHS`] up)
    /// as `format` ([`uploads::format_for`]), without its metadata, from imgproxy: `None`
    /// when there's no imgproxy or it couldn't.
    async fn resized(&self, key: &str, width: u32, format: Option<&str>) -> Option<Response> {
        let imgproxy = self.beside.imgproxy.as_deref()?;
        let width = uploads::width_for(width);
        let format = uploads::format_for(format);
        let source = self
            .store
            .link(key, SOURCE_SECONDS, For::Beside, &Served::default());
        // Unsigned: only the server reaches imgproxy, which nothing publishes.
        let url = format!(
            "{imgproxy}/insecure/rs:fit:{width}:0/{}.{format}",
            URL_SAFE_NO_PAD.encode(source)
        );
        let answer = match self.http.get(url).send().await {
            Ok(answer) if answer.status().is_success() => answer,
            Ok(answer) => {
                tracing::warn!(key, status = %answer.status(), "imgproxy couldn't resize");
                return None;
            }
            Err(err) => {
                tracing::warn!(key, "imgproxy didn't answer: {err}");
                return None;
            }
        };
        let bytes = answer.bytes().await.ok()?;
        Some(
            (
                [
                    (header::CONTENT_TYPE, format!("image/{format}")),
                    (header::CACHE_CONTROL, "private, max-age=86400".to_owned()),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
                ],
                bytes,
            )
                .into_response(),
        )
    }
}

/// Where a video's poster is kept, once made.
fn poster_key(id: &str) -> String {
    format!("cache/{id}/poster.jpg")
}

/// Where a video's copy to play, `height` lines high at most, is kept, once made.
fn copy_key(id: &str, height: u32) -> String {
    format!("cache/{id}/{height}.mp4")
}

impl Media {
    /// What the video at `key` is, as the media worker reads it: `None` when there's no
    /// worker, or it isn't a video it can read, which is then kept as a file.
    async fn probed(&self, key: &str) -> Option<uploads::Video> {
        let worker = self.beside.worker.as_deref()?;
        let source = self
            .store
            .link(key, SOURCE_SECONDS, For::Beside, &Served::default());
        let answer = self
            .http
            .post(format!("{worker}/probe"))
            .json(&json!({ "source": source }))
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .ok()?;
        if !answer.status().is_success() {
            return None;
        }
        let probed: zirc_media_worker::Probed = answer.json().await.ok()?;
        Some(uploads::Video {
            width: probed.width,
            height: probed.height,
            duration_ms: probed.duration_ms,
        })
    }

    /// The video `id`'s poster at a width (`w`), or where its copy about `r` lines high plays
    /// from, for half an hour: the media worker makes each the first time it's asked for,
    /// answering 202 meanwhile, as zirc.dev does. `None` for the original.
    async fn video(&self, id: &str, record: &Record, asked: &Asked) -> Option<Response> {
        if let Some(width) = asked.w {
            let poster = poster_key(id);
            if self.store.size(&poster).await.is_none() {
                return Some(self.make(id, record, Make::Poster).await);
            }
            if let Some(resized) = self.resized(&poster, width, asked.f.as_deref()).await {
                return Some(resized);
            }
            let served = Served {
                mime: Some("image/jpeg".to_owned()),
                disposition: Some("inline".to_owned()),
            };
            return Some(redirect(&self.store.link(
                &poster,
                LINK_SECONDS,
                For::People,
                &served,
            )));
        }
        let height = zirc_media_worker::height_for(asked.r?);
        let copy = copy_key(id, height);
        if self.store.size(&copy).await.is_none() {
            return Some(self.make(id, record, Make::Copy { height }).await);
        }
        let served = Served {
            mime: Some("video/mp4".to_owned()),
            disposition: Some("inline".to_owned()),
        };
        Some(redirect(&self.store.link(
            &copy,
            PLAY_SECONDS,
            For::People,
            &served,
        )))
    }

    /// Asks the media worker to make `make` from the video `id`, which it puts where it's
    /// kept, and answers that it's being made: 202, to ask again shortly.
    async fn make(&self, id: &str, record: &Record, make: Make) -> Response {
        let Some(worker) = self.beside.worker.as_deref() else {
            return refused(503, "This server doesn't make videos' copies.");
        };
        let (key, mime, most, name) = match make {
            Make::Poster => (
                poster_key(id),
                "image/jpeg",
                20_000_000,
                "poster".to_owned(),
            ),
            // A copy can be larger than a well-compressed original, but not without bound.
            Make::Copy { height } => (
                copy_key(id, height),
                "video/mp4",
                (record.size * 3).max(50_000_000),
                height.to_string(),
            ),
        };
        let put = self
            .store
            .put_link(&key, mime, most, MAKING_SECONDS, For::Beside);
        let job = Job {
            id: format!("{id}/{name}"),
            source: self.store.link(
                &bytes_key(id),
                MAKING_SECONDS,
                For::Beside,
                &Served::default(),
            ),
            make,
            put: zirc_media_worker::Put {
                url: put.url,
                headers: put.headers,
            },
        };
        let asked = self
            .http
            .post(format!("{worker}/jobs"))
            .json(&job)
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        if let Err(err) = asked.and_then(reqwest::Response::error_for_status) {
            tracing::warn!(id, "the media worker didn't take a job: {err}");
        }
        (
            StatusCode::ACCEPTED,
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::RETRY_AFTER, "5"),
            ],
            Json(json!({ "processing": true })),
        )
            .into_response()
    }
}

/// A `Content-Disposition` that saves a file as `name`, whatever its characters.
fn attachment(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    format!("attachment; filename*=UTF-8''{encoded}")
}

/// Sends the asker to `location`, which lasts minutes: theirs alone to keep for less.
fn redirect(location: &str) -> Response {
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, location.to_owned()),
            (
                header::CACHE_CONTROL,
                format!("private, max-age={}", LINK_SECONDS - 60),
            ),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_says_the_account_in_lowercase_and_its_password() {
        let mut headers = HeaderMap::new();
        let encoded = STANDARD.encode("Alice:p:w");
        headers.insert(
            header::AUTHORIZATION,
            format!("Basic {encoded}").parse().unwrap(),
        );
        assert_eq!(basic(&headers), Some(("alice".into(), "p:w".into())));
        headers.insert(header::AUTHORIZATION, "Basic !!".parse().unwrap());
        assert_eq!(basic(&headers), None);
    }

    #[test]
    fn attachments_keep_their_names() {
        assert_eq!(
            attachment("a b\"é.txt"),
            "attachment; filename*=UTF-8''a%20b%22%C3%A9.txt"
        );
    }

    #[test]
    fn previews_are_escaped() {
        assert_eq!(escape("<b>&\"'"), "&lt;b&gt;&amp;&quot;&#39;");
    }
}
