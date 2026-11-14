//! The admin commands, run inside the server's container (`docker exec`): they ask its
//! account endpoint on loopback, with the token in its data directory. People and their
//! accounts are zirc.dev's to make; these only look.

use std::net::SocketAddr;

use anyhow::bail;
use serde_json::Value;

/// The running server's account endpoint.
pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    /// The endpoint at `accounts`, which takes `token`.
    #[must_use]
    pub fn new(accounts: SocketAddr, token: String) -> Self {
        Self {
            base: format!("http://{accounts}"),
            token,
            http: zirc_sidecar::http(),
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> anyhow::Result<Value> {
        let response = request.bearer_auth(&self.token).send().await.map_err(|e| {
            anyhow::anyhow!("couldn't reach the server's account endpoint: is it running? ({e})")
        })?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let said = body["message"].as_str().or(body["error"].as_str());
            bail!(
                "the server refused ({status}): {}",
                said.unwrap_or("no reason given")
            );
        }
        Ok(body)
    }
}

/// The accounts, one a line: name, and whether it's connected.
///
/// # Errors
///
/// When the server can't be reached or refuses.
pub async fn list(client: &Client) -> anyhow::Result<String> {
    let body = client
        .send(client.http.get(format!("{}/v1/accounts", client.base)))
        .await?;
    Ok(lines(&body))
}

fn lines(body: &Value) -> String {
    body["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|account| {
            let name = account["name"].as_str().unwrap_or("?");
            let on = if account["connected"].as_bool() == Some(true) {
                "connected"
            } else {
                "away"
            };
            format!("{name}\t{on}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn accounts_are_listed_one_a_line() {
        let body = json!({"accounts": [
            {"name": "alice", "connected": true},
            {"name": "bob", "connected": false},
        ]});
        assert_eq!(super::lines(&body), "alice\tconnected\nbob\taway");
    }
}
