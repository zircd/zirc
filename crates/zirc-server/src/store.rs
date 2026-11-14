//! Where a self-hosted server keeps files' bytes: on its own disk, under `<data>/media`, or in
//! an S3 bucket of the organisation's own (any S3-compatible service), set by `ZIRC_S3_*`.
//! What's known about each file stays on disk either way ([`crate::media`]).
//!
//! Everything is reached through short-lived links, as zirc.dev's bucket is: apps upload to
//! one and play videos from another, and imgproxy and the media worker read originals and
//! put what they make through others, so neither needs credentials for anything. In a bucket
//! the links are S3's own presigned ones; on disk they're the server's, at `/s/<key>`,
//! signed with a key of its own (`<data>/media/links.key`) for one method, one object and
//! what it's served as, until a time.
//!
//! Keys are `files/<id>` for a file and `cache/<id>/…` for what's made from it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as Key, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use ring::hmac;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use serde::Deserialize;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tower::ServiceExt as _;

/// The path disk links are served at.
pub const LINKS: &str = "/s";

/// Who a link is for: people's apps and browsers, at the server's address, or imgproxy and
/// the media worker, beside the server, at the address they reach it at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum For {
    /// Apps and browsers.
    People,
    /// imgproxy and the media worker.
    Beside,
}

/// What a link serves an object as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Served {
    /// Its `Content-Type`.
    pub mime: Option<String>,
    /// Its `Content-Disposition`.
    pub disposition: Option<String>,
}

/// Where to put something: a link to `PUT` it to, and the headers to send with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Put {
    /// The link.
    pub url: String,
    /// The headers.
    pub headers: BTreeMap<String, String>,
}

/// An S3 bucket's settings.
#[derive(Debug, Clone, clap::Args)]
pub struct S3 {
    /// The organisation's own S3 bucket to keep files in, rather than the server's disk.
    #[arg(long = "s3-bucket", env = "ZIRC_S3_BUCKET")]
    pub bucket: Option<String>,
    /// Its service's address, such as `https://s3.eu-west-2.amazonaws.com` or a
    /// Cloudflare R2 account's.
    #[arg(long = "s3-endpoint", env = "ZIRC_S3_ENDPOINT")]
    pub endpoint: Option<String>,
    /// Its region, `auto` for services that have none.
    #[arg(long = "s3-region", env = "ZIRC_S3_REGION", default_value = "auto")]
    pub region: String,
    /// The bucket is in the address's path, not its host name: for services that need it.
    #[arg(long = "s3-path-style", env = "ZIRC_S3_PATH_STYLE")]
    pub path_style: bool,
}

enum Backing {
    Disk(PathBuf),
    Bucket {
        bucket: Bucket,
        credentials: Credentials,
    },
}

/// The files' store.
pub struct Store {
    backing: Backing,
    /// What disk links are signed with.
    key: hmac::Key,
    /// The server's address, for people's links: `https://<host>`.
    public: String,
    /// The server's address as imgproxy and the media worker reach it.
    beside: String,
    http: reqwest::Client,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether `key` is one the store keeps: `files/<id>` or `cache/<id>/<name>`, of plain
/// characters, nothing that climbs out of where it is.
#[must_use]
pub fn valid(key: &str) -> bool {
    let plain = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    let parts: Vec<&str> = key.split('/').collect();
    match parts.as_slice() {
        ["files", id] => plain(id),
        ["cache", id, name] => plain(id) && plain(name),
        _ => false,
    }
}

impl Store {
    /// The store for a server whose data directory is `data`, reached by people at `public`
    /// and by imgproxy and the media worker at `beside`: `s3`'s bucket if one is set, with
    /// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, else the disk.
    ///
    /// # Errors
    ///
    /// When the bucket's settings are wrong, or the links' key can't be read or made.
    pub fn new(data: &Path, s3: &S3, public: String, beside: String) -> anyhow::Result<Store> {
        let dir = data.join("media");
        std::fs::create_dir_all(&dir)?;
        let backing = match &s3.bucket {
            None => Backing::Disk(dir.clone()),
            Some(name) => {
                let endpoint = s3.endpoint.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("ZIRC_S3_ENDPOINT says where the bucket's service is")
                })?;
                let style = if s3.path_style {
                    UrlStyle::Path
                } else {
                    UrlStyle::VirtualHost
                };
                let bucket =
                    Bucket::new(endpoint.parse()?, style, name.clone(), s3.region.clone())?;
                let credentials = Credentials::from_env().ok_or_else(|| {
                    anyhow::anyhow!(
                        "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are the bucket's credentials"
                    )
                })?;
                Backing::Bucket {
                    bucket,
                    credentials,
                }
            }
        };
        Ok(Store {
            backing,
            key: hmac::Key::new(hmac::HMAC_SHA256, &links_key(&dir)?),
            public,
            beside,
            http: zirc_sidecar::http(),
        })
    }

    /// Whether files are kept in a bucket.
    #[must_use]
    pub fn in_bucket(&self) -> bool {
        matches!(self.backing, Backing::Bucket { .. })
    }

    fn path(&self, key: &str) -> Option<PathBuf> {
        match &self.backing {
            Backing::Disk(dir) if valid(key) => Some(dir.join(key)),
            _ => None,
        }
    }

    /// A link to `GET` the object at `key`, for `seconds`, served as `served`.
    #[must_use]
    pub fn link(&self, key: &str, seconds: u32, to: For, served: &Served) -> String {
        match &self.backing {
            Backing::Bucket {
                bucket,
                credentials,
            } => {
                let mut action = bucket.get_object(Some(credentials), key);
                if let Some(mime) = &served.mime {
                    action
                        .query_mut()
                        .insert("response-content-type", mime.clone());
                }
                if let Some(disposition) = &served.disposition {
                    action
                        .query_mut()
                        .insert("response-content-disposition", disposition.clone());
                }
                action.sign(Duration::from_secs(seconds.into())).to_string()
            }
            Backing::Disk(_) => {
                let expires = now() + u64::from(seconds);
                let mime = served.mime.clone().unwrap_or_default();
                let disposition = served.disposition.clone().unwrap_or_default();
                let signature = self.sign(&["GET", key, &mime, &disposition, &expires.to_string()]);
                let mut url = format!("{}{LINKS}/{key}?e={expires}&s={signature}", self.base(to));
                for (name, value) in [("t", &mime), ("d", &disposition)] {
                    if !value.is_empty() {
                        let _ = write!(url, "&{name}={}", encode(value));
                    }
                }
                url
            }
        }
    }

    /// A link to `PUT` at most `most` bytes to `key`, for `seconds`.
    #[must_use]
    pub fn put_link(&self, key: &str, mime: &str, most: u64, seconds: u32, to: For) -> Put {
        let headers = BTreeMap::from([("Content-Type".to_owned(), mime.to_owned())]);
        let url = match &self.backing {
            Backing::Bucket {
                bucket,
                credentials,
            } => bucket
                .put_object(Some(credentials), key)
                .sign(Duration::from_secs(seconds.into()))
                .to_string(),
            Backing::Disk(_) => {
                let expires = now() + u64::from(seconds);
                let signature = self.sign(&["PUT", key, &most.to_string(), &expires.to_string()]);
                format!(
                    "{}{LINKS}/{key}?e={expires}&n={most}&s={signature}",
                    self.base(to)
                )
            }
        };
        Put { url, headers }
    }

    fn base(&self, to: For) -> &str {
        match to {
            For::People => &self.public,
            For::Beside => &self.beside,
        }
    }

    fn sign(&self, parts: &[&str]) -> String {
        URL_SAFE_NO_PAD.encode(hmac::sign(&self.key, parts.join("\n").as_bytes()))
    }

    fn signed(&self, parts: &[&str], signature: &str) -> bool {
        URL_SAFE_NO_PAD
            .decode(signature)
            .is_ok_and(|tag| hmac::verify(&self.key, parts.join("\n").as_bytes(), &tag).is_ok())
    }

    /// How large the object at `key` is, if it's there.
    pub async fn size(&self, key: &str) -> Option<u64> {
        match &self.backing {
            Backing::Disk(_) => Some(tokio::fs::metadata(self.path(key)?).await.ok()?.len()),
            Backing::Bucket {
                bucket,
                credentials,
            } => {
                let url = bucket
                    .head_object(Some(credentials), key)
                    .sign(Duration::from_secs(60));
                let answer = self.http.head(url).send().await.ok()?;
                if !answer.status().is_success() {
                    return None;
                }
                answer
                    .headers()
                    .get(header::CONTENT_LENGTH)?
                    .to_str()
                    .ok()?
                    .parse()
                    .ok()
            }
        }
    }

    /// At most the first `bytes` of the object at `key`, if it's there.
    pub async fn start(&self, key: &str, bytes: usize) -> Option<Vec<u8>> {
        match &self.backing {
            Backing::Disk(_) => {
                use tokio::io::AsyncReadExt as _;
                let file = tokio::fs::File::open(self.path(key)?).await.ok()?;
                let mut start = Vec::with_capacity(bytes);
                file.take(bytes as u64).read_to_end(&mut start).await.ok()?;
                Some(start)
            }
            Backing::Bucket {
                bucket,
                credentials,
            } => {
                let url = bucket
                    .get_object(Some(credentials), key)
                    .sign(Duration::from_secs(60));
                let answer = self
                    .http
                    .get(url)
                    .header(
                        header::RANGE,
                        format!("bytes=0-{}", bytes.saturating_sub(1)),
                    )
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?;
                let mut start = answer.bytes().await.ok()?.to_vec();
                start.truncate(bytes);
                Some(start)
            }
        }
    }

    /// Removes the object at `key`, or every one under it when it ends in `/`. What isn't
    /// there is already gone.
    pub async fn remove(&self, key: &str) {
        match &self.backing {
            Backing::Disk(dir) => {
                if let Some(prefix) = key.strip_suffix('/') {
                    if valid(&format!("{prefix}/x")) {
                        let _ = tokio::fs::remove_dir_all(dir.join(prefix)).await;
                    }
                } else if let Some(path) = self.path(key) {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
            Backing::Bucket {
                bucket,
                credentials,
            } => {
                let keys = if key.ends_with('/') {
                    self.listed(bucket, credentials, key).await
                } else {
                    vec![key.to_owned()]
                };
                for key in keys {
                    let url = bucket
                        .delete_object(Some(credentials), &key)
                        .sign(Duration::from_secs(60));
                    if let Err(err) = self.http.delete(url).send().await {
                        tracing::warn!(key, "couldn't remove from the bucket: {err}");
                    }
                }
            }
        }
    }

    /// The keys under `prefix`, as many as one listing gives: what's made from a file is a
    /// few.
    async fn listed(
        &self,
        bucket: &Bucket,
        credentials: &Credentials,
        prefix: &str,
    ) -> Vec<String> {
        let mut action = bucket.list_objects_v2(Some(credentials));
        action.with_prefix(prefix);
        let url = action.sign(Duration::from_secs(60));
        let Ok(answer) = self.http.get(url).send().await else {
            return Vec::new();
        };
        let Ok(text) = answer.text().await else {
            return Vec::new();
        };
        rusty_s3::actions::ListObjectsV2::parse_response(&text)
            .map(|listed| listed.contents.into_iter().map(|c| c.key).collect())
            .unwrap_or_default()
    }
}

/// The key disk links are signed with, made once.
fn links_key(dir: &Path) -> anyhow::Result<Vec<u8>> {
    let path = dir.join("links.key");
    if let Ok(key) = std::fs::read(&path)
        && key.len() == 32
    {
        return Ok(key);
    }
    let mut key = vec![0; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut key)
        .map_err(|_| anyhow::anyhow!("no randomness for the links' key"))?;
    crate::secrets::write_private(&path, &key)?;
    Ok(key)
}

fn encode(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value)
}

fn decode(value: Option<&String>) -> String {
    value
        .and_then(|v| URL_SAFE_NO_PAD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct Signed {
    e: u64,
    s: String,
    #[serde(default)]
    n: Option<u64>,
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    d: Option<String>,
}

/// The routes disk links are served at: nothing when files are in a bucket.
pub fn router(store: Arc<Store>) -> Router {
    if store.in_bucket() {
        return Router::new();
    }
    Router::new()
        .route(&format!("{LINKS}/{{*key}}"), get(serve).put(take))
        .with_state(store)
}

fn refused(status: StatusCode) -> Response {
    status.into_response()
}

/// An object, for a link signed for it that hasn't run out, ranges and all, as the link
/// says to serve it.
async fn serve(
    State(store): State<Arc<Store>>,
    Key(key): Key<String>,
    Query(signed): Query<Signed>,
    request: Request,
) -> Response {
    let mime = decode(signed.t.as_ref());
    let disposition = decode(signed.d.as_ref());
    let parts = [
        "GET",
        key.as_str(),
        &mime,
        &disposition,
        &signed.e.to_string(),
    ];
    if signed.e < now() || !store.signed(&parts, &signed.s) {
        return refused(StatusCode::FORBIDDEN);
    }
    let Some(path) = store.path(&key) else {
        return refused(StatusCode::NOT_FOUND);
    };
    let mut response = tower_http::services::ServeFile::new(path)
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {});
    let headers = response.headers_mut();
    let mime = if mime.is_empty() {
        "application/octet-stream".to_owned()
    } else {
        mime
    };
    if let Ok(value) = mime.parse() {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if !disposition.is_empty()
        && let Ok(value) = disposition.parse()
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=3600"),
    );
    response.map(Body::new)
}

/// An object's bytes, for a link signed for putting it that hasn't run out, at most as many
/// as it says.
async fn take(
    State(store): State<Arc<Store>>,
    Key(key): Key<String>,
    Query(signed): Query<Signed>,
    body: Body,
) -> Response {
    let Some(most) = signed.n else {
        return refused(StatusCode::FORBIDDEN);
    };
    let parts = [
        "PUT",
        key.as_str(),
        &most.to_string(),
        &signed.e.to_string(),
    ];
    if signed.e < now() || !store.signed(&parts, &signed.s) {
        return refused(StatusCode::FORBIDDEN);
    }
    let Some(path) = store.path(&key) else {
        return refused(StatusCode::NOT_FOUND);
    };
    if let Some(dir) = path.parent()
        && tokio::fs::create_dir_all(dir).await.is_err()
    {
        return refused(StatusCode::SERVICE_UNAVAILABLE);
    }
    let part = path.with_extension("part");
    let Ok(mut out) = tokio::fs::File::create(&part).await else {
        return refused(StatusCode::SERVICE_UNAVAILABLE);
    };
    let mut written = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            let _ = tokio::fs::remove_file(&part).await;
            return refused(StatusCode::BAD_REQUEST);
        };
        written += chunk.len() as u64;
        if written > most {
            let _ = tokio::fs::remove_file(&part).await;
            return refused(StatusCode::PAYLOAD_TOO_LARGE);
        }
        if out.write_all(&chunk).await.is_err() {
            let _ = tokio::fs::remove_file(&part).await;
            return refused(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
    if out.flush().await.is_err() || tokio::fs::rename(&part, &path).await.is_err() {
        return refused(StatusCode::SERVICE_UNAVAILABLE);
    }
    StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk() -> (tempfile::TempDir, Store) {
        let data = tempfile::tempdir().unwrap();
        let s3 = S3 {
            bucket: None,
            endpoint: None,
            region: "auto".into(),
            path_style: false,
        };
        let store = Store::new(
            data.path(),
            &s3,
            "https://irc.acme.test".into(),
            "http://zirc:8090".into(),
        )
        .unwrap();
        (data, store)
    }

    #[test]
    fn keys_are_a_file_or_what_s_made_from_one() {
        assert!(valid("files/abc_-1"));
        assert!(valid("cache/abc/poster.jpg"));
        for bad in [
            "files/../x",
            "files/.x",
            "x/abc",
            "files/a/b",
            "cache/a",
            "files/",
            "files/a b",
        ] {
            assert!(!valid(bad), "{bad}");
        }
    }

    #[test]
    fn disk_links_are_for_one_object_and_what_it_s_served_as() {
        let (_data, store) = disk();
        let served = Served {
            mime: Some("image/png".into()),
            disposition: None,
        };
        let link = store.link("files/abc", 60, For::People, &served);
        assert!(
            link.starts_with("https://irc.acme.test/s/files/abc?e="),
            "{link}"
        );
        let beside = store.link("files/abc", 60, For::Beside, &served);
        assert!(
            beside.starts_with("http://zirc:8090/s/files/abc?e="),
            "{beside}"
        );
        let query: String = link.split_once('?').unwrap().1.to_owned();
        let signed: Signed = serde_urlencoded_like(&query);
        let mime = decode(signed.t.as_ref());
        assert_eq!(mime, "image/png");
        let e = signed.e.to_string();
        assert!(store.signed(&["GET", "files/abc", &mime, "", &e], &signed.s));
        assert!(!store.signed(&["GET", "files/abd", &mime, "", &e], &signed.s));
        assert!(!store.signed(&["GET", "files/abc", "text/html", "", &e], &signed.s));
    }

    #[test]
    fn the_links_key_is_kept() {
        let data = tempfile::tempdir().unwrap();
        assert_eq!(
            links_key(data.path()).unwrap(),
            links_key(data.path()).unwrap()
        );
    }

    #[test]
    fn bucket_links_are_presigned() {
        let data = tempfile::tempdir().unwrap();
        let bucket = Bucket::new(
            "https://s3.eu-west-2.amazonaws.com".parse().unwrap(),
            UrlStyle::VirtualHost,
            "acme-files",
            "eu-west-2",
        )
        .unwrap();
        let store = Store {
            backing: Backing::Bucket {
                bucket,
                credentials: Credentials::new("AKIDEXAMPLE", "secret"),
            },
            key: hmac::Key::new(hmac::HMAC_SHA256, &links_key(data.path()).unwrap()),
            public: "https://irc.acme.test".into(),
            beside: "http://zirc:8090".into(),
            http: zirc_sidecar::http(),
        };
        let served = Served {
            mime: None,
            disposition: Some("attachment".into()),
        };
        let link = store.link("files/abc", 60, For::People, &served);
        assert!(
            link.starts_with("https://acme-files.s3.eu-west-2.amazonaws.com/files/abc?"),
            "{link}"
        );
        assert!(
            link.contains("X-Amz-Signature=")
                && link.contains("response-content-disposition=attachment"),
            "{link}"
        );
        let put = store.put_link("files/abc", "image/png", 10, 60, For::People);
        assert!(put.url.contains("X-Amz-Signature="));
        assert_eq!(put.headers["Content-Type"], "image/png");
    }

    fn serde_urlencoded_like(query: &str) -> Signed {
        let pairs: BTreeMap<&str, &str> = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .collect();
        Signed {
            e: pairs["e"].parse().unwrap(),
            s: pairs["s"].to_owned(),
            n: pairs.get("n").map(|n| n.parse().unwrap()),
            t: pairs.get("t").map(|t| (*t).to_owned()),
            d: pairs.get("d").map(|d| (*d).to_owned()),
        }
    }
}
