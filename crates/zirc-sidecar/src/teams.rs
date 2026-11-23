//! Teams' channels on a tenant's Ergo, made to match what the controller asks for.
//!
//! Each team channel is registered with `ChanServ` to the founder account `zirc`, which the
//! sidecar logs in as (also identifying as Ergo's operator, to take ops in a channel that
//! already has people in it). Channels stay open to everyone in the organisation; a team's
//! members get ops in its channels, but not their machines ([`crate::machines`]), as a
//! persistent mode (`ChanServ AMODE +o`), which Ergo gives them on joining and takes away,
//! there and then, when it's removed. A channel no team has any more keeps its registration
//! but nobody's ops. An invite-only team's channels are `+i`, and its people's machines
//! (whose [`crate::machines`] says) get a persistent `+v`, which Ergo lets past `+i` (and an account, unlike a
//! nick, can't be pretended to); everyone else needs an `INVITE`. The controller's
//! Ergo tests check this on the release.
//!
//! Owners get ops in every other channel there is too, but threads (`#<parent>/<name>`, not
//! one of the rooms repositories had before 20 Nov 2026: [`zirc_proto::is_thread`]): registering a
//! channel keeps it however empty, and a thread is nobody's to keep, as the user chose on
//! 5 Nov: "I want to stop creating channels for every branch, and sweep them". A team's own
//! channels are registered whatever their name.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zirc_irc::{Credentials, Incoming, Line};

use crate::ergo::{Api, Operator};

/// The account team channels are registered to.
pub const FOUNDER: &str = "zirc";

/// A channel as the controller wants it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// The channel's name.
    pub name: String,
    /// People's accounts with ops in it (not their machines').
    pub accounts: BTreeSet<String>,
    /// Only they and their machines may join (`Team::invite_only`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invite_only: bool,
}

/// What the controller asks for: the channels teams have, those they no longer do, and the
/// owners, who get ops in every channel, teams' or not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Desired {
    /// The channels teams have.
    pub channels: Vec<Channel>,
    #[serde(default)]
    /// The channels no team has any more.
    pub removed: Vec<String>,
    #[serde(default)]
    /// The owners, who get ops in every channel.
    pub owners: BTreeSet<String>,
    /// Who may read what (`ZircServerSpec::visibility`), which the sidecar applies to the
    /// history it serves.
    #[serde(default)]
    pub visibility: zirc_proto::desired::Visibility,
    /// The keys zirc.dev signs browser logins with, and whether the server runs workflows.
    #[serde(default)]
    pub public: Public,
    /// Whether the server answers content requests itself, and as which host name
    /// (`crate::content`); left out, it doesn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    /// Every machine's account with its owner's handle
    /// (`zirc_proto::desired::DesiredState::machines`), which [`crate::machines`] keeps; left
    /// out, by a controller from before them, the sidecar keeps what it had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machines: Option<BTreeMap<String, String>>,
    /// Those of `machines` that are their owner's own client under another name, not a
    /// machine (`zirc_proto::desired::MachineKind::Client`); left out, by a controller from
    /// before them, the sidecar keeps what it had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clients: Option<BTreeSet<String>>,
}

/// What the controller tells the sidecar of the organisation beside its channels.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Public {
    /// The organisation's public keys, PEM (`ZircServerSpec::token_keys`): a browser login's
    /// token signed with any of them is good, as with Ergo's own logins, so zirc.dev can move
    /// to a new key without the server hearing of it first.
    #[serde(default)]
    pub keys: Vec<String>,
    /// The first of them, as sidecars from before `keys` read it: the controller sends both,
    /// as the tenants' sidecar image is pinned apart from its own and may be older. Read only
    /// when `keys` is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The server runs its organisation's workflows (`spec.workflows`, `crate::workflows`):
    /// off unless zirc.dev turns them on, so no other organisation sees their host.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workflows: bool,
}

/// The content API's settings (`zirc_proto::desired::DesiredState::content_api`), with the
/// teams: whom people's tokens must be for, and who the organisation's admins are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// The server's host name, such as `acme.irc.zirc.dev`: a token's audience.
    pub server: String,
    /// The organisation's admins, by their handles.
    #[serde(default)]
    pub admins: BTreeSet<String>,
    /// The pages that may ask from a browser (CORS): zirc.dev's own origin, such as
    /// `https://zirc.dev`, and no other.
    #[serde(default)]
    pub origins: BTreeSet<String>,
}

/// How each channel came out: `None` if it's as asked, or what went wrong.
pub type Outcome = BTreeMap<String, Option<String>>;

/// Every account among `existing` that gets ops: the people asked for who have one. Never
/// their machines, whose accounts are never a person's handle.
#[must_use]
pub fn opped(people: &BTreeSet<String>, existing: &[String]) -> BTreeSet<String> {
    existing
        .iter()
        .filter(|account| people.contains(*account))
        .cloned()
        .collect()
}

/// Who gets which persistent mode in a channel: its people with accounts ops (`+o`), and in
/// an invite-only one, their machines (whose [`crate::machines`] says) voice (`+v`), which
/// lets them in.
#[must_use]
pub fn modes(
    channel: &Channel,
    existing: &[String],
    machines: Option<&crate::machines::Owners>,
) -> BTreeMap<String, &'static str> {
    let mut modes = all("+o", opped(&channel.accounts, existing));
    if channel.invite_only {
        let machines = existing.iter().filter(|account| {
            crate::machines::owner_in(machines, account)
                .is_some_and(|handle| channel.accounts.contains(&handle))
        });
        modes.extend(machines.map(|machine| (machine.clone(), "+v")));
    }
    modes
}

fn all(mode: &'static str, accounts: BTreeSet<String>) -> BTreeMap<String, &'static str> {
    accounts.into_iter().map(|a| (a, mode)).collect()
}

#[derive(Deserialize)]
struct Listed {
    accounts: Vec<Named>,
}

#[derive(Deserialize)]
struct Named {
    #[serde(rename = "accountName")]
    name: String,
}

/// Makes Ergo's channels match `desired`, as the founder: the teams' channels, and with
/// owners, every other channel there is too, with ops for the owners. The outcome has every
/// channel it went over.
///
/// # Errors
///
/// When it can't reach Ergo or log in; a single channel's trouble is in the outcome instead.
pub async fn reconcile(
    api: &Api,
    operator: &Operator,
    desired: &Desired,
) -> Result<Outcome, String> {
    let others = if desired.owners.is_empty() {
        BTreeSet::new()
    } else {
        channels(api).await?
    };
    let (client, mut incoming, existing) = log_in(api, operator).await?;
    let mut session = Session {
        client,
        incoming: &mut incoming,
    };
    let mut outcome = Outcome::new();
    for channel in &desired.channels {
        let result = session
            .channel(
                &channel.name,
                &modes(channel, &existing, desired.machines.as_ref()),
                Some(channel.invite_only),
            )
            .await;
        outcome.insert(channel.name.clone(), result.err());
    }
    let owners = all("+o", opped(&desired.owners, &existing));
    for name in &desired.removed {
        if outcome.contains_key(name) {
            continue;
        }
        // No team's now, so open again.
        let result = session.channel(name, &owners, Some(false)).await;
        outcome.insert(name.clone(), result.err());
    }
    for name in others.iter().filter(|name| owned(name)) {
        if outcome.contains_key(name) {
            continue;
        }
        let result = session.channel(name, &owners, None).await;
        outcome.insert(name.clone(), result.err());
    }
    let _ = session.client.quit("done");
    Ok(outcome)
}

/// Gives the owners ops in the channels made since `done`, which it returns with them, as
/// [`reconcile`] does. Without owners, or new channels, it does nothing.
///
/// # Errors
///
/// When it can't reach Ergo or log in.
pub async fn sweep(
    api: &Api,
    operator: &Operator,
    owners: &BTreeSet<String>,
    done: &BTreeSet<String>,
) -> Result<Outcome, String> {
    if owners.is_empty() {
        return Ok(Outcome::new());
    }
    let new: Vec<String> = channels(api)
        .await?
        .difference(done)
        .filter(|name| owned(name))
        .cloned()
        .collect();
    if new.is_empty() {
        return Ok(Outcome::new());
    }
    let (client, mut incoming, existing) = log_in(api, operator).await?;
    let mut session = Session {
        client,
        incoming: &mut incoming,
    };
    let owners = all("+o", opped(owners, &existing));
    let mut outcome = Outcome::new();
    for name in new {
        let result = session.channel(&name, &owners, None).await;
        outcome.insert(name, result.err());
    }
    let _ = session.client.quit("done");
    Ok(outcome)
}

/// Whether owners get ops in `channel`, a channel no team lists: in all but threads.
fn owned(channel: &str) -> bool {
    !zirc_proto::is_thread(channel)
}

/// The channels on the server, lowercase.
pub(crate) async fn channels(api: &Api) -> Result<BTreeSet<String>, String> {
    #[derive(Deserialize)]
    struct List {
        channels: Vec<Listed>,
    }
    #[derive(Deserialize)]
    struct Listed {
        name: String,
    }
    let list: List = api
        .call("list", json!({}))
        .await
        .map_err(|e| e.to_string())?;
    Ok(list
        .channels
        .into_iter()
        .map(|c| c.name.to_lowercase())
        .collect())
}

/// Logs in to Ergo as the founder, also its operator, with the accounts there are.
pub(crate) async fn log_in(
    api: &Api,
    operator: &Operator,
) -> Result<(zirc_irc::Client, Incoming, Vec<String>), String> {
    // The founder account, with the operator's password, which only the sidecar knows.
    let founder = json!({"accountName": FOUNDER, "passphrase": operator.password});
    let made: Value = api
        .call("ns/saregister", founder.clone())
        .await
        .map_err(|e| e.to_string())?;
    if made["success"] != true {
        let _: Value = api
            .call("ns/passwd", founder)
            .await
            .map_err(|e| e.to_string())?;
    }
    let listed: Listed = api
        .call("ns/list", json!({}))
        .await
        .map_err(|e| e.to_string())?;
    let existing: Vec<String> = listed
        .accounts
        .into_iter()
        .map(|a| a.name.to_lowercase())
        .collect();

    let mut config = zirc_irc::Config::new(operator.host.clone(), operator.port, FOUNDER);
    config.tls = false;
    config.server_password.clone_from(&operator.server_password);
    config.sasl = Some(Credentials {
        account: FOUNDER.into(),
        password: operator.password.clone(),
    });
    let (client, mut incoming) = zirc_irc::connect(&config)
        .await
        .map_err(|e| e.to_string())?;
    let mut session = Session {
        client,
        incoming: &mut incoming,
    };
    session.send(Line::new("OPER", ["admin", operator.password.as_str()]))?;
    session
        .until(
            |l| l.command == "381",
            |l| matches!(l.command.as_str(), "464" | "491"),
        )
        .await?;
    let Session { client, .. } = session;
    Ok((client, incoming, existing))
}

/// An IRC connection to Ergo as the founder and operator ([`log_in`]).
pub(crate) struct Session<'a> {
    pub(crate) client: zirc_irc::Client,
    pub(crate) incoming: &'a mut Incoming,
}

impl Session<'_> {
    pub(crate) fn send(&self, line: Line) -> Result<(), String> {
        self.client.send(line).map_err(|e| e.to_string())
    }

    /// Reads until a line `done` matches (returned), or `failed` does (its text, as an error).
    pub(crate) async fn until(
        &mut self,
        done: impl Fn(&Line) -> bool,
        failed: impl Fn(&Line) -> bool,
    ) -> Result<Line, String> {
        let incoming = &mut *self.incoming;
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(line) = incoming.recv().await {
                if failed(&line) {
                    return Err(line.params.last().cloned().unwrap_or_default());
                }
                if done(&line) {
                    return Ok(line);
                }
            }
            Err("Ergo closed the connection".to_owned())
        })
        .await
        .unwrap_or_else(|_| Err("Ergo didn't answer in time".to_owned()))
    }

    /// One channel, made registered to the founder, with exactly `modes` given (`+o` or `+v`
    /// by account), and made invite-only or not, or left as it is.
    async fn channel(
        &mut self,
        name: &str,
        modes: &BTreeMap<String, &'static str>,
        invite_only: Option<bool>,
    ) -> Result<(), String> {
        if !name.starts_with('#') || name.contains([' ', ',', '\x07']) {
            return Err("not a channel name".into());
        }
        let info = self.chanserv(&format!("INFO {name}")).await?;
        let fresh = info.contains("is not registered");
        if fresh {
            self.register(name).await?;
        } else if info.contains("is registered") {
            // Then its founder, and when; the founder has to be zirc for the rest to work.
            let founder = self.chanserv_notice().await?;
            self.chanserv_notice().await?;
            if !founder.eq_ignore_ascii_case(&format!("Founder: {FOUNDER}")) {
                return Err(format!("registered to someone else ({founder})"));
            }
        } else {
            return Err(format!("ChanServ: {info}"));
        }

        // Ergo lists the count, then a line per account.
        let count = self.chanserv(&format!("AMODE {name}")).await?;
        let Some(count) = count
            .strip_prefix(&format!("Channel {name} has "))
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse::<usize>().ok())
        else {
            return Err(format!("ChanServ: {count}"));
        };
        let mut listed = BTreeMap::new();
        for _ in 0..count {
            let line = self.chanserv_notice().await?;
            if let Some((account, mode)) = line
                .strip_prefix("Account ")
                .and_then(|rest| rest.split_once(" receives mode "))
                && matches!(mode, "+o" | "+v")
            {
                listed.insert(account.to_lowercase(), mode.to_owned());
            }
        }
        for (account, mode) in &listed {
            if modes.get(account).copied() != Some(mode.as_str()) {
                self.amode(name, &mode.replacen('+', "-", 1), account)
                    .await?;
            }
        }
        for (account, mode) in modes {
            if listed.get(account).map(String::as_str) != Some(*mode) {
                self.amode(name, mode, account).await?;
            }
        }
        if let Some(invite_only) = invite_only {
            let mode = if invite_only { "+i" } else { "-i" };
            self.send(Line::new("SAMODE", [name, mode]))?;
        }
        if fresh {
            // Ergo gave whoever made it ops, a machine as often as not.
            let opped = modes
                .iter()
                .filter(|(_, mode)| **mode == "+o")
                .map(|(account, _)| account.clone())
                .collect();
            self.deop_others(name, &opped).await?;
        }
        self.header(name).await;
        Ok(())
    }

    /// Gives `name` its header, the topic every app shows under its name, when it's a room
    /// that has one ([`zirc_proto::room_header`]) and has none yet: a person's own `/topic`
    /// is never replaced. Best effort: a room without its header is as it was, so a failure
    /// is said and nothing else waits on it.
    async fn header(&mut self, name: &str) {
        let Some(header) = zirc_proto::room_header(name) else {
            return;
        };
        if let Err(why) = self.set_header(name, header).await {
            tracing::warn!("couldn't give {name} its header: {why}");
        }
    }

    async fn set_header(&mut self, name: &str, header: &str) -> Result<(), String> {
        // A room with a topic is left alone, and LIST says so from outside: joining to ask,
        // on every teams set, showed `zirc joined, zirc left` in every headered room, dozens
        // of times an hour (12 Nov 2026).
        if self.listed_topic(name).await.is_some_and(|t| !t.is_empty()) {
            return Ok(());
        }
        // Ergo answers a TOPIC query only from inside the channel (442 otherwise), so join:
        // the burst has the topic (332) when there is one, and says nothing when there is
        // none. As the founder, taking ops as `register` does, then leave.
        self.send(Line::new("JOIN", [name]))?;
        let has = std::sync::atomic::AtomicBool::new(false);
        let burst = self
            .until(
                |l| {
                    if l.command == "332" {
                        has.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    l.command == "366"
                },
                |l| matches!(l.command.as_str(), "471" | "473" | "474" | "475" | "403"),
            )
            .await
            .map(drop)
            .map_err(|why| format!("couldn't join {name}: {why}"));
        let said = match burst {
            Ok(()) if has.load(std::sync::atomic::Ordering::Relaxed) => Ok(()),
            Ok(()) => self.say_header(name, header).await,
            Err(why) => Err(why),
        };
        self.send(Line::new("PART", [name]))?;
        said
    }

    /// `name`'s topic as `LIST` gives it (322), which needs no joining: empty when it has
    /// none, `None` when it isn't listed or Ergo doesn't answer, which joining then settles.
    async fn listed_topic(&mut self, name: &str) -> Option<String> {
        self.send(Line::new("LIST", [name])).ok()?;
        let topic = std::sync::Mutex::new(None);
        self.until(
            |l| {
                if l.command == "322"
                    && l.params
                        .get(1)
                        .is_some_and(|c| c.eq_ignore_ascii_case(name))
                {
                    *topic
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(l.params.get(3).cloned().unwrap_or_default());
                }
                l.command == "323"
            },
            |_| false,
        )
        .await
        .ok()?;
        topic
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn say_header(&mut self, name: &str, header: &str) -> Result<(), String> {
        self.send(Line::new("SAMODE", [name, "+o", FOUNDER]))?;
        self.send(Line::new("TOPIC", [name, header]))?;
        self.until(
            |l| l.command == "TOPIC",
            |l| matches!(l.command.as_str(), "482" | "477"),
        )
        .await
        .map(drop)
        .map_err(|why| format!("TOPIC was refused: {why}"))
    }

    /// Takes ops from everyone in a channel but `opped`, and the founder.
    async fn deop_others(&mut self, name: &str, opped: &BTreeSet<String>) -> Result<(), String> {
        // WHOX: each member's nick, flags (`@` for ops) and account (`0` for none).
        self.send(Line::new("WHO", [name, "%nfa"]))?;
        let mut ops = Vec::new();
        loop {
            let line = self
                .until(
                    |l| matches!(l.command.as_str(), "354" | "315"),
                    |l| l.command == "403",
                )
                .await?;
            if line.command == "315" {
                break;
            }
            if let [_, nick, flags, account] = line.params.as_slice()
                && flags.contains('@')
                && !nick.eq_ignore_ascii_case(FOUNDER)
                && !opped.contains(&account.to_lowercase())
            {
                ops.push(nick.clone());
            }
        }
        for nick in ops {
            self.send(Line::new("SAMODE", [name, "-o", nick.as_str()]))?;
        }
        Ok(())
    }

    /// Registers a channel to the founder: join it, take ops (as the operator, should others
    /// be in it), register, leave.
    async fn register(&mut self, name: &str) -> Result<(), String> {
        self.send(Line::new("JOIN", [name]))?;
        self.joined(name).await?;
        self.send(Line::new("SAMODE", [name, "+o", FOUNDER]))?;
        let reply = self.chanserv(&format!("REGISTER {name}")).await?;
        self.send(Line::new("PART", [name]))?;
        if reply.contains("successfully registered") || reply.contains("already registered") {
            Ok(())
        } else {
            Err(format!("ChanServ: {reply}"))
        }
    }

    async fn amode(&mut self, name: &str, mode: &str, account: &str) -> Result<(), String> {
        let reply = self
            .chanserv(&format!("AMODE {name} {mode} {account}"))
            .await?;
        if reply.starts_with("Successfully") || reply.starts_with("No changes") {
            Ok(())
        } else {
            Err(format!("ChanServ, {mode} {account}: {reply}"))
        }
    }

    /// Says `command` to `ChanServ`, returning its first answer.
    pub(crate) async fn chanserv(&mut self, command: &str) -> Result<String, String> {
        self.send(Line::new("PRIVMSG", ["ChanServ", command]))?;
        self.chanserv_notice().await
    }

    pub(crate) async fn chanserv_notice(&mut self) -> Result<String, String> {
        let line = self
            .until(|l| from(l, "ChanServ") && l.command == "NOTICE", |_| false)
            .await?;
        Ok(line.params.last().cloned().unwrap_or_default())
    }

    /// Waits until a channel's joined.
    pub(crate) async fn joined(&mut self, name: &str) -> Result<(), String> {
        self.until(
            |l| l.command == "366",
            |l| matches!(l.command.as_str(), "471" | "473" | "474" | "475" | "403"),
        )
        .await
        .map(drop)
        .map_err(|why| format!("couldn't join {name}: {why}"))
    }
}

fn from(line: &Line, nick: &str) -> bool {
    line.source
        .as_deref()
        .is_some_and(|s| s.split('!').next() == Some(nick))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn owners_get_channels_and_standing_rooms_but_not_threads() {
        assert!(owned("#proj"));
        assert!(owned("#proj/agents"));
        assert!(owned("#proj/landings"));
        assert!(!owned("#proj/web-c26b2cc"));
    }

    #[test]
    fn people_get_ops_not_their_machines() {
        let existing = [
            "alice",
            "alice-mbp",
            "alice-ci",
            "bob",
            "bob-mbp",
            "alicex-mbp",
        ]
        .map(String::from);
        assert_eq!(opped(&set(&["alice"]), &existing), set(&["alice"]));
        assert_eq!(opped(&set(&[]), &existing), set(&[]));
        // Someone without an account yet gets ops once they have one.
        assert_eq!(opped(&set(&["carol"]), &existing), set(&[]));
    }

    #[test]
    fn an_invite_only_channel_lets_in_its_people_s_machines() {
        let existing =
            ["alice", "m3-mac", "build-1", "alice-mbp", "bob", "bob-mbp"].map(String::from);
        // Whose machines are, as the server was told: not by their names.
        let owners: crate::machines::Owners = [
            ("m3-mac", "alice"),
            ("build-1", "alice"),
            ("bob-mbp", "bob"),
        ]
        .map(|(a, o)| (a.to_owned(), o.to_owned()))
        .into();
        let mut channel = Channel {
            name: "#secret".into(),
            accounts: set(&["alice"]),
            invite_only: true,
        };
        let modes = |channel: &Channel| -> Vec<(String, &str)> {
            modes(channel, &existing, Some(&owners))
                .into_iter()
                .collect()
        };
        assert_eq!(
            modes(&channel),
            [
                ("alice".to_owned(), "+o"),
                ("build-1".to_owned(), "+v"),
                ("m3-mac".to_owned(), "+v"),
            ]
        );
        // Open, the machines need nothing to get in.
        channel.invite_only = false;
        assert_eq!(modes(&channel), [("alice".to_owned(), "+o")]);
    }
}
