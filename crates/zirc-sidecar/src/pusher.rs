//! The tenant's pusher: what reaches people's phones while their app is closed, decided here,
//! beside the history, so what anyone said never leaves the organisation's server but
//! encrypted for the phone. zirc.dev only relays it to Apple's or Google's push service, as
//! it relayed Ergo's, whose own web push is off.
//!
//! It hears every message the moment Ergo keeps it: it watches Ergo's history file for writes
//! (the OS's file events, merged over [`THROTTLE`]) and reads only the rows after the last it
//! read ([`kept`]), stopping before one Ergo is still writing. For each, and each phone
//! registered with it ([`store`]), zirc-chat's push rule says whether and why it's pushed: a
//! mention, a reply to or a reaction to what the person said, a direct message, or anything
//! where they asked to hear everything, by how much that conversation notifies on that phone.
//! A channel's goes only to its members (Ergo's
//! `whois`). Each waits [`DELAY`], and goes only if none of the person's apps has read that
//! conversation meanwhile; when one reads it after, the phone is told, so the notification
//! goes from it.
//!
//! Phones register with it, and every app says what its person read, over IRC: a `TAGMSG`
//! to its nick ([`zirc_proto::push`]), on one connection to Ergo on the pod's loopback, as the
//! recorder's ([`crate::recorder`]). Ergo vouches for who sent each.

pub(crate) mod kept;
pub mod store;
pub mod webpush;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tracing::{info, warn};
use zirc_chat::chat::NotifySettings;
use zirc_chat::chat::push::{Heard, why};
use zirc_irc::Line;
use zirc_proto::push::{PUSHER, QUOTE_TAG, Said, TAG, WHY_TAG, Why};

use crate::exporter::{NOTICE, PRIVMSG, TAGMSG};
use kept::{Kept, Place};
use store::Subscription;
use webpush::Vapid;

/// How long a push waits for the person to read it on another device first, as Ergo's did.
pub const DELAY: Duration = Duration::from_secs(15);

/// How long after one write to the history the pusher reads again, so a burst of messages is
/// one read.
pub const THROTTLE: Duration = Duration::from_millis(200);

/// How often it reads where the OS can't tell it of writes: only then.
const FALLBACK: Duration = Duration::from_secs(2);

/// How long it waits on a message Ergo is still writing, reading again every [`THROTTLE`],
/// before it reads past it: one to someone without an account never gets a recipient's copy,
/// nor one Ergo failed to finish ([`kept::after`]).
const UNPLACED: Duration = Duration::from_secs(10);

/// A message Ergo was still writing at the last read, and since when the pusher has waited on
/// it.
type Waiting = Option<(i64, Instant)>;

/// How long what channels an account is in is taken as known.
const MEMBERS_FOR: Duration = Duration::from_secs(30);

/// How long a push service keeps a push for a phone that's off, in seconds.
const TTL: u32 = 86_400;

/// How much of a message a reaction's push quotes, in characters.
const QUOTE: usize = 140;

/// How long the pusher remembers what it pushed and what was read.
const REMEMBER_NANOS: i64 = 86_400 * 1_000_000_000;

/// How long it waits before connecting again, at first and at most.
/// How long after a push that notifies failed in passing (its endpoint not reached, or
/// answering that it or the push service is in trouble) it's sent again, each time.
const AGAIN: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(10)];

const RETRY: Duration = Duration::from_secs(5);
const RETRY_MOST: Duration = Duration::from_mins(5);

/// What the pusher needs: how to reach Ergo, where its history is, and who it says it is.
#[derive(Clone)]
pub struct Pusher {
    /// Ergo's loopback address.
    pub host: String,
    /// Its IRC port there.
    pub port: u16,
    /// The server's gate password, if it has one.
    pub gate: Option<String>,
    /// The endpoint's token, from which its passphrase is derived.
    pub token: String,
    /// Ergo's history, beside which its store is kept.
    pub history: PathBuf,
    /// Ergo's API, for which channels each person is in.
    pub api: crate::ergo::Api,
    /// Who sends the pushes, as VAPID's `sub` says: zirc.dev's address.
    pub subscriber: String,
    /// How long a push waits for the person to read it elsewhere: [`DELAY`].
    pub delay: Duration,
}

/// Why a phone zirc.dev hands the pusher isn't kept or taken away.
#[derive(Debug, Clone, Copy)]
pub enum PhoneRefused {
    /// What's wrong with it.
    Invalid(&'static str),
    /// The store can't be written.
    Store,
}

/// Keeps a phone zirc.dev registered for `phone.account` (zirc-proto's `push::FanOut`), as
/// the phone's own [`Said::Register`] would be kept: an organisation on the phone it doesn't
/// show hears it so, the phone being connected only to the one it shows. Only an endpoint on
/// `subscriber`'s relay, as for any phone.
///
/// # Errors
///
/// When the phone isn't one, or the store can't be written.
pub async fn add_phone(
    history: &Path,
    subscriber: &str,
    phone: zirc_proto::push::Phone,
) -> Result<(), PhoneRefused> {
    let account = phone.account.to_lowercase();
    if !crate::accounts::valid_name(&account) {
        return Err(PhoneRefused::Invalid("not an account name"));
    }
    if !relayed(&phone.endpoint, subscriber) || phone.p256dh.is_empty() || phone.auth.is_empty() {
        return Err(PhoneRefused::Invalid("not a relayed phone with its keys"));
    }
    // zirc.dev passes the phone's levels on unread: checked here, where they're read.
    if serde_json::from_value::<NotifySettings>(phone.settings.clone()).is_err() {
        return Err(PhoneRefused::Invalid("not notification settings"));
    }
    let subscription = Subscription {
        account,
        endpoint: phone.endpoint,
        p256dh: phone.p256dh,
        auth: phone.auth,
        settings: phone.settings.to_string(),
    };
    let store = store::path_for(history);
    let now = jiff::Timestamp::now().as_second();
    let kept =
        tokio::task::spawn_blocking(move || store::keep(&store::open(&store)?, &subscription, now))
            .await;
    if matches!(kept, Ok(Ok(()))) {
        Ok(())
    } else {
        warn!("the pusher couldn't keep a phone zirc.dev registered");
        Err(PhoneRefused::Store)
    }
}

/// Takes the phone at `endpoint` away from `account`, as zirc.dev does when the phone leaves
/// this organisation or signs out: only that account's.
///
/// # Errors
///
/// When `account` isn't one, or the store can't be written.
pub async fn remove_phone(
    history: &Path,
    account: &str,
    endpoint: &str,
) -> Result<(), PhoneRefused> {
    let account = account.to_lowercase();
    if !crate::accounts::valid_name(&account) {
        return Err(PhoneRefused::Invalid("not an account name"));
    }
    let (store, endpoint) = (store::path_for(history), endpoint.to_owned());
    let kept = tokio::task::spawn_blocking(move || {
        store::unregister(&store::open(&store)?, &account, &endpoint)
    })
    .await;
    if matches!(kept, Ok(Ok(()))) {
        Ok(())
    } else {
        warn!("the pusher couldn't forget a phone zirc.dev took away");
        Err(PhoneRefused::Store)
    }
}

/// Forgets every phone of `account`, as it's erased: someone who leaves the organisation
/// hears nothing more from it.
pub async fn forget_phones(history: &Path, account: &str) {
    let (store, account) = (store::path_for(history), account.to_lowercase());
    let gone =
        tokio::task::spawn_blocking(move || store::forget_account(&store::open(&store)?, &account))
            .await;
    if !matches!(gone, Ok(Ok(_))) {
        warn!("the pusher couldn't forget an erased account's phones");
    }
}

/// Whether `endpoint` is one the web app's relay made for a phone, `<subscriber>api/push/<id>`,
/// `subscriber` being the web app's address as the pusher says it (`https://zirc.dev/`). The
/// pusher posts to nothing else: an endpoint is what a member's app says, and anywhere else
/// it would have the tenant's pod post to whatever a member named, inside the cluster too.
pub(crate) fn relayed(endpoint: &str, subscriber: &str) -> bool {
    subscriber.ends_with('/')
        && endpoint
            .strip_prefix(subscriber)
            .and_then(|rest| rest.strip_prefix("api/push/"))
            .is_some_and(|id| {
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
}

/// One phone, and how much each of its conversations notifies.
pub struct Phone {
    /// Its registration.
    pub subscription: Subscription,
    /// Its settings, read.
    pub settings: NotifySettings,
}

impl Phone {
    fn of(subscription: Subscription) -> Self {
        let settings = serde_json::from_str(&subscription.settings).unwrap_or_default();
        Self {
            subscription,
            settings,
        }
    }
}

/// One push decided, to send once [`DELAY`] has passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Push {
    /// The phone's endpoint.
    pub endpoint: String,
    /// The person's account.
    pub account: String,
    /// The conversation as they name it.
    pub conversation: String,
    /// The channel, when it's said in one, which they must be in.
    pub channel: Option<String>,
    /// When it was said, in nanoseconds.
    pub nanotime: i64,
    /// What's pushed: the line as the person's app would have heard it.
    pub line: String,
}

/// The pushes `kept` makes for `phones`, `answered` being the message it answers or reacts
/// to, if any, and `now` in seconds since the Unix epoch.
#[must_use]
pub(crate) fn decide(
    kept: &Kept,
    phones: &[Phone],
    answered: Option<&crate::exporter::Said>,
    now: i64,
) -> Vec<Push> {
    let mut pushes = Vec::new();
    let sender = kept.said.account.to_lowercase();
    for phone in phones {
        let account = phone.subscription.account.as_str();
        if account == sender {
            continue;
        }
        let (conversation, target, channel) = match &kept.place {
            Place::Channel(channel) => (channel.as_str(), channel.as_str(), Some(channel)),
            Place::Direct(to) if to == account => (kept.said.nick.as_str(), account, None),
            Place::Direct(_) => continue,
        };
        let Some(mut line) = line_of(kept, target) else {
            return pushes;
        };
        let theirs = answered.is_some_and(|said| said.account.eq_ignore_ascii_case(account));
        let heard = Heard::of(&line, conversation, theirs);
        let Some(reason) = why(&phone.settings, now, account, &heard) else {
            continue;
        };
        line.tags
            .push((WHY_TAG.to_owned(), Some(reason.as_str().to_owned())));
        if let (Why::Reaction, Some(said)) = (reason, answered) {
            line.tags.push((QUOTE_TAG.to_owned(), Some(quote(said))));
        }
        // A line too long for the push services, as an agent's request with its card
        // often is, goes as how it starts rather than not at all.
        let whole = line.to_string().len();
        if whole > zirc_proto::push::LINE_BYTES
            && let Some(body) = line.params.get_mut(1)
        {
            *body = zirc_proto::push::fitted(body, whole - body.len());
        }
        pushes.push(Push {
            endpoint: phone.subscription.endpoint.clone(),
            account: account.to_owned(),
            conversation: conversation.to_owned(),
            channel: channel.cloned(),
            nanotime: kept.nanotime,
            line: line.to_string(),
        });
    }
    pushes
}

/// `kept` as the line the person's app would have heard, sent to `target`: its time, id and
/// sender's account, and the tags Ergo kept of it. `None` for what isn't a message or a
/// reaction.
fn line_of(kept: &Kept, target: &str) -> Option<Line> {
    let said = &kept.said;
    let command = match said.kind {
        PRIVMSG => "PRIVMSG",
        NOTICE => "NOTICE",
        TAGMSG => "TAGMSG",
        _ => return None,
    };
    let mut line = if command == "TAGMSG" {
        Line::new(command, [target])
    } else {
        Line::new(command, [target, said.text.as_str()])
    };
    line.source = Some(said.mask.clone());
    let time = jiff::Timestamp::from_nanosecond(i128::from(kept.nanotime)).ok()?;
    line.tags = vec![
        ("time".to_owned(), Some(format!("{time:.3}"))),
        ("msgid".to_owned(), Some(kept.msgid.clone())),
    ];
    if crate::exporter::known_account(&said.account).is_some() {
        line.tags
            .push(("account".to_owned(), Some(said.account.clone())));
    }
    // Never a sender's own word for why it's pushed: only the pusher's.
    line.tags.extend(
        said.tags
            .iter()
            .filter(|(key, _)| !key.starts_with(TAG))
            .map(|(key, value)| (key.clone(), Some(value.clone()))),
    );
    Some(line)
}

/// The start of what `said` says, as people read it, for a reaction's push.
fn quote(said: &crate::exporter::Said) -> String {
    let text = zirc_proto::decode(&said.text)
        .ok()
        .flatten()
        .map_or_else(|| said.text.clone(), |message| message.event.human_text());
    let text = text
        .strip_prefix("\u{1}ACTION ")
        .map_or(text.as_str(), |action| action.trim_end_matches('\u{1}'));
    text.chars().take(QUOTE).collect()
}

/// What an app said to the pusher in `line`, with its sender's account and nick, or `None`
/// for any other line.
#[must_use]
pub fn told(line: &Line) -> Option<(String, String, Said)> {
    if line.command != "TAGMSG"
        || !line
            .params
            .first()
            .is_some_and(|target| target.eq_ignore_ascii_case(PUSHER))
    {
        return None;
    }
    let account = line
        .tag("account")
        .and_then(crate::exporter::known_account)
        .map(str::to_lowercase)?;
    let nick = line.source.as_deref()?.split('!').next()?.to_owned();
    Some((account, nick, Said::of(line.tag(TAG)?)?))
}

/// What's remembered between pushes: what each person read and was pushed, by conversation,
/// and which channels each is in.
#[derive(Default)]
struct Memory {
    /// (account, conversation folded) → read up to, in nanoseconds.
    read: HashMap<(String, String), i64>,
    /// (account, conversation folded) → the newest pushed, in nanoseconds.
    pushed: HashMap<(String, String), i64>,
    /// account → when asked, and the channels it's in, folded.
    members: HashMap<String, (Instant, BTreeSet<String>)>,
}

impl Memory {
    fn forget_old(&mut self, now: i64) {
        let since = now - REMEMBER_NANOS;
        self.read.retain(|_, at| *at > since);
        self.pushed.retain(|_, at| *at > since);
    }
}

/// The pusher as it runs.
struct Running {
    pusher: Pusher,
    store: PathBuf,
    vapid: Vapid,
    http: reqwest::Client,
    memory: Mutex<Memory>,
}

fn key(account: &str, conversation: &str) -> (String, String) {
    (account.to_owned(), conversation.to_lowercase())
}

/// The pusher's VAPID public key, which zirc.dev checks its pushes against, made the first
/// time it's asked for.
///
/// # Errors
///
/// When its store can't be read or written.
pub fn public_key(history: &Path) -> Result<String, String> {
    load_vapid(&store::path_for(history)).map(|vapid| vapid.public())
}

fn load_vapid(store: &Path) -> Result<Vapid, String> {
    let connection = store::open(store).map_err(|error| error.to_string())?;
    let pkcs8 = store::vapid(&connection, || {
        Vapid::make().map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))
    })
    .map_err(|error| error.to_string())?;
    Vapid::from_pkcs8(&pkcs8).map_err(|error| error.to_string())
}

impl Pusher {
    /// Pushes until the process ends: listens to apps on its own connection to Ergo,
    /// connecting again whenever it drops, and reads the history as Ergo writes it. `account`
    /// makes sure its account exists with the derived passphrase, as the recorder's.
    pub async fn run<F, Fut>(self, account: F)
    where
        F: Fn(bool) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let store = store::path_for(&self.history);
        let vapid = loop {
            match load_vapid(&store) {
                Ok(vapid) => break vapid,
                Err(error) => warn!(%error, "the pusher can't open its store"),
            }
            tokio::time::sleep(RETRY).await;
        };
        let running = Arc::new(Running {
            pusher: self,
            store,
            vapid,
            http: crate::http(),
            memory: Mutex::new(Memory::default()),
        });
        tokio::spawn(Arc::clone(&running).follow());
        running.listen(account).await;
    }
}

impl Running {
    fn memory(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Listens to apps, connecting again whenever the connection drops.
    async fn listen<F, Fut>(self: &Arc<Self>, account: F)
    where
        F: Fn(bool) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let mut wait = RETRY;
        let mut reset = false;
        loop {
            if account(reset).await {
                match self.connected().await {
                    Ok(()) => wait = RETRY,
                    Err(zirc_irc::Error::Sasl(why)) => {
                        warn!(%why, "the pusher's login was refused: setting its passphrase again");
                        reset = true;
                    }
                    Err(error) => warn!(%error, "the pusher's connection ended"),
                }
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(RETRY_MOST);
        }
    }

    async fn connected(self: &Arc<Self>) -> Result<(), zirc_irc::Error> {
        let pusher = &self.pusher;
        let config = crate::say::config(
            &pusher.host,
            pusher.port,
            pusher.gate.as_deref(),
            &pusher.token,
            PUSHER,
        );
        let (client, mut incoming) = zirc_irc::connect(&config).await?;
        info!("the pusher is listening");
        while let Some(line) = incoming.recv().await {
            let Some((account, nick, said)) = told(&line) else {
                continue;
            };
            if let Some(answer) = self.heard(&account, said).await {
                let mut line = Line::new("TAGMSG", [nick]);
                line.tags.push((TAG.to_owned(), Some(answer.tag())));
                client.send(line)?;
            }
        }
        Ok(())
    }

    /// Does what `account`'s app said, and says what to answer it, if anything.
    async fn heard(self: &Arc<Self>, account: &str, said: Said) -> Option<Said> {
        match said {
            Said::Register {
                endpoint,
                p256dh,
                auth,
                settings,
            } => {
                if !relayed(&endpoint, &self.pusher.subscriber) {
                    warn!("the pusher refused a phone whose endpoint isn't zirc.dev's relay");
                    return None;
                }
                let subscription = Subscription {
                    account: account.to_owned(),
                    endpoint: endpoint.clone(),
                    p256dh,
                    auth,
                    settings: settings.to_string(),
                };
                let store = self.store.clone();
                let now = jiff::Timestamp::now().as_second();
                let kept = tokio::task::spawn_blocking(move || {
                    store::keep(&store::open(&store)?, &subscription, now)
                })
                .await;
                match kept {
                    Ok(Ok(())) => Some(Said::Registered { endpoint }),
                    Ok(Err(error)) => {
                        warn!(%error, "the pusher couldn't keep a phone");
                        None
                    }
                    Err(error) => {
                        warn!(%error, "keeping a phone didn't finish");
                        None
                    }
                }
            }
            Said::Unregister { endpoint } => {
                let (store, account) = (self.store.clone(), account.to_owned());
                let gone = tokio::task::spawn_blocking(move || {
                    store::unregister(&store::open(&store)?, &account, &endpoint)
                })
                .await;
                if !matches!(gone, Ok(Ok(()))) {
                    warn!("the pusher couldn't forget a phone");
                }
                None
            }
            Said::Read { target, at } => {
                self.read(account, &target, &at).await;
                None
            }
            Said::Registered { .. } => None,
        }
    }

    /// `account` read `target` up to `at`: what waits for it there isn't sent, and if its
    /// phones were pushed something there already, they're told, so it goes from them.
    async fn read(&self, account: &str, target: &str, at: &str) {
        let Some(nanos) = at
            .parse::<jiff::Timestamp>()
            .ok()
            .and_then(|at| i64::try_from(at.as_nanosecond()).ok())
        else {
            return;
        };
        let told = {
            let mut memory = self.memory();
            let key = key(account, target);
            let read = memory.read.entry(key.clone()).or_default();
            *read = (*read).max(nanos);
            let pushed = memory
                .pushed
                .get(&key)
                .is_some_and(|pushed| *pushed <= nanos);
            if pushed {
                memory.pushed.remove(&key);
            }
            pushed
        };
        if told {
            let mut line = Line::new("MARKREAD", [target, &format!("timestamp={at}")]);
            line.source = Some(PUSHER.to_owned());
            self.send_all(account, &line.to_string()).await;
        }
    }

    /// Reads the history as Ergo writes it, until the process ends.
    async fn follow(self: Arc<Self>) {
        let (_watcher, mut written) = match watch(&self.pusher.history) {
            Ok((watcher, written)) => (Some(watcher), Some(written)),
            Err(error) => {
                warn!(%error, "the pusher can't watch the history: it reads it every 2s instead");
                (None, None)
            }
        };
        let mut waiting = None;
        loop {
            match Arc::clone(&self).read_new(waiting).await {
                Ok(still) => waiting = still,
                Err(error) => warn!(%error, "the pusher couldn't read the history"),
            }
            tokio::time::sleep(THROTTLE).await;
            // What finishes a message may have been written before the read could see it,
            // with no write to come after: read again without waiting for one.
            if waiting.is_some() {
                continue;
            }
            match written.as_mut() {
                Some(written) => {
                    if written.recv().await.is_none() {
                        return;
                    }
                }
                None => tokio::time::sleep(FALLBACK).await,
            }
        }
    }

    /// Reads what's new in the history, and sends each push it makes once [`DELAY`] has
    /// passed; says what it's waiting on Ergo to finish writing, if anything.
    async fn read_new(self: Arc<Self>, mut waiting: Waiting) -> Result<Waiting, String> {
        let (history, store) = (self.pusher.history.clone(), self.store.clone());
        let (pushes, waiting) = tokio::task::spawn_blocking(move || {
            pushes_after(&history, &store, &mut waiting).map(|pushes| (pushes, waiting))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        for push in pushes {
            if let Some(channel) = &push.channel
                && !self.is_member(&push.account, channel).await
            {
                info!("a push not sent: its person isn't in the channel now");
                continue;
            }
            let running = Arc::clone(&self);
            tokio::spawn(async move {
                tokio::time::sleep(running.pusher.delay).await;
                running.deliver(push).await;
            });
        }
        Ok(waiting)
    }

    /// Whether `account` is in `channel` now, as Ergo says, asked at most every
    /// [`MEMBERS_FOR`].
    async fn is_member(&self, account: &str, channel: &str) -> bool {
        #[derive(Deserialize)]
        struct Whois {
            #[serde(default)]
            present: bool,
            #[serde(default)]
            channels: Option<Vec<Joined>>,
        }
        #[derive(Deserialize)]
        struct Joined {
            name: String,
        }
        let known = self
            .memory()
            .members
            .get(account)
            .filter(|(asked, _)| asked.elapsed() < MEMBERS_FOR)
            .map(|(_, channels)| channels.contains(&channel.to_lowercase()));
        if let Some(known) = known {
            return known;
        }
        let asked: Result<Whois, _> = self
            .pusher
            .api
            .call("whois", serde_json::json!({"nickname": account}))
            .await;
        let channels: BTreeSet<String> = match asked {
            Ok(whois) if whois.present => whois
                .channels
                .unwrap_or_default()
                .into_iter()
                .map(|joined| joined.name.to_lowercase())
                .collect(),
            Ok(_) => BTreeSet::new(),
            Err(error) => {
                warn!(%error, "the pusher couldn't ask Ergo which channels someone is in");
                return false;
            }
        };
        let member = channels.contains(&channel.to_lowercase());
        self.memory()
            .members
            .insert(account.to_owned(), (Instant::now(), channels));
        member
    }

    /// Sends `push`, unless the person read the conversation meanwhile.
    async fn deliver(&self, push: Push) {
        {
            let mut memory = self.memory();
            let key = key(&push.account, &push.conversation);
            if memory
                .read
                .get(&key)
                .is_some_and(|read| *read >= push.nanotime)
            {
                info!("a push not sent: read on another device meanwhile");
                return;
            }
            let pushed = memory.pushed.entry(key).or_default();
            *pushed = (*pushed).max(push.nanotime);
            memory.forget_old(push.nanotime);
        }
        let subscription = self.subscription(&push.endpoint).await;
        if let Some(subscription) = subscription {
            self.send(&subscription, &push.line, true).await;
        } else {
            info!("a push not sent: its phone is no longer on file");
        }
    }

    async fn subscription(&self, endpoint: &str) -> Option<Subscription> {
        let (store, endpoint) = (self.store.clone(), endpoint.to_owned());
        tokio::task::spawn_blocking(move || {
            store::all(&store::open(&store).ok()?)
                .ok()?
                .into_iter()
                .find(|subscription| subscription.endpoint == endpoint)
        })
        .await
        .ok()
        .flatten()
    }

    /// Sends `line` to every phone of `account`'s.
    async fn send_all(&self, account: &str, line: &str) {
        let (store, account) = (self.store.clone(), account.to_owned());
        let phones = tokio::task::spawn_blocking(move || {
            store::all(&store::open(&store).ok()?).ok().map(|all| {
                all.into_iter()
                    .filter(|subscription| subscription.account == account)
                    .collect::<Vec<_>>()
            })
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
        for phone in phones {
            self.send(&phone, line, false).await;
        }
    }

    /// Encrypts `line` for `subscription`'s phone and sends it; `urgent` for what notifies.
    /// A phone its push service says is gone is forgotten.
    async fn send(&self, subscription: &Subscription, line: &str, urgent: bool) {
        // Kept before the pusher checked them: never posted to.
        if !relayed(&subscription.endpoint, &self.pusher.subscriber) {
            warn!("a phone on file whose endpoint isn't zirc.dev's relay: forgetting it");
            self.forget(&subscription.endpoint).await;
            return;
        }
        let body = match webpush::encrypt(&subscription.p256dh, &subscription.auth, line.as_bytes())
        {
            Ok(body) => body,
            Err(error) => {
                warn!(%error, "the pusher couldn't encrypt for a phone: forgetting it");
                self.forget(&subscription.endpoint).await;
                return;
            }
        };
        let expires = jiff::Timestamp::now().as_second() + 12 * 3600;
        let authorization =
            match self
                .vapid
                .authorization(&subscription.endpoint, &self.pusher.subscriber, expires)
            {
                Ok(authorization) => authorization,
                Err(error) => {
                    warn!(%error, "the pusher couldn't sign a push");
                    return;
                }
            };
        // What notifies is tried again when it fails in passing: sent once, a push the
        // relay couldn't hand to the push service just then was lost, an agent's question
        // with it (four in two days, 21 and 22 Nov 2026).
        let mut again = AGAIN.iter();
        loop {
            let sent = self
                .http
                .post(&subscription.endpoint)
                .header("Authorization", &authorization)
                .header("Content-Encoding", "aes128gcm")
                .header("Content-Type", "application/octet-stream")
                .header("TTL", TTL.to_string())
                .header("Urgency", if urgent { "high" } else { "normal" })
                .timeout(Duration::from_secs(10))
                .body(body.clone())
                .send()
                .await
                .map(|response| response.status());
            let passing = match &sent {
                Ok(status) => status.is_server_error() || *status == 429,
                Err(_) => true,
            };
            if let (true, true, Some(wait)) = (urgent, passing, again.next()) {
                info!(outcome = ?sent, "a push failed in passing: trying again in {wait:?}");
                tokio::time::sleep(*wait).await;
                continue;
            }
            match sent {
                Ok(status) if status.is_success() => info!(urgent, "a push was taken by the relay"),
                Ok(status) if status == 404 || status == 410 => {
                    info!(%status, "a phone's push endpoint is gone: forgetting it");
                    self.forget(&subscription.endpoint).await;
                }
                Ok(status) => warn!(%status, "a push was refused"),
                Err(error) => warn!(%error, "a push didn't reach its endpoint"),
            }
            return;
        }
    }

    async fn forget(&self, endpoint: &str) {
        let (store, endpoint) = (self.store.clone(), endpoint.to_owned());
        let _ =
            tokio::task::spawn_blocking(move || store::forget(&store::open(&store)?, &endpoint))
                .await;
    }
}

/// The pushes the history's rows after the last read make, for every phone in the store at
/// `store`, and keeps where it read to. The first time, it starts from the history's end:
/// nothing said before it ran is pushed. It stops before a message Ergo is still writing,
/// which `waiting` then names, until it's finished or has been waited on for [`UNPLACED`].
fn pushes_after(
    history: &Path,
    store: &Path,
    waiting: &mut Waiting,
) -> rusqlite::Result<Vec<Push>> {
    let kept_store = store::open(store)?;
    if !history.exists() {
        // A new server: everything it keeps from now on is new.
        if store::last(&kept_store)?.is_none() {
            store::set_last(&kept_store, 0)?;
        }
        return Ok(Vec::new());
    }
    let connection = crate::trim::open(history)?;
    let Some(mut last) = store::last(&kept_store)? else {
        store::set_last(&kept_store, kept::newest(&connection)?)?;
        return Ok(Vec::new());
    };
    let phones: Vec<Phone> = store::all(&kept_store)?
        .into_iter()
        .map(Phone::of)
        .collect();
    let now = jiff::Timestamp::now().as_second();
    let mut pushes = Vec::new();
    loop {
        let pass = waiting
            .filter(|(_, since)| since.elapsed() >= UNPLACED)
            .map(|(id, _)| id);
        let read = kept::after(&connection, last, pass)?;
        for row in read.kept.iter().filter(|_| !phones.is_empty()) {
            let answered = row
                .said
                .tags
                .get("+draft/reply")
                .map(|msgid| kept::by_msgid(&connection, msgid))
                .transpose()?
                .flatten();
            pushes.extend(decide(row, &phones, answered.as_ref(), now));
        }
        // What it read and made of it, in counts: why nothing reached a phone is otherwise
        // invisible (step 0, 24 Nov 2026: no push for a day, and nothing said why).
        if !read.kept.is_empty() {
            let accounts: BTreeSet<&str> = phones
                .iter()
                .map(|phone| phone.subscription.account.as_str())
                .collect();
            info!(
                messages = read.kept.len(),
                phones = phones.len(),
                accounts = accounts.len(),
                pushes = pushes.len(),
                "the pusher read new messages"
            );
        }
        let moved = read.newest != last;
        if moved {
            last = read.newest;
            store::set_last(&kept_store, last)?;
        }
        if let Some(id) = read.unplaced {
            if waiting.is_none_or(|(waited, _)| waited != id) {
                *waiting = Some((id, Instant::now()));
            }
            return Ok(pushes);
        }
        *waiting = None;
        if !moved {
            return Ok(pushes);
        }
    }
}

/// Watches the history at `history` for writes to it and its WAL, each one coalesced into the
/// channel: one waiting at most; it watches until what it returns is dropped. On Linux, where
/// tenants run, the kernel says each write at once (inotify). macOS's file events don't say
/// writes to a file a process keeps open, as Ergo keeps its WAL, so there, in development, it
/// looks at the two files' size and time every [`THROTTLE`] instead: to the nanosecond, as
/// `notify`'s `PollWatcher` keeps a file's time only to the second, so it missed a write in the
/// second it last looked, when SQLite had made and removed the WAL between its looks and the
/// file kept its size (flaky-159, 24 Nov 2026).
fn watch(history: &Path) -> notify::Result<(Box<dyn Send>, tokio::sync::mpsc::Receiver<()>)> {
    let (written, heard) = tokio::sync::mpsc::channel(1);
    // The file and its log, where Ergo writes each message: not its shared memory (`-shm`),
    // which reading it touches too.
    let name = history
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let names = [name.clone(), format!("{name}-wal")];
    let directory = history.parent().unwrap_or_else(|| Path::new("."));
    #[cfg(target_os = "linux")]
    {
        use notify::Watcher as _;
        let handler = move |event: notify::Result<notify::Event>| {
            let ours = event.is_ok_and(|event| {
                (event.kind.is_modify() || event.kind.is_create())
                    && event.paths.iter().any(|path| {
                        path.file_name().is_some_and(|file| {
                            names.iter().any(|name| file.to_string_lossy() == *name)
                        })
                    })
            });
            if ours {
                let _ = written.try_send(());
            }
        };
        let mut watcher = notify::recommended_watcher(handler)?;
        watcher.watch(directory, notify::RecursiveMode::NonRecursive)?;
        Ok((Box::new(watcher), heard))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // As inotify can't watch a directory that isn't there.
        std::fs::metadata(directory).map_err(notify::Error::io)?;
        let files = names.map(|name| directory.join(name));
        let seen = move || {
            files
                .iter()
                .map(|file| {
                    std::fs::metadata(file)
                        .ok()
                        .map(|metadata| (metadata.len(), metadata.modified().ok()))
                })
                .collect::<Vec<_>>()
        };
        let mut last = seen();
        let looking = tokio::spawn(async move {
            loop {
                tokio::time::sleep(THROTTLE).await;
                let now = seen();
                if now != last {
                    last = now;
                    let _ = written.try_send(());
                }
            }
        });
        Ok((Box::new(Looking(looking.abort_handle())), heard))
    }
}

/// What keeps [`watch`] looking at the history on macOS: dropped, it stops.
#[cfg(not(target_os = "linux"))]
struct Looking(tokio::task::AbortHandle);

#[cfg(not(target_os = "linux"))]
impl Drop for Looking {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests;
