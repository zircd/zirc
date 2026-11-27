//! Signing in with keys made on each device (`zirc_proto::device_key`), checked here, beside
//! Ergo, with nothing asked of zirc.dev: so apps and agents keep connecting while it's down.
//!
//! - zirc.dev registers a device's public key for its account (`PUT
//!   /v1/accounts/<name>/keys/<key>`, `crate::accounts`), and the sidecar keeps it on the
//!   server's own disk, beside Ergo's history ([`Keys`]). The person is told of each new one.
//! - Ergo hands each SASL login to the sidecar over a Unix socket, its `auth-script`
//!   ([`serve`]): one JSON line in, one out. A device's token comes as `OAUTHBEARER`; the
//!   sidecar checks its signature against the account's keys, that it's for this server and
//!   in date, and that its `jti` wasn't used before, and answers with the account. A
//!   passphrase it leaves to Ergo, which checks it itself when the sidecar says no. If the
//!   sidecar doesn't answer, Ergo falls back to its own passphrases: only device keys wait.
//! - Every attempt goes in the server's login log ([`Log`]), on its own disk, never to Cloud
//!   Logging: when, which account, how (which device key, or a passphrase), from where, and
//!   whether it was let in. zirc.dev can neither write nor read it; the server's own pages
//!   show it to owners. Kept 90 days.
//!
//! Ergo's own log says only "logged into account", without how or from where, and nothing at
//! all of a failed login, so the sidecar keeps this one (Ergo 2.19.1, as tried in the spike).

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, warn};
use zirc_proto::device_key;

/// The most keys one account may have: a person's phones, tablets and computers, with room
/// for a few left behind.
pub const MAX_KEYS: usize = 20;

/// The longest a key's label may be, in characters.
pub const LABEL_CHARS: usize = 64;

/// How long the login log keeps an attempt, in seconds.
pub const KEPT: i64 = 90 * 24 * 60 * 60;

/// The most a log holds on the disk Ergo's history shares, in bytes: past it, its older half
/// goes. Anyone may try to sign in, so the login log otherwise grows with every try.
pub const MOST_BYTES: u64 = 16 * 1024 * 1024;

/// The longest account name an attempt keeps, in characters: a token names any it likes.
pub const ACCOUNT_CHARS: usize = 64;

/// How many logins wait to be heard ([`Signin::logins`]): past it the oldest go, and the
/// next look for machines come on (`crate::profiles::EVERY`) catches them.
const LOGINS_HELD: usize = 256;

/// How a pass the server makes itself begins ([`Signin::pass`]), never a device's token.
pub const PASS_PREFIX: &str = "zirc-pass.";

/// How long such a pass is good for, in seconds: the moment it takes to sign in.
pub const PASS_SECONDS: u64 = 30;

/// Where Ergo reaches the sidecar, beside its history on the disk they share.
#[must_use]
pub fn socket_beside(history: &Path) -> PathBuf {
    history.with_file_name("zirc-auth.sock")
}

/// One device's key.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    /// What the person calls the device, such as "Ada's iPhone".
    pub label: String,
    /// When zirc.dev registered it, in seconds since the epoch.
    pub added_at: i64,
    /// When it last signed in, if it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
}

/// Every account's device keys, by account and then by key (`device_key::key_text`), kept
/// beside Ergo's history.
#[derive(Debug)]
pub struct Keys {
    path: PathBuf,
    accounts: BTreeMap<String, BTreeMap<String, Key>>,
}

/// Why a key can't be added.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a P-256 public key.
    NotAKey,
    /// An empty, overlong or unprintable label.
    Label,
    /// The account has [`MAX_KEYS`] already.
    TooMany,
}

impl Keys {
    /// What was kept beside Ergo's history, if anything.
    #[must_use]
    pub fn beside(history: &Path) -> Self {
        let path = history.with_file_name("zirc-device-keys.json");
        let accounts = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, accounts }
    }

    fn save(&self) {
        if let Err(error) = crate::save_json(&self.path, &self.accounts) {
            warn!(%error, "couldn't save the device keys");
        }
    }

    /// Adds `key` to `account` as `label`: true when it's new, false when it was there
    /// already (its label then set to this one).
    ///
    /// # Errors
    ///
    /// Why it can't be.
    pub fn add(
        &mut self,
        account: &str,
        key: &str,
        label: &str,
        now: i64,
    ) -> Result<bool, Refused> {
        if device_key::point(key).is_none() {
            return Err(Refused::NotAKey);
        }
        let label = label.trim();
        if label.is_empty()
            || label.chars().count() > LABEL_CHARS
            || label.chars().any(char::is_control)
        {
            return Err(Refused::Label);
        }
        let keys = self.accounts.entry(account.to_owned()).or_default();
        let new = !keys.contains_key(key);
        if new && keys.len() >= MAX_KEYS {
            return Err(Refused::TooMany);
        }
        keys.entry(key.to_owned())
            .and_modify(|known| label.clone_into(&mut known.label))
            .or_insert_with(|| Key {
                label: label.to_owned(),
                added_at: now,
                last_used: None,
            });
        self.save();
        Ok(new)
    }

    /// Removes `key` from `account`: whether it was there.
    pub fn remove(&mut self, account: &str, key: &str) -> bool {
        let removed = self
            .accounts
            .get_mut(account)
            .and_then(|keys| keys.remove(key))
            .is_some();
        self.accounts.retain(|_, keys| !keys.is_empty());
        if removed {
            self.save();
        }
        removed
    }

    /// Forgets every key of `account`, as when it's erased.
    pub fn forget(&mut self, account: &str) {
        if self.accounts.remove(account).is_some() {
            self.save();
        }
    }

    /// `account`'s keys.
    #[must_use]
    pub fn of(&self, account: &str) -> BTreeMap<String, Key> {
        self.accounts.get(account).cloned().unwrap_or_default()
    }

    fn get(&self, account: &str, key: &str) -> Option<&Key> {
        self.accounts.get(account)?.get(key)
    }

    fn used(&mut self, account: &str, key: &str, now: i64) {
        if let Some(known) = self.accounts.get_mut(account).and_then(|k| k.get_mut(key)) {
            known.last_used = Some(now);
            self.save();
        }
    }
}

/// One attempt to sign in, as the login log has it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    /// When, in seconds since the epoch.
    pub at: i64,
    /// The account it was for, if it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// How: `device key`, `passphrase` or `certificate`.
    pub method: String,
    /// The device key's label, for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// The token's `jti`, for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Where from, as Ergo saw it (the client's own address, behind the edge's PROXY header).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// `accepted`, `refused`, or `ergo` for a passphrase, which Ergo checks.
    pub result: String,
    /// Why it was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Something a log on the server's own disk keeps, which says when it was.
pub trait Dated: Serialize + serde::de::DeserializeOwned {
    /// When, in seconds since the epoch.
    fn at(&self) -> i64;
}

impl Dated for Attempt {
    fn at(&self) -> i64 {
        self.at
    }
}

/// A log on the server's own disk, never in Cloud Logging: one JSON line an entry, kept
/// [`KEPT`]. The login log is one ([`Attempt`]); the content API's access log another
/// (`crate::accounts`).
#[derive(Debug)]
pub struct Log<T: Dated = Attempt> {
    path: PathBuf,
    /// [`MOST_BYTES`], less in a test.
    most: u64,
    kept: std::marker::PhantomData<T>,
}

impl Log<Attempt> {
    /// The login log beside Ergo's history, without what's older than [`KEPT`] at `now`.
    #[must_use]
    pub fn beside(history: &Path, now: i64) -> Self {
        Self::at(history.with_file_name("zirc-logins.jsonl"), now)
    }
}

impl<T: Dated> Log<T> {
    /// The log at `path`, without what's older than [`KEPT`] at `now`.
    #[must_use]
    pub fn at(path: PathBuf, now: i64) -> Self {
        let log = Self {
            path,
            most: MOST_BYTES,
            kept: std::marker::PhantomData,
        };
        log.trim(now);
        log
    }

    /// Adds `entry`.
    pub fn add(&self, entry: &T) {
        let written = serde_json::to_string(entry)
            .map_err(std::io::Error::other)
            .and_then(|line| {
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?;
                writeln!(file, "{line}")?;
                file.metadata()
            });
        match written {
            Ok(written) if written.len() > self.most => self.halve(),
            Ok(_) => {}
            Err(error) => warn!(%error, path = %self.path.display(), "couldn't write a log"),
        }
    }

    /// Drops the older half of what's kept, by bytes: a log never outgrows [`MOST_BYTES`].
    fn halve(&self) {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return;
        };
        // From the first line that starts past the middle: a byte after a newline, so never
        // inside a character.
        let middle = text.len() / 2;
        let newer = text.as_bytes()[middle..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or("", |end| &text[middle + end + 1..]);
        self.replace(newer);
    }

    /// The entries kept, oldest first.
    #[must_use]
    pub fn read(&self) -> Vec<T> {
        std::fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Drops what's older than [`KEPT`] at `now`.
    pub fn trim(&self, now: i64) {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return;
        };
        let kept: Vec<&str> = text
            .lines()
            .filter(|line| serde_json::from_str::<T>(line).is_ok_and(|e| e.at() > now - KEPT))
            .collect();
        if kept.len() == text.lines().count() {
            return;
        }
        let mut body = kept.join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        self.replace(&body);
    }

    /// Puts `body` in the log's place, whole or not at all.
    fn replace(&self, body: &str) {
        let partial = self.path.with_extension("jsonl.partial");
        if let Err(error) =
            std::fs::write(&partial, body).and_then(|()| std::fs::rename(&partial, &self.path))
        {
            warn!(%error, path = %self.path.display(), "couldn't trim a log");
        }
    }
}

/// What Ergo's auth-script hands over (Ergo's `AuthScriptInput`).
#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    /// The account a passphrase is for (SASL PLAIN).
    #[serde(default)]
    pub account_name: Option<String>,
    /// Never kept or logged; only whether there was one.
    #[serde(default)]
    pub passphrase: Option<String>,
    /// A client certificate's fingerprint, which zirc doesn't use.
    #[serde(default)]
    pub certfp: Option<String>,
    /// The client's address.
    #[serde(default)]
    pub ip: Option<String>,
    /// An `OAUTHBEARER` login: `{"token"}`, a device's.
    #[serde(default)]
    pub oauth2: Option<Value>,
}

/// What Ergo is answered (its `AuthScriptOutput`).
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    /// Whether to let it in.
    pub success: bool,
    /// The account to log it in to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
}

/// Signing in on this server: its keys, the tokens already used, and the log.
#[derive(Debug)]
pub struct Signin {
    /// The server's host name, which tokens must be for; without it, none is.
    pub server_name: Option<String>,
    /// Every account's device keys.
    pub keys: tokio::sync::Mutex<Keys>,
    /// The tokens used, by key and `jti`, until they expire.
    used: std::sync::Mutex<HashMap<String, u64>>,
    /// The passes the server made itself to sign in as a person once ([`Self::pass`]): the
    /// account each is for, and when it ends.
    passes: std::sync::Mutex<HashMap<String, (String, u64)>>,
    /// The login log.
    pub log: Log,
    /// Each account logging in, as Ergo asks: let in by a device key, or by a passphrase
    /// Ergo checks itself, so perhaps not in after all ([`Self::logins`]).
    logins: tokio::sync::broadcast::Sender<String>,
}

impl Signin {
    /// Signing in with what's kept beside Ergo's history.
    #[must_use]
    pub fn beside(history: &Path, server_name: Option<String>, now: i64) -> Self {
        Self {
            server_name: server_name.filter(|n| !n.is_empty()),
            keys: tokio::sync::Mutex::new(Keys::beside(history)),
            used: std::sync::Mutex::default(),
            passes: std::sync::Mutex::default(),
            log: Log::beside(history, now),
            logins: tokio::sync::broadcast::channel(LOGINS_HELD).0,
        }
    }

    /// Each account's name, lower-case, as it logs in from now on: what sets a machine's
    /// name and picture once it's on (`crate::accounts`), as no other word of a login reaches
    /// the sidecar. A passphrase's account may yet be refused, by Ergo.
    #[must_use]
    pub fn logins(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.logins.subscribe()
    }

    /// A pass for `account`, good once within [`PASS_SECONDS`] of `now`: how the server signs in
    /// to its own Ergo as a person whose token the content API checked, to send what they wrote
    /// on a page (`crate::accounts::content`), with no credential of theirs kept or passed on.
    /// It comes to Ergo as `OAUTHBEARER`, as a device's token does, and goes in the login log.
    ///
    /// # Panics
    ///
    /// When the system has no random numbers to give, as nothing safe can be done then.
    #[must_use]
    pub fn pass(&self, account: &str, now: u64) -> String {
        let mut bytes = [0u8; 24];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
            .expect("the system's random numbers");
        let pass = format!("{PASS_PREFIX}{}", zirc_proto::hex(&bytes));
        let mut passes = self
            .passes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        passes.retain(|_, (_, ends)| *ends > now);
        passes.insert(pass.clone(), (account.to_lowercase(), now + PASS_SECONDS));
        pass
    }

    /// The account a pass of [`Self::pass`] is for, once, while it's good.
    fn passed(&self, pass: &str, now: u64) -> Option<String> {
        self.passes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(pass)
            .filter(|(_, ends)| *ends > now)
            .map(|(account, _)| account)
    }

    /// Answers one of Ergo's questions at `now` (seconds since the epoch), and logs it.
    pub async fn answer(&self, input: &Input, now: u64) -> Output {
        let at = i64::try_from(now).unwrap_or(i64::MAX);
        let mut attempt = Attempt {
            at,
            account: input.account_name.clone(),
            method: String::new(),
            device: None,
            jti: None,
            ip: input.ip.clone(),
            result: "refused".into(),
            reason: None,
        };
        let token = input
            .oauth2
            .as_ref()
            .and_then(|o| o["token"].as_str())
            .map(str::to_owned);
        let passed = token
            .as_deref()
            .filter(|token| token.starts_with(PASS_PREFIX))
            .map(|pass| self.passed(pass, now));
        let output = if let Some(passed) = passed {
            attempt.method = "a page, through the server".into();
            if let Some(account) = passed {
                attempt.account = Some(account.clone());
                attempt.result = "accepted".into();
                Output {
                    success: true,
                    account_name: Some(account),
                }
            } else {
                attempt.reason = Some("not a pass this server made, or used before".into());
                refused()
            }
        } else if let Some(token) = token {
            attempt.method = "device key".into();
            match self.check(&token, now).await {
                Ok((account, device, jti)) => {
                    attempt.account = Some(account.clone());
                    attempt.device = Some(device);
                    attempt.jti = Some(jti);
                    attempt.result = "accepted".into();
                    Output {
                        success: true,
                        account_name: Some(account),
                    }
                }
                Err((account, reason)) => {
                    attempt.account = account;
                    attempt.reason = Some(reason.into());
                    refused()
                }
            }
        } else if input.passphrase.is_some() {
            attempt.method = "passphrase".into();
            attempt.result = "ergo".into();
            refused()
        } else if input.certfp.is_some() {
            attempt.method = "certificate".into();
            attempt.reason = Some("certificates aren't used".into());
            refused()
        } else {
            attempt.method = "unknown".into();
            refused()
        };
        // Named by whoever tried: kept no longer than an account's name can be.
        attempt.account = attempt
            .account
            .map(|account| account.chars().take(ACCOUNT_CHARS).collect());
        self.log.add(&attempt);
        let logging_in = match (&output.account_name, &input.passphrase) {
            (Some(account), _) if output.success => Some(account),
            (_, Some(_)) => input.account_name.as_ref(),
            _ => None,
        };
        if let Some(account) = logging_in {
            // Nobody listening, as before the sidecar has started, is fine.
            let _ = self.logins.send(account.to_lowercase());
        }
        output
    }

    /// The account, device and `jti` of a good token, or the account it named and why not.
    async fn check(
        &self,
        token: &str,
        now: u64,
    ) -> Result<(String, String, String), (Option<String>, &'static str)> {
        let Some(server) = self.server_name.as_deref() else {
            let named = device_key::parse(token).ok().map(|p| p.claims.sub);
            return Err((named, "device keys aren't on"));
        };
        self.check_for(token, server, now).await
    }

    /// [`Self::check`] for a token that must be for `server`: the content API's
    /// (`crate::accounts`), which takes the same tokens as logins, each once.
    ///
    /// # Errors
    ///
    /// The account it named, and why it isn't good.
    pub async fn check_for(
        &self,
        token: &str,
        server: &str,
        now: u64,
    ) -> Result<(String, String, String), (Option<String>, &'static str)> {
        let parsed = device_key::parse(token).map_err(|why| (None, why))?;
        let account = parsed.claims.sub.clone();
        let named = || Some(account.clone());
        parsed
            .claims
            .check(server, now)
            .map_err(|why| (named(), why))?;
        let mut keys = self.keys.lock().await;
        let key = keys
            .get(&account, &parsed.kid)
            .ok_or((named(), "not one of the account's keys"))?
            .clone();
        let point = device_key::point(&parsed.kid).ok_or((named(), "not a key"))?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, point)
            .verify(parsed.signing_input.as_bytes(), &parsed.signature)
            .map_err(|_| (named(), "not signed by that key"))?;
        {
            let mut used = self
                .used
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            used.retain(|_, exp| *exp > now);
            let once = format!("{} {}", parsed.kid, parsed.claims.jti);
            if used.insert(once, parsed.claims.exp).is_some() {
                return Err((named(), "used before"));
            }
        }
        keys.used(
            &account,
            &parsed.kid,
            i64::try_from(now).unwrap_or(i64::MAX),
        );
        Ok((account, key.label, parsed.claims.jti))
    }
}

fn refused() -> Output {
    Output {
        success: false,
        account_name: None,
    }
}

/// Answers Ergo on the Unix socket at `path` until the process ends, and trims the log daily.
///
/// # Errors
///
/// When it can't listen.
pub async fn serve(path: PathBuf, signin: Arc<Signin>) -> std::io::Result<()> {
    // Left by the sidecar before this one, on the disk they share.
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    info!("answering Ergo's logins");
    let trimming = signin.clone();
    tokio::spawn(async move {
        let mut daily = tokio::time::interval(std::time::Duration::from_hours(24));
        loop {
            daily.tick().await;
            trimming
                .log
                .trim(i64::try_from(crate::now()).unwrap_or(i64::MAX));
        }
    });
    loop {
        let (stream, _) = listener.accept().await?;
        let signin = signin.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            // Ergo sends one line of at most a few kilobytes: a token, or a client's
            // certificates.
            let mut reader = BufReader::new(tokio::io::AsyncReadExt::take(read, 1 << 20));
            if reader.read_line(&mut line).await.is_err() {
                return;
            }
            let input: Input = serde_json::from_str(&line).unwrap_or_default();
            let output = signin.answer(&input, crate::now()).await;
            let mut answer = serde_json::to_vec(&output).unwrap_or_default();
            answer.push(b'\n');
            let _ = write.write_all(&answer).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
    use serde_json::json;

    const SERVER: &str = "zirc.irc.zirc.dev";
    const NOW: u64 = 1_800_000_000;

    struct Device(EcdsaKeyPair);

    impl Device {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            Self(
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap(),
            )
        }
        fn key(&self) -> String {
            device_key::key_text(self.0.public_key().as_ref())
        }
        fn token(&self, claims: &device_key::Claims) -> String {
            let input = device_key::signing_input(&self.key(), claims);
            let signature = self.0.sign(&SystemRandom::new(), input.as_bytes()).unwrap();
            device_key::token(&input, signature.as_ref())
        }
        fn login(&self, account: &str, jti: &str) -> String {
            self.token(&device_key::Claims::new(account, SERVER, NOW, jti.into()))
        }
    }

    fn signin(dir: &tempfile::TempDir) -> Signin {
        Signin::beside(
            &dir.path().join("history.db"),
            Some(SERVER.into()),
            i64::try_from(NOW).unwrap(),
        )
    }

    fn with(token: &str) -> Input {
        Input {
            oauth2: Some(json!({"token": token})),
            ip: Some("203.0.113.9".into()),
            ..Input::default()
        }
    }

    #[tokio::test]
    async fn a_pass_the_server_made_signs_its_person_in_once_and_only_while_good() {
        let dir = tempfile::tempdir().unwrap();
        let signin = signin(&dir);
        let pass = signin.pass("Ada", NOW);
        let late = signin.pass("ada", NOW);

        let first = signin.answer(&with(&pass), NOW + 1).await;
        let again = signin.answer(&with(&pass), NOW + 2).await;
        let expired = signin.answer(&with(&late), NOW + PASS_SECONDS).await;
        let made_up = signin.answer(&with(&format!("{PASS_PREFIX}00")), NOW).await;

        assert_eq!(
            first,
            Output {
                success: true,
                account_name: Some("ada".into())
            }
        );
        assert!(!again.success && !expired.success && !made_up.success);
        let log = signin.log.read();
        assert_eq!(log[0].method, "a page, through the server");
        assert_eq!(log[0].result, "accepted");
        assert_eq!(log[1].result, "refused");
    }

    #[tokio::test]
    async fn a_registered_device_signs_in_once_per_token() {
        let dir = tempfile::tempdir().unwrap();
        let signin = signin(&dir);
        let phone = Device::new();
        assert_eq!(
            signin
                .keys
                .lock()
                .await
                .add("ada", &phone.key(), "Ada's phone", 1),
            Ok(true)
        );

        let token = phone.login("ada", "one");
        let first = signin.answer(&with(&token), NOW).await;
        let again = signin.answer(&with(&token), NOW + 1).await;

        assert_eq!(
            first,
            Output {
                success: true,
                account_name: Some("ada".into())
            }
        );
        assert!(!again.success);
        let log = signin.log.read();
        assert_eq!(log[0].result, "accepted");
        assert_eq!(log[0].device.as_deref(), Some("Ada's phone"));
        assert_eq!(log[0].jti.as_deref(), Some("one"));
        assert_eq!(log[0].ip.as_deref(), Some("203.0.113.9"));
        assert_eq!(log[1].reason.as_deref(), Some("used before"));
        assert_eq!(
            signin.keys.lock().await.of("ada")[&phone.key()].last_used,
            Some(i64::try_from(NOW).unwrap())
        );
    }

    #[tokio::test]
    async fn tokens_are_refused_for_anything_amiss() {
        let dir = tempfile::tempdir().unwrap();
        let signin = signin(&dir);
        let phone = Device::new();
        let stranger = Device::new();
        signin
            .keys
            .lock()
            .await
            .add("ada", &phone.key(), "phone", 1)
            .unwrap();

        let other_server = phone.token(&device_key::Claims::new(
            "ada",
            "other.irc.zirc.dev",
            NOW,
            "a".into(),
        ));
        let old = phone.token(&device_key::Claims::new(
            "ada",
            SERVER,
            NOW - 1000,
            "b".into(),
        ));
        // A stranger's token naming Ada's key: not signed by it.
        let forged = {
            let input = device_key::signing_input(
                &phone.key(),
                &device_key::Claims::new("ada", SERVER, NOW, "c".into()),
            );
            let sig = stranger
                .0
                .sign(&SystemRandom::new(), input.as_bytes())
                .unwrap();
            device_key::token(&input, sig.as_ref())
        };
        for (token, why) in [
            (other_server, "for another server"),
            (old, "expired"),
            (stranger.login("ada", "d"), "not one of the account's keys"),
            (phone.login("bob", "e"), "not one of the account's keys"),
            (forged, "not signed by that key"),
            ("nonsense".to_owned(), "not a device token"),
        ] {
            assert!(!signin.answer(&with(&token), NOW).await.success, "{why}");
            assert_eq!(
                signin.log.read().last().unwrap().reason.as_deref(),
                Some(why)
            );
        }
    }

    #[tokio::test]
    async fn no_server_name_no_device_keys() {
        let dir = tempfile::tempdir().unwrap();
        let signin = Signin::beside(&dir.path().join("history.db"), None, 0);
        let phone = Device::new();
        signin
            .keys
            .lock()
            .await
            .add("ada", &phone.key(), "phone", 1)
            .unwrap();
        assert!(
            !signin
                .answer(&with(&phone.login("ada", "x")), NOW)
                .await
                .success
        );
    }

    #[tokio::test]
    async fn passphrases_are_left_to_ergo_and_logged_without_them() {
        let dir = tempfile::tempdir().unwrap();
        let signin = signin(&dir);
        let input = Input {
            account_name: Some("ada".into()),
            passphrase: Some("hunter2hunter2".into()),
            ip: Some("203.0.113.9".into()),
            ..Input::default()
        };
        assert!(!signin.answer(&input, NOW).await.success);
        let text = std::fs::read_to_string(dir.path().join("zirc-logins.jsonl")).unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        assert_eq!(signin.log.read()[0].result, "ergo");
        assert_eq!(signin.log.read()[0].method, "passphrase");
    }

    #[tokio::test]
    async fn each_account_logging_in_is_told_but_not_a_refused_key() {
        let dir = tempfile::tempdir().unwrap();
        let signin = signin(&dir);
        let mut logins = signin.logins();
        let phone = Device::new();
        signin
            .keys
            .lock()
            .await
            .add("ada", &phone.key(), "Ada's phone", 1)
            .unwrap();

        signin.answer(&with(&phone.login("ada", "one")), NOW).await;
        signin.answer(&with(&phone.login("bob", "two")), NOW).await;
        let passphrase = Input {
            account_name: Some("Ada-MBP".into()),
            passphrase: Some("hunter2hunter2".into()),
            ..Input::default()
        };
        signin.answer(&passphrase, NOW).await;

        assert_eq!(logins.try_recv().unwrap(), "ada");
        // Ergo checks a passphrase itself: told, as it may let it in.
        assert_eq!(logins.try_recv().unwrap(), "ada-mbp");
        assert!(logins.try_recv().is_err(), "bob's refused key was told");
    }

    #[test]
    fn keys_are_kept_bounded_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("history.db");
        let mut keys = Keys::beside(&history);
        let devices: Vec<Device> = (0..=MAX_KEYS).map(|_| Device::new()).collect();
        for device in &devices[..MAX_KEYS] {
            assert_eq!(keys.add("ada", &device.key(), "a device", 1), Ok(true));
        }
        assert_eq!(
            keys.add("ada", &devices[MAX_KEYS].key(), "one more", 1),
            Err(Refused::TooMany)
        );
        assert_eq!(keys.add("ada", &devices[0].key(), "renamed", 2), Ok(false));
        assert_eq!(keys.add("ada", "AAAA", "x", 1), Err(Refused::NotAKey));
        assert_eq!(
            keys.add("bob", &devices[0].key(), " ", 1),
            Err(Refused::Label)
        );

        // Kept on the disk.
        let again = Keys::beside(&history);
        assert_eq!(again.of("ada").len(), MAX_KEYS);
        assert_eq!(again.of("ada")[&devices[0].key()].label, "renamed");

        assert!(keys.remove("ada", &devices[0].key()));
        assert!(!keys.remove("ada", &devices[0].key()));
        keys.forget("ada");
        assert!(Keys::beside(&history).of("ada").is_empty());
    }

    #[tokio::test]
    async fn a_named_account_is_kept_short_and_the_log_small() {
        let dir = tempfile::tempdir().unwrap();
        let mut signin = signin(&dir);
        signin.log.most = 4096;
        let input = Input {
            account_name: Some("é".repeat(10_000)),
            passphrase: Some("x".into()),
            ..Input::default()
        };
        for _ in 0..200 {
            signin.answer(&input, NOW).await;
        }
        let kept = signin.log.read();
        assert!(
            kept.iter()
                .all(|a| a.account.as_ref().unwrap().chars().count() == ACCOUNT_CHARS)
        );
        let bytes = std::fs::metadata(dir.path().join("zirc-logins.jsonl"))
            .unwrap()
            .len();
        assert!(
            bytes <= 4096 && !kept.is_empty(),
            "{bytes} bytes, {} kept",
            kept.len()
        );
    }

    #[test]
    fn the_log_keeps_ninety_days() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("history.db");
        let log = Log::beside(&history, 0);
        for at in [0, KEPT, KEPT + 10] {
            log.add(&Attempt {
                at,
                account: None,
                method: "passphrase".into(),
                device: None,
                jti: None,
                ip: None,
                result: "ergo".into(),
                reason: None,
            });
        }
        let later = Log::beside(&history, KEPT + 5);
        assert_eq!(
            later.read().iter().map(|a| a.at).collect::<Vec<_>>(),
            [KEPT, KEPT + 10]
        );
    }
}
