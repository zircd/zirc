//! A team's own hints as its server keeps them ([`zirc_proto::hints::Listing`]), read and
//! changed through the server's content API with this machine's own key, and kept on this
//! machine, one file per organisation ([`path`]), for `zirc watch`, `zirc hints` and `zirc
//! playbook` to read. zircd reads them when it starts and again whenever zirc's own account
//! says they changed ([`zirc_proto::hints::Changed`]); `zirc hints add` and `remove` change
//! them for the person or the agent session asking.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use zirc_proto::hints::{Change, Hint, Listing, ORG};

use crate::device_key::{self, DeviceKey};
use crate::settings::Organisation;

/// Where this machine keeps `org`'s hints, as last read from its server.
#[must_use]
pub fn path(home: &Path, org: &str) -> PathBuf {
    home.join("team-hints").join(format!("{org}.json"))
}

/// `org`'s hints, as this machine last read them; none before it has.
#[must_use]
pub fn kept(home: &Path, org: &str) -> Listing {
    std::fs::read(path(home, org))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Keeps `listing` as `org`'s hints, unless what's kept is newer.
///
/// # Errors
///
/// When it can't be written.
pub fn keep(home: &Path, org: &str, listing: &Listing) -> std::io::Result<()> {
    if kept(home, org).version > listing.version {
        return Ok(());
    }
    let path = path(home, org);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, serde_json::to_vec(listing)?)?;
    std::fs::rename(&partial, &path)
}

/// The hints for the repository whose channel is `channel`, and the organisation's, among
/// those kept on this machine: from `org`'s alone when it's known, else from every
/// organisation's, as a machine signed in to one keeps its repositories without saying whose.
#[must_use]
pub fn for_repo(home: &Path, org: Option<&str>, channel: &str) -> (Vec<Hint>, Vec<Hint>) {
    let listings: Vec<Listing> = match org {
        Some(org) => vec![kept(home, org)],
        None => std::fs::read_dir(home.join("team-hints"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let org = name.strip_suffix(".json")?.to_owned();
                Some(kept(home, &org))
            })
            .collect(),
    };
    let repo = listings
        .iter()
        .flat_map(|listing| listing.for_scope(channel))
        .collect();
    let organisation = listings
        .iter()
        .flat_map(|listing| listing.for_scope(ORG))
        .collect();
    (repo, organisation)
}

/// Where a server's content API is: `https://<host>/api/v1`, or as `ZIRC_CONTENT_URL` says
/// with `{host}` for the host, as zirc.dev's own setting of that name does.
#[must_use]
pub fn base(host: &str) -> String {
    match std::env::var("ZIRC_CONTENT_URL") {
        Ok(given) if !given.is_empty() => given.replace("{host}", host),
        _ => format!("https://{host}/api/v1"),
    }
}

/// An organisation's server, asked as this machine.
pub struct Server {
    base: String,
    host: String,
    account: String,
    key: Arc<dyn DeviceKey>,
    http: reqwest::Client,
}

impl Server {
    /// `org`'s server, asked with the key in `home`, when the server has this machine's key.
    #[must_use]
    pub fn of(home: &Path, org: &Organisation) -> Option<Self> {
        let account = org.account.as_ref().filter(|account| account.device_key)?;
        let key = Arc::new(device_key::Software::in_file(crate::paths::device_key(
            home,
        )));
        Self::with_key(
            &base(&org.server.host),
            &org.server.host,
            &account.name,
            key,
        )
    }

    /// The server at `base`, whose host is `host`, asked as `account` with `key`.
    #[must_use]
    pub fn with_key(
        base: &str,
        host: &str,
        account: &str,
        key: Arc<dyn DeviceKey>,
    ) -> Option<Self> {
        // reqwest's rustls comes without a crypto provider: use ring, as zirc-irc does.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(zirc_proto::version::user_agent())
            .timeout(Duration::from_secs(20))
            .build()
            .ok()?;
        Some(Self {
            base: base.trim_end_matches('/').to_owned(),
            host: host.to_owned(),
            account: account.to_owned(),
            key,
            http,
        })
    }

    /// A fresh token: the server takes each once.
    fn token(&self) -> Result<String, String> {
        device_key::token(&*self.key, &self.account, &self.host)
            .ok_or_else(|| "this machine's key makes no token".to_owned())
    }

    /// The team's hints.
    ///
    /// # Errors
    ///
    /// When the server can't be reached or refuses, saying why.
    pub async fn list(&self) -> Result<Listing, String> {
        let response = self
            .http
            .get(format!("{}/hints", self.base))
            .bearer_auth(self.token()?)
            .send()
            .await
            .map_err(|err| format!("couldn't reach the server: {err}"))?;
        answer(response, "hints").await
    }

    /// How the organisation's roles stand ([`zirc_proto::roles::Standing`]).
    ///
    /// # Errors
    ///
    /// When the server can't be reached or refuses, saying why.
    pub async fn roles(&self) -> Result<zirc_proto::roles::Standing, String> {
        let response = self
            .http
            .get(format!("{}{}", self.base, zirc_proto::roles::PATH))
            .bearer_auth(self.token()?)
            .send()
            .await
            .map_err(|err| format!("couldn't reach the server: {err}"))?;
        answer(response, "roles").await
    }

    /// Adds `hint` for `scope`, for `session` when an agent's asking.
    ///
    /// # Errors
    ///
    /// When the server can't be reached or refuses, saying why.
    pub async fn add(
        &self,
        scope: &str,
        hint: &Hint,
        session: Option<&str>,
    ) -> Result<Change, String> {
        let response = self
            .http
            .post(format!("{}/hints", self.base))
            .bearer_auth(self.token()?)
            .json(&json!({"scope": scope, "hint": hint, "session": session}))
            .send()
            .await
            .map_err(|err| format!("couldn't reach the server: {err}"))?;
        answer(response, "hints").await
    }

    /// Removes the hint `id`, for `session` when an agent's asking.
    ///
    /// # Errors
    ///
    /// When the server can't be reached or refuses, saying why.
    pub async fn remove(&self, id: &str, session: Option<&str>) -> Result<Change, String> {
        // A session id is letters, digits, dashes, underscores and dots, which need no
        // escaping, and the server takes no other.
        let session = session
            .filter(|session| zirc_proto::hints::is_session(session))
            .map(|session| format!("?session={session}"))
            .unwrap_or_default();
        let id: String = id.chars().filter(char::is_ascii_hexdigit).collect();
        let response = self
            .http
            .delete(format!("{}/hints/{id}{session}", self.base))
            .bearer_auth(self.token()?)
            .send()
            .await
            .map_err(|err| format!("couldn't reach the server: {err}"))?;
        answer(response, "hints").await
    }
}

/// What a response says: its JSON, or the server's own words for why not; `what` is what
/// was asked for, as a server too old to keep it is told.
async fn answer<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<T, String> {
    let status = response.status();
    if status.is_success() {
        // Read whole, then decoded, so what didn't read is named (serde's "missing field
        // `title`"), where reqwest's json() says only that the body didn't decode.
        let body = response
            .bytes()
            .await
            .map_err(|err| format!("couldn't read the server's answer: {err}"))?;
        return serde_json::from_slice(&body)
            .map_err(|err| format!("the server's answer about {what} didn't read: {err}"));
    }
    let said = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| body.get("message")?.as_str().map(str::to_owned));
    Err(match said {
        Some(said) => said,
        None if status == reqwest::StatusCode::NOT_FOUND => {
            format!("this organisation's server doesn't keep {what} yet")
        }
        None => format!("the server answered {status}"),
    })
}

#[cfg(test)]
mod tests {
    use zirc_proto::hints::{Level, Stored, When};

    use super::*;

    fn stored(scope: &str, text: &str) -> Stored {
        let hint = Hint {
            kind: "overlap".into(),
            when: When::default(),
            level: Level::Tell,
            text: text.into(),
            required: false,
        };
        Stored {
            id: hint.id(),
            scope: scope.into(),
            hint,
            by: "ada".into(),
            session: None,
            at: "2026-11-16T10:00:00Z".into(),
        }
    }

    #[test]
    fn keeps_the_newest_and_finds_a_repositorys_and_the_organisations() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let newer = Listing {
            version: 2,
            hints: vec![stored("#api", "ask first"), stored(ORG, "be brief")],
        };
        keep(home, "zirc", &newer).unwrap();
        // An older answer, as from a read that crossed a change, doesn't undo it.
        keep(home, "zirc", &Listing::default()).unwrap();
        assert_eq!(kept(home, "zirc"), newer);
        let texts = |hints: Vec<Hint>| hints.into_iter().map(|h| h.text).collect::<Vec<_>>();
        let (repo, org) = for_repo(home, Some("zirc"), "#api");
        assert_eq!(
            (texts(repo), texts(org)),
            (vec!["ask first".to_owned()], vec!["be brief".to_owned()])
        );
        // Without saying whose the repository is, every organisation's are read.
        let (repo, _) = for_repo(home, None, "#API");
        assert_eq!(texts(repo), ["ask first"]);
        let (repo, org) = for_repo(home, Some("other"), "#api");
        assert!(repo.is_empty() && org.is_empty());
    }

    #[tokio::test]
    async fn asks_the_server_with_a_fresh_token_of_this_machines_and_says_its_refusals() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let answers = [
            (
                "200 OK",
                serde_json::to_string(&Listing {
                    version: 4,
                    hints: vec![stored("#api", "ask first")],
                })
                .unwrap(),
            ),
            (
                "403 Forbidden",
                r#"{"error":"role","message":"only the organisation's people and their machines ask here"}"#.to_owned(),
            ),
        ];
        let server = tokio::spawn(async move {
            let mut asked = Vec::new();
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![0u8; 8192];
                let read = stream.read(&mut request).await.unwrap();
                asked.push(String::from_utf8_lossy(&request[..read]).into_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            asked
        });
        let dir = tempfile::tempdir().unwrap();
        let key = Arc::new(device_key::Software::in_file(dir.path().join("device-key")));
        let client = Server::with_key(
            &format!("http://127.0.0.1:{port}/api/v1/"),
            "zirc.irc.zirc.dev",
            "ada-mbp",
            key,
        )
        .unwrap();
        let listing = client.list().await.unwrap();
        assert_eq!((listing.version, listing.hints.len()), (4, 1));
        let refused = client
            .remove("0123456789abcdef", Some("cc-1"))
            .await
            .unwrap_err();
        assert_eq!(
            refused,
            "only the organisation's people and their machines ask here"
        );
        let asked = server.await.unwrap();
        assert!(
            asked[0].starts_with("GET /api/v1/hints HTTP/1.1"),
            "{}",
            asked[0]
        );
        assert!(
            asked[1].starts_with("DELETE /api/v1/hints/0123456789abcdef?session=cc-1 HTTP/1.1"),
            "{}",
            asked[1]
        );
        let bearer = |request: &str| -> String {
            request
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("authorization: bearer ")
                        .map(str::to_owned)
                })
                .unwrap()
        };
        // A token each time, as the server takes each once.
        assert_ne!(bearer(&asked[0]), bearer(&asked[1]));
    }

    #[test]
    fn a_servers_content_api_is_at_its_host() {
        assert_eq!(
            base("zirc.irc.zirc.dev"),
            "https://zirc.irc.zirc.dev/api/v1"
        );
    }
}
