//! Reaching a tenant's Ergo from beside it in its pod: its HTTP API, and an operator's IRC
//! connection for what the API can't do (suspending an account, removing a message). Both are
//! on the pod's loopback only.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use zirc_irc::{Config, Incoming, Line};

/// METADATA keys on an account, each with its value, or `None` to delete it.
pub type Metadata<'a> = Vec<(&'a str, Option<&'a str>)>;

/// Ergo's HTTP API.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Api {
    /// An API client for Ergo's account endpoint at `base`, with the bearer `token`.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            http: crate::http(),
            base: base.into(),
            token: token.into(),
        }
    }

    /// Calls `/v1/<endpoint>` with `body`. Ergo answers 200 for what it understood, with
    /// `success` saying whether it worked.
    ///
    /// # Errors
    ///
    /// When Ergo doesn't answer, or answers other than 200 with JSON.
    pub async fn call<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: Value,
    ) -> reqwest::Result<T> {
        self.http
            .post(format!("{}/v1/{endpoint}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
}

/// Logging in to Ergo as its operator, on the loopback listener that needs neither a PROXY
/// header nor SASL.
#[derive(Clone)]
pub struct Operator {
    /// Where Ergo listens on loopback.
    pub host: String,
    /// The port Ergo listens on.
    pub port: u16,
    /// The gate password, while the server has one.
    pub server_password: Option<String>,
    /// The operator's password.
    pub password: String,
}

#[derive(Debug, thiserror::Error)]
/// Why an operator's login or command failed.
pub enum OperatorError {
    #[error("couldn't connect to Ergo: {0}")]
    /// Ergo couldn't be reached.
    Connect(#[from] zirc_irc::Error),
    #[error("Ergo refused the operator password")]
    /// Ergo refused the operator password.
    Refused,
    #[error("Ergo didn't answer in time")]
    /// Ergo didn't answer in time.
    Timeout,
    #[error("the connection ended")]
    /// Ergo closed the connection.
    Closed,
    /// Ergo's `FAIL` code, such as `UNKNOWN_MSGID`, and what it said.
    #[error("Ergo refused: {code}: {message}")]
    Failed {
        /// The `FAIL` code.
        code: String,
        /// What Ergo said.
        message: String,
    },
}

impl Operator {
    /// Runs a `NickServ` command as the operator and returns all `NickServ` says to it.
    ///
    /// # Errors
    ///
    /// When it can't connect, isn't let in as an operator, or `NickServ` doesn't answer.
    pub async fn nickserv(&self, command: &str) -> Result<String, OperatorError> {
        let mut answers = self.service("NickServ", &[command.to_owned()]).await?;
        Ok(answers.pop().unwrap_or_default())
    }

    /// Runs `ChanServ` commands as the operator, one after another in one login, and returns
    /// all `ChanServ` says to each, a notice a line.
    ///
    /// # Errors
    ///
    /// When it can't connect, isn't let in as an operator, or `ChanServ` doesn't answer.
    pub async fn chanserv(&self, commands: &[String]) -> Result<Vec<String>, OperatorError> {
        self.service("ChanServ", commands).await
    }

    /// Runs `HostServ` commands as the operator, one after another in one login, and returns
    /// all `HostServ` says to each.
    ///
    /// # Errors
    ///
    /// When it can't connect, isn't let in as an operator, or `HostServ` doesn't answer.
    pub async fn hostserv(&self, commands: &[String]) -> Result<Vec<String>, OperatorError> {
        self.service("HostServ", commands).await
    }

    /// Runs commands of Ergo's `service` as the operator and returns all it says to each, a
    /// notice a line.
    async fn service(
        &self,
        service: &str,
        commands: &[String],
    ) -> Result<Vec<String>, OperatorError> {
        let (client, mut incoming) = self.logged_in().await?;
        let prefix = format!("{service}!");
        let mut answers = Vec::with_capacity(commands.len());
        for command in commands {
            let answer = async {
                client.privmsg(service, command)?;
                // Answered once the service is done with the command, which may take more than
                // one notice: ChanServ gives the code to confirm a purge in its second.
                client.send(Line::new("TIME", [] as [&str; 0]))?;
                let mut said = Vec::new();
                loop {
                    let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                    let from_service = line
                        .source
                        .as_deref()
                        .is_some_and(|s| s.starts_with(&prefix));
                    if from_service && line.command == "NOTICE" {
                        said.push(line.params.last().cloned().unwrap_or_default());
                    } else if line.command == "391" {
                        return Ok(said.join("\n"));
                    }
                }
            };
            match within(answer).await {
                Ok(answer) => answers.push(answer),
                Err(failure) => {
                    let _ = client.quit("done");
                    return Err(failure);
                }
            }
        }
        let _ = client.quit("done");
        Ok(answers)
    }

    /// Sets `modes` on `channel` as the operator (`SAMODE`), whoever holds it.
    ///
    /// # Errors
    ///
    /// When the operator can't log in or the line can't be sent.
    pub async fn modes(&self, channel: &str, modes: &str) -> Result<(), OperatorError> {
        let (client, mut incoming) = self.logged_in().await?;
        let set = async {
            client.send(Line::new("SAMODE", [channel, modes]))?;
            // Answered once what came before it is done.
            client.send(Line::new("TIME", [] as [&str; 0]))?;
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                if line.command == "391" {
                    return Ok(());
                }
            }
        };
        let result = within(set).await;
        let _ = client.quit("done");
        result
    }

    /// Says `text` in `channel` as a notice, joining it for that and leaving after.
    ///
    /// # Errors
    ///
    /// When the operator can't log in or the line can't be sent.
    pub async fn notice(&self, channel: &str, text: &str) -> Result<(), OperatorError> {
        self.notice_from(&self.config(), channel, text).await
    }

    /// [`Self::notice`], from `from`, a nick only one connection holds at a time.
    ///
    /// # Errors
    ///
    /// As [`Self::notice`].
    pub async fn notice_as(
        &self,
        from: &str,
        channel: &str,
        text: &str,
    ) -> Result<(), OperatorError> {
        self.notice_from(&self.config_as(from), channel, text).await
    }

    async fn notice_from(
        &self,
        config: &Config,
        channel: &str,
        text: &str,
    ) -> Result<(), OperatorError> {
        let (client, mut incoming) = self.logged_in_as(config).await?;
        let said = async {
            client.send(Line::new("JOIN", [channel]))?;
            client.send(Line::new("NOTICE", [channel, text]))?;
            // Answered once what came before it is done.
            client.send(Line::new("TIME", [] as [&str; 0]))?;
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                if line.command == "391" {
                    return Ok(());
                }
            }
        };
        let result = within(said).await;
        let _ = client.send(Line::new("PART", [channel]));
        let _ = client.quit("done");
        result
    }

    /// Sends `text` as a private message from `from` to each of `accounts`, which Ergo's web
    /// push turns into a notification on their phones. Callers send one at a time: `from` is
    /// a nick only one connection holds.
    ///
    /// # Errors
    ///
    /// When the operator can't log in or the line can't be sent.
    pub async fn direct(
        &self,
        from: &str,
        accounts: &[String],
        text: &str,
    ) -> Result<(), OperatorError> {
        let (client, mut incoming) = self.logged_in_as(&self.config_as(from)).await?;
        let said = async {
            for account in accounts {
                client.privmsg(account, text)?;
            }
            // Answered once what came before it is done.
            client.send(Line::new("TIME", [] as [&str; 0]))?;
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                if line.command == "391" {
                    return Ok(());
                }
            }
        };
        let result = within(said).await;
        let _ = client.quit("done");
        result
    }

    /// Removes message `msgid` from `channel`'s history with `REDACT`, which also tells
    /// everyone in the channel whose client takes `draft/message-redaction`.
    ///
    /// # Errors
    ///
    /// As [`Operator::nickserv`], and [`OperatorError::Failed`] when Ergo refuses, as with
    /// `UNKNOWN_MSGID` for a message it doesn't have.
    pub async fn redact(&self, channel: &str, msgid: &str) -> Result<(), OperatorError> {
        let (client, mut incoming) = self.logged_in().await?;
        let answer = async {
            client.send(Line::new("REDACT", [channel, msgid]))?;
            // Ergo answers a REDACT only when it fails, and the operator isn't in the channel
            // to see it go by; the TIME behind it is answered once it's done.
            client.send(Line::new("TIME", [] as [&str; 0]))?;
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                match line.command.as_str() {
                    "FAIL" if line.params.first().is_some_and(|c| c == "REDACT") => {
                        return Err(OperatorError::Failed {
                            code: line.params.get(1).cloned().unwrap_or_default(),
                            message: line.params.last().cloned().unwrap_or_default(),
                        });
                    }
                    "391" => return Ok(()),
                    _ => {}
                }
            }
        };
        let result = within(answer).await;
        let _ = client.quit("done");
        result
    }

    /// Has Ergo read its config file again (`REHASH`).
    ///
    /// # Errors
    ///
    /// As [`Operator::nickserv`], and [`OperatorError::Failed`] with what Ergo said when it
    /// keeps the config it had, as for a file it can't read or a change it can't make live.
    pub async fn rehash(&self) -> Result<(), OperatorError> {
        let (client, mut incoming) = self.logged_in().await?;
        let answer = async {
            client.send(Line::new("REHASH", [] as [&str; 0]))?;
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                match line.command.as_str() {
                    "NOTICE" if line.params.last().is_some_and(|t| t == "Rehash complete") => {
                        return Ok(());
                    }
                    "400" if line.params.get(1).is_some_and(|c| c == "REHASH") => {
                        return Err(OperatorError::Failed {
                            code: "REHASH".into(),
                            message: line.params.last().cloned().unwrap_or_default(),
                        });
                    }
                    _ => {}
                }
            }
        };
        let result = within(answer).await;
        let _ = client.quit("done");
        result
    }

    /// Erases `account`, every record of it, with `NickServ`'s `ERASE` and the code it asks for.
    ///
    /// # Errors
    ///
    /// As [`Operator::nickserv`], and [`OperatorError::Failed`] when `NickServ` gives no code
    /// or doesn't confirm.
    pub async fn erase(&self, account: &str) -> Result<(), OperatorError> {
        let (client, mut incoming) = self.logged_in().await?;
        let notices = async |incoming: &mut zirc_irc::Incoming| -> Result<String, OperatorError> {
            // NickServ answers in several notices; the TIME behind the command ends them.
            client.send(Line::new("TIME", [] as [&str; 0]))?;
            let mut said = String::new();
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                match line.command.as_str() {
                    "NOTICE"
                        if line
                            .source
                            .as_deref()
                            .is_some_and(|s| s.starts_with("NickServ!")) =>
                    {
                        said.push_str(line.params.last().map_or("", String::as_str));
                        said.push('\n');
                    }
                    "391" => return Ok(said),
                    _ => {}
                }
            }
        };
        let done = async {
            client.privmsg("NickServ", &format!("ERASE {account}"))?;
            let asked = notices(&mut incoming).await?;
            let code = asked
                .split_whitespace()
                .skip_while(|w| !w.eq_ignore_ascii_case(account))
                .nth(1)
                .map(str::to_owned)
                .ok_or_else(|| OperatorError::Failed {
                    code: "ERASE".into(),
                    message: asked.trim().to_owned(),
                })?;
            client.privmsg("NickServ", &format!("ERASE {account} {code}"))?;
            let answer = notices(&mut incoming).await?;
            // "Successfully unregistered account …", as Ergo 2.19.1 says it.
            if answer.to_lowercase().contains("successfully") {
                Ok(())
            } else {
                Err(OperatorError::Failed {
                    code: "ERASE".into(),
                    message: answer.trim().to_owned(),
                })
            }
        };
        let result = within(done).await;
        let _ = client.quit("done");
        result
    }

    /// Deletes messages from the history, each `(target, msgid)`, with `HistServ`'s `DELETE`,
    /// which takes them from wherever Ergo serves them. Returns how many it deleted.
    ///
    /// # Errors
    ///
    /// As [`Operator::nickserv`].
    pub async fn delete_messages(
        &self,
        messages: &[(String, String)],
    ) -> Result<usize, OperatorError> {
        if messages.is_empty() {
            return Ok(0);
        }
        let (client, mut incoming) = self.logged_in().await?;
        let deleted = async {
            for (target, msgid) in messages {
                client.privmsg("HistServ", &format!("DELETE {target} {msgid}"))?;
            }
            // Answered once what came before it is done.
            client.send(Line::new("TIME", [] as [&str; 0]))?;
            let mut deleted = 0;
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                match line.command.as_str() {
                    "NOTICE"
                        if line
                            .source
                            .as_deref()
                            .is_some_and(|s| s.starts_with("HistServ!"))
                            && line
                                .params
                                .last()
                                .is_some_and(|t| t.starts_with("Successfully")) =>
                    {
                        deleted += 1;
                    }
                    "391" => return Ok(deleted),
                    _ => {}
                }
            }
        };
        // A long list takes a while: ten seconds for every thousand.
        let allowed =
            Duration::from_secs(10) * u32::try_from(messages.len() / 1000 + 1).unwrap_or(u32::MAX);
        let result = tokio::time::timeout(allowed, deleted)
            .await
            .unwrap_or(Err(OperatorError::Timeout));
        let _ = client.quit("done");
        result
    }

    /// Sets METADATA on accounts, as the operator (who may edit anyone's): each key to its
    /// value, or deletes it for `None`. Returns the accounts it was set on: not one with no
    /// client on now to set it on, as a machine's hasn't while it's away.
    ///
    /// # Errors
    ///
    /// As [`Operator::nickserv`], and [`OperatorError::Failed`] when Ergo refuses a value.
    pub async fn set_metadata(
        &self,
        accounts: &[(String, Metadata<'_>)],
    ) -> Result<std::collections::BTreeSet<String>, OperatorError> {
        if accounts.is_empty() {
            return Ok(std::collections::BTreeSet::new());
        }
        let (client, mut incoming) = self.logged_in().await?;
        let set = async {
            for (account, keys) in accounts {
                for (key, value) in keys {
                    let mut params = vec![account.as_str(), "SET", *key];
                    params.extend(*value);
                    client.send(Line::new("METADATA", params))?;
                }
            }
            // Answered once what came before it is done.
            client.send(Line::new("TIME", [] as [&str; 0]))?;
            let mut set: std::collections::BTreeSet<String> =
                accounts.iter().map(|(a, _)| a.clone()).collect();
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                match line.command.as_str() {
                    "FAIL" if line.params.first().is_some_and(|c| c == "METADATA") => {
                        let code = line.params.get(1).cloned().unwrap_or_default();
                        if code == "INVALID_TARGET" {
                            if let Some(target) = line.params.get(2) {
                                set.remove(&target.to_lowercase());
                            }
                        } else {
                            return Err(OperatorError::Failed {
                                code,
                                message: line.params.last().cloned().unwrap_or_default(),
                            });
                        }
                    }
                    "391" => return Ok(set),
                    _ => {}
                }
            }
        };
        let result = within(set).await;
        let _ = client.quit("done");
        result
    }

    /// The server's VAPID public key, with which it signs the pushes it sends: what it tells
    /// every client in `ISUPPORT`, as `VAPID=<key>`. `None` while push is off. Needs no
    /// operator: any connection is told.
    ///
    /// # Errors
    ///
    /// When it can't connect, or Ergo doesn't finish greeting in time.
    pub async fn vapid_key(&self) -> Result<Option<String>, OperatorError> {
        let (client, mut incoming) = zirc_irc::connect(&self.config()).await?;
        let key = within(async {
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                match line.command.as_str() {
                    "005" => {
                        let key = line.params.iter().find_map(|p| p.strip_prefix("VAPID="));
                        if let Some(key) = key {
                            return Ok(Some(key.to_owned()));
                        }
                    }
                    // The end of the MOTD, or that there's none: the greeting is over.
                    "376" | "422" => return Ok(None),
                    _ => {}
                }
            }
        })
        .await;
        let _ = client.quit("done");
        key
    }

    fn config(&self) -> Config {
        let suffix: u32 = rand::random::<u32>() % 100_000;
        self.config_as(&format!("zirc-operator-{suffix}"))
    }

    fn config_as(&self, nick: &str) -> Config {
        let mut config = Config::new(self.host.clone(), self.port, nick.to_owned());
        config.tls = false;
        config.server_password.clone_from(&self.server_password);
        config
    }

    async fn logged_in(&self) -> Result<(zirc_irc::Client, Incoming), OperatorError> {
        self.logged_in_as(&self.config()).await
    }

    async fn logged_in_as(
        &self,
        config: &Config,
    ) -> Result<(zirc_irc::Client, Incoming), OperatorError> {
        let (client, mut incoming) = zirc_irc::connect(config).await?;
        let oper = async {
            client.send(Line::new("OPER", ["admin", self.password.as_str()]))?;
            loop {
                let line = incoming.recv().await.ok_or(OperatorError::Closed)?;
                match line.command.as_str() {
                    "381" => return Ok(()),
                    "464" | "491" => return Err(OperatorError::Refused),
                    _ => {}
                }
            }
        };
        match within(oper).await {
            Ok(()) => Ok((client, incoming)),
            Err(failure) => {
                let _ = client.quit("done");
                Err(failure)
            }
        }
    }
}

/// `work`, or [`OperatorError::Timeout`] if Ergo takes more than ten seconds.
async fn within<T>(
    work: impl Future<Output = Result<T, OperatorError>>,
) -> Result<T, OperatorError> {
    tokio::time::timeout(Duration::from_secs(10), work)
        .await
        .unwrap_or(Err(OperatorError::Timeout))
}
