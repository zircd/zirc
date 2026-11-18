//! Anonymous usage analytics: the events declared in [`zirc_proto::telemetry`], sent to
//! `PostHog`'s capture API.
//!
//! Recording an event only appends a line to a queue in zirc's directory, so it never
//! blocks or slows a command. A long-running program, zircd or the TUI, sends what's queued
//! in the background with [`send_every`], as one `/batch` request, and drops it if that
//! fails.
//!
//! It's on by default, sending to zirc's project ([`DEFAULT_KEY`]) unless `config.toml` names
//! another with `posthog_key`, or none with `posthog_key = ""`. `telemetry = false` in
//! `config.toml` turns it off everywhere, zircd included. `ZIRC_TELEMETRY=0` or
//! `DO_NOT_TRACK=1` turns it off only for what runs with them in its environment: agents'
//! runs inherit a shell's, but a zircd started by launchd or systemd never sees it. Each install has a random id; once the person signs in
//! it's linked to their account's opaque id with `$identify`, never to a name. No event
//! carries an IP address or a host name, but `PostHog` sees the address a batch comes from
//! and records where it is.
//!
//! An install of zirc's own CI, tests or team says so in every event (`internal`), so the
//! dashboards can leave it out: run anything with `ZIRC_TELEMETRY_INTERNAL=1` once and the
//! install remembers it, zircd started later by the system included.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use zirc_proto::telemetry::{
    AppOpened, Client, ConnectionFailed, Context, Event, Failure, Feature, FeatureUsed, Os,
    UpdateInstalled,
};

use crate::settings::Settings;

pub mod dashboards;

/// zirc's `PostHog` project's key, which events are sent with when `posthog_key` isn't
/// set. It's public by design, as in every web page that sends to `PostHog`; zirc.dev's
/// posthog-js has the same key.
pub const DEFAULT_KEY: &str = "phc_oAWKHhhrwuakRyBDcpnUrGqyyfAVDn5i9ULVSy8tHALr";

/// Where events go when `posthog_host` isn't set: `PostHog` Cloud's EU region.
pub const DEFAULT_HOST: &str = "https://eu.i.posthog.com";

/// How often [`send_every`] sends what's queued.
pub const EVERY: Duration = Duration::from_mins(5);

/// The most the queue holds: past it, events are dropped until it's sent.
const QUEUE_MAX_BYTES: u64 = 512 * 1024;

/// How long sending may take before it's given up.
const SEND_WITHIN: Duration = Duration::from_secs(10);

/// What's said once, on the first interactive run with analytics on.
pub const NOTICE: &str = "zirc sends anonymous usage counts to PostHog (which commands and \
                          features you use, never what you write; PostHog sees your IP \
                          address): set telemetry = false in config.toml to turn them off, \
                          zircd's included (ZIRC_TELEMETRY=0 turns off only what runs in \
                          that shell).";

/// An app's switch for its own analytics, on its Settings' Privacy page.
pub const SETTING: &str = "Send anonymous usage counts";

/// Said under [`SETTING`].
pub const SETTING_NOTE: &str = "Which version runs and which features you use, never what you \
                                write or to whom. PostHog, which receives them, sees your IP \
                                address.";

/// Records and sends analytics events, or, when off, does nothing. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Telemetry(Option<Arc<On>>);

#[derive(Debug)]
struct On {
    /// Where the queue and the install's id are kept.
    dir: PathBuf,
    /// zirc's directory, whose `config.toml` says whether it's still on, when its
    /// `telemetry` setting is the one that counts: a long-running zircd hears it turned off.
    follows: Option<PathBuf>,
    key: String,
    host: String,
    context: Context,
}

/// Whether analytics is on, as `config.toml`'s `telemetry` and the environment (`env`, a
/// variable's value by name) say: on unless one of them turns it off.
pub fn enabled(setting: Option<bool>, env: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name| set(&env, name);
    if set("DO_NOT_TRACK").is_some_and(|value| !off(&value)) {
        return false;
    }
    if set("ZIRC_TELEMETRY").is_some_and(|value| off(&value)) {
        return false;
    }
    // Under `bazel test`, here or in a repository that tests zirc's programs, or in CI
    // (Buildkite, GitHub Actions and most others set CI): tests aren't usage.
    if set("TEST_TMPDIR").is_some() || set("CI").is_some_and(|value| !off(&value)) {
        return false;
    }
    setting != Some(false)
}

/// An environment variable's value, trimmed and lowercased, if it's set to anything.
fn set(env: impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name)
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

/// Whether a variable's value says no.
fn off(value: &str) -> bool {
    matches!(value, "0" | "false" | "off" | "no")
}

/// Whether the install whose analytics files are in `dir` is zirc's own, as
/// `ZIRC_TELEMETRY_INTERNAL` says (`env`), now or once before: a yes is remembered there.
fn internal(dir: &Path, env: impl Fn(&str) -> Option<String>) -> bool {
    let marker = dir.join("internal");
    if set(env, "ZIRC_TELEMETRY_INTERNAL").is_some_and(|value| !off(&value)) {
        let _ = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&marker, ""));
        return true;
    }
    marker.exists()
}

impl Telemetry {
    /// One that does nothing.
    #[must_use]
    pub fn off() -> Self {
        Self(None)
    }

    /// The one `client` uses with zirc's directory `home`, as its `config.toml` and the
    /// process's environment say. Without a `config.toml` it's on; with one that can't be
    /// read, off, since it may say so.
    #[must_use]
    pub fn load(home: &Path, client: Client) -> Self {
        let Ok(settings) = read_settings(home) else {
            return Self::off();
        };
        Self::with(home, client, settings.as_ref(), |name| {
            std::env::var(name).ok()
        })
    }

    /// The one an app with settings of its own uses, such as the desktop app: as
    /// [`Telemetry::load`], but at the app's own `version`, and with its own `setting` for
    /// analytics, when it has one, instead of `config.toml`'s.
    #[must_use]
    pub fn for_app(home: &Path, client: Client, version: &str, setting: Option<bool>) -> Self {
        let Ok(settings) = read_settings(home) else {
            return Self::off();
        };
        let own = setting.is_some();
        let setting = setting.or_else(|| settings.as_ref().and_then(|s| s.telemetry));
        let mut telemetry =
            Self::build(home, client, version, settings.as_ref(), setting, |name| {
                std::env::var(name).ok()
            });
        if own && let Some(on) = telemetry.0.as_mut().and_then(Arc::get_mut) {
            on.follows = None;
        }
        telemetry
    }

    /// The one `client` uses with zirc's directory `home`, as `settings`, if there are
    /// any, and `env` say.
    #[must_use]
    pub fn with(
        home: &Path,
        client: Client,
        settings: Option<&Settings>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let setting = settings.and_then(|settings| settings.telemetry);
        Self::build(
            home,
            client,
            env!("CARGO_PKG_VERSION"),
            settings,
            setting,
            env,
        )
    }

    fn build(
        home: &Path,
        client: Client,
        version: &str,
        settings: Option<&Settings>,
        setting: Option<bool>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let key = settings
            .and_then(|settings| settings.posthog_key.as_deref())
            .unwrap_or(DEFAULT_KEY)
            .trim();
        if key.is_empty() || !enabled(setting, &env) {
            return Self::off();
        }
        let dir = dir(home);
        let internal = internal(&dir, &env);
        let host = settings
            .and_then(|settings| settings.posthog_host.as_deref())
            .unwrap_or(DEFAULT_HOST)
            .trim_end_matches('/')
            .to_owned();
        Self(Some(Arc::new(On {
            dir,
            follows: Some(home.to_owned()),
            key: key.to_owned(),
            host,
            context: Context {
                client,
                version: version.to_owned(),
                os: Os::current(),
                // Releases are optimised builds without debug assertions; tests and
                // developers' builds have them.
                dev: cfg!(debug_assertions),
                internal,
            },
        })))
    }

    /// Records that the app opened, and, if it was opened at another version before, that
    /// that version was updated to this one: for apps updated by something else, as the
    /// desktop app is by Sparkle.
    pub fn opened(&self) {
        let Some(on) = &self.0 else { return };
        self.record(&Event::AppOpened(AppOpened {}));
        let client = serde_json::to_value(on.context.client).unwrap_or_default();
        let path = on
            .dir
            .join(format!("version-{}", client.as_str().unwrap_or("app")));
        let current = &on.context.version;
        if let Some(from) = read(&path).filter(|from| from != current) {
            self.record(&Event::UpdateInstalled(UpdateInstalled {
                from,
                to: current.clone(),
            }));
        }
        let _ = std::fs::create_dir_all(&on.dir).and_then(|()| std::fs::write(&path, current));
    }

    /// Whether it records anything.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.0.is_some()
    }

    /// Queues `event`, to be sent later. Never fails: an event that can't be queued is
    /// dropped.
    pub fn record(&self, event: &Event) {
        self.record_in(event, None);
    }

    /// Queues `event`, as [`Telemetry::record`] does, as one of the organisation zirc calls
    /// `org` too, when [`link_organisation`] gave its opaque id: the event's group in
    /// `PostHog`, as zirc.dev's and the controller's events about it are.
    pub fn record_in(&self, event: &Event, org: Option<&str>) {
        let Some(on) = &self.0 else { return };
        let mut properties = event.properties();
        if let Value::Object(context) = serde_json::to_value(&on.context).unwrap_or_default() {
            properties.extend(context);
        }
        properties.insert("$lib".into(), "zirc".into());
        if let Some(id) = org.and_then(|org| organisation(&on.dir, org)) {
            properties.insert("$groups".into(), json!({"organisation": id}));
        }
        let line = json!({
            "event": event.name(),
            "properties": properties,
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });
        let _ = append(&on.dir.join("queue.jsonl"), &format!("{line}\n"));
    }

    /// Says [`NOTICE`] to `out` the first time it's asked to while on, when `interactive`
    /// (the program's output is a terminal), and remembers that it has.
    pub fn notice(&self, interactive: bool, out: &mut impl std::io::Write) {
        let Some(on) = &self.0 else { return };
        let shown = on.dir.join("notice-shown");
        if !interactive || shown.exists() {
            return;
        }
        if writeln!(out, "{NOTICE}").is_ok() {
            let _ = std::fs::create_dir_all(&on.dir).and_then(|()| std::fs::write(shown, ""));
        }
    }

    /// Sends what's queued as one batch, with the install linked to the account first if
    /// it's been signed in to since. What can't be sent is dropped.
    pub async fn send(&self, http: &reqwest::Client) {
        let Some(on) = &self.0 else { return };
        let queue = on.dir.join("queue.jsonl");
        // Turned off in config.toml since it started: what was queued is dropped, unsent.
        if let Some(home) = &on.follows
            && read_settings(home).map_or(true, |settings| {
                settings.is_some_and(|settings| settings.telemetry == Some(false))
            })
        {
            let _ = std::fs::remove_file(&queue);
            return;
        }
        // Taken from the queue first, so what's recorded meanwhile, or by another program
        // sending at the same time, isn't sent twice.
        let sending = on.dir.join(format!("sending-{}.jsonl", std::process::id()));
        if std::fs::rename(&queue, &sending).is_err() {
            return;
        }
        let text = std::fs::read_to_string(&sending).unwrap_or_default();
        let _ = std::fs::remove_file(&sending);
        let install = install_id(&on.dir);
        let account = read(&on.dir.join("account"));
        let mut batch = Vec::new();
        let identify = account
            .clone()
            .filter(|account| read(&on.dir.join("identified")).as_ref() != Some(account));
        if let (Some(account), Some(install)) = (&identify, &install) {
            batch.push(json!({
                "event": "$identify",
                "distinct_id": account,
                "properties": {"$anon_distinct_id": install},
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            }));
        }
        let Some(distinct_id) = account.or(install) else {
            return;
        };
        for line in text.lines() {
            if let Ok(Value::Object(mut event)) = serde_json::from_str::<Value>(line) {
                event.insert("distinct_id".into(), distinct_id.clone().into());
                batch.push(Value::Object(event));
            }
        }
        if batch.is_empty() {
            return;
        }
        let sent = http
            .post(format!("{}/batch/", on.host))
            .timeout(SEND_WITHIN)
            .json(&json!({"api_key": on.key, "batch": batch}))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        match (sent, identify) {
            (Ok(_), Some(account)) => {
                let _ = std::fs::write(on.dir.join("identified"), account);
            }
            (Ok(_), None) => {}
            (Err(err), _) => tracing::debug!("dropped analytics events: {err}"),
        }
    }
}

/// Links this install, with zirc's directory `home`, to the account with opaque id
/// `account_id`, as signing in does: the next batch sent says so with `$identify`, and
/// later events are the account's. Whether analytics is on or not.
pub fn link(home: &Path, account_id: &str) {
    let dir = dir(home);
    let _ = std::fs::create_dir_all(&dir)
        .and_then(|()| crate::paths::write_private(&dir.join("account"), account_id));
}

/// Remembers that the organisation zirc calls `org`, which this install, with zirc's
/// directory `home`, signed in to, has opaque id `organisation_id` on zirc.dev: its events
/// are then grouped by it ([`Telemetry::record_in`]). Whether analytics is on or not.
pub fn link_organisation(home: &Path, org: &str, organisation_id: &str) {
    if !crate::settings::valid_org_name(org) {
        return;
    }
    let dir = dir(home).join("organisations");
    let _ =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(org), organisation_id));
}

/// Links this install, with zirc's directory `home`, to what a sign-in at `web` brought in
/// `credentials`: the account ([`link`]) and each organisation's opaque id
/// ([`link_organisation`]). Every client that signs in calls it, so its events are the
/// account's. Whether analytics is on or not.
pub fn link_login(home: &Path, credentials: &zirc_proto::login::Credentials, web: &str) {
    if let Some(account_id) = &credentials.account_id {
        link(home, account_id);
    }
    for org in crate::login::each_org(credentials, web).unwrap_or_default() {
        if let Some(id) = &org.credentials.organisation_id {
            let name = org
                .name
                .unwrap_or_else(|| crate::settings::org_name(&org.credentials.server));
            link_organisation(home, &name, id);
        }
    }
}

/// The opaque id [`link_organisation`] remembered for `org`, in the analytics files at `dir`.
fn organisation(dir: &Path, org: &str) -> Option<String> {
    if !crate::settings::valid_org_name(org) {
        return None;
    }
    read(&dir.join("organisations").join(org))
}

/// Why connecting failed, as [`Event::ConnectionFailed`] says it: by its class, since the
/// error's text can name hosts.
#[must_use]
pub fn failure(err: &zirc_irc::Error) -> Failure {
    match err {
        zirc_irc::Error::Io(_) => Failure::Network,
        zirc_irc::Error::Tls(_) => Failure::Tls,
        zirc_irc::Error::Timeout(_) => Failure::Timeout,
        zirc_irc::Error::Closed(_) => Failure::Closed,
        zirc_irc::Error::BadPassword => Failure::Password,
        zirc_irc::Error::Sasl(_) => Failure::Sasl,
        zirc_irc::Error::Nick(_) => Failure::Nick,
        _ => Failure::Other,
    }
}

/// [`Event::ConnectionFailed`] for `reason`.
#[must_use]
pub fn connection_failed(reason: Failure) -> Event {
    Event::ConnectionFailed(ConnectionFailed { reason })
}

/// The settings in `home`'s `config.toml`, if there is one.
fn read_settings(home: &Path) -> Result<Option<Settings>, ()> {
    match std::fs::read_to_string(crate::paths::config(home)) {
        Ok(text) => Settings::from_toml(&text).map(Some).map_err(|_| ()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// Where analytics keeps its files in zirc's directory `home`.
fn dir(home: &Path) -> PathBuf {
    home.join("telemetry")
}

/// The install's random id, made the first time it's needed.
fn install_id(dir: &Path) -> Option<String> {
    std::fs::create_dir_all(dir).ok()?;
    crate::machine::random_id_in(&dir.join("install-id"))
}

/// A file's text, trimmed, if it has any.
fn read(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Appends `line` to the queue at `path`, unless it's full.
fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|queue| queue.len() >= QUEUE_MAX_BYTES) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// Sends what `telemetry` queues every `every`, starting now, for as long as it runs: for
/// zircd and the TUI, in the background.
pub async fn send_every(telemetry: Telemetry, every: Duration) {
    if !telemetry.is_on() {
        return;
    }
    let Some(http) = http() else { return };
    let mut ticks = tokio::time::interval(every);
    loop {
        ticks.tick().await;
        if !PAUSED.load(Ordering::Relaxed) {
            telemetry.send(&http).await;
        }
    }
}

/// Sends what `telemetry` queues every [`EVERY`] on a thread of its own, for apps that
/// don't run tokio, such as the desktop app.
pub fn send_in_background(telemetry: Telemetry) {
    if !telemetry.is_on() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("analytics".into())
        .spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(send_every(telemetry, EVERY));
            }
        });
}

/// An HTTP client to [`Telemetry::send`] with.
#[must_use]
pub fn http() -> Option<reqwest::Client> {
    // reqwest's rustls comes without a crypto provider: use ring, as the rest of zirc does.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(concat!("zirc/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
}

static GLOBAL: OnceLock<Telemetry> = OnceLock::new();

/// Whether the person turned analytics off while the app runs: from then on [`record`]
/// records nothing and [`send_every`] sends nothing, in this process.
static PAUSED: AtomicBool = AtomicBool::new(false);

/// Stops recording and sending in this process, or starts again, as an app's setting is
/// changed while it runs. Starting again can't turn on what [`init`] set up off.
pub fn pause(paused: bool) {
    PAUSED.store(paused, Ordering::Relaxed);
}

/// What saying something to `target` counts as: a direct message, to a group or a person,
/// or one with an agent session (`machine/session`); nothing in a channel.
#[must_use]
pub fn said_to(target: &str) -> Option<Feature> {
    if crate::chat::is_group(target) {
        Some(Feature::Dm)
    } else if crate::chat::is_channel(target) {
        None
    } else if target.contains('/') {
        Some(Feature::AgentDm)
    } else {
        Some(Feature::Dm)
    }
}

/// Records that `feature` was used, with the process's [`Telemetry`].
pub fn used(feature: Feature) {
    record(&Event::FeatureUsed(FeatureUsed { feature }));
}

/// Makes `telemetry` the one [`record`] uses in this process, unless one already is, and
/// returns the one that is.
pub fn init(telemetry: Telemetry) -> Telemetry {
    GLOBAL.get_or_init(|| telemetry).clone()
}

/// Records `event` with the process's [`Telemetry`], if [`init`] has set one up.
pub fn record(event: &Event) {
    record_in(event, None);
}

/// Records `event` with the process's [`Telemetry`], as one of the organisation zirc calls
/// `org` ([`Telemetry::record_in`]).
pub fn record_in(event: &Event, org: Option<&str>) {
    if let (Some(telemetry), false) = (GLOBAL.get(), PAUSED.load(Ordering::Relaxed)) {
        telemetry.record_in(event, org);
    }
}

/// The event's properties as recorded, for tests.
#[cfg(test)]
fn queued(home: &Path) -> Vec<serde_json::Map<String, Value>> {
    std::fs::read_to_string(dir(home).join("queue.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[cfg(test)]
mod tests;
