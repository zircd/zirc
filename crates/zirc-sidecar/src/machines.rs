//! Whose each machine is: its account and its owner's handle, as zirc.dev says
//! (`zirc_proto::desired::DesiredState::machines`), through the controller's teams
//! (`crate::teams::Desired::machines`) or as an account's made (`PUT /v1/accounts/{name}`).
//!
//! A machine's name says nothing of whose it is (`m3-mac`), so what the server was told is the
//! only word on it: an account it wasn't told of is a person's, if it could be a handle, and
//! otherwise no one's.
//!
//! What it was told is kept beside Ergo's history, so a restart doesn't go back to reading
//! names before the controller says it again. Each machine is told to everyone as its host,
//! `machine/<owner>` (`zirc_proto::names::machine_host`), which the sidecar sets as Ergo's
//! operator ([`Machines::unhosted`]), and which the apps and zircd read.
//!
//! Some of these accounts are no machine but a person's own client under another name, such
//! as `mattias-irc`, their IRC client (`zirc_proto::desired::MachineKind::Client`): the
//! person at a keyboard, where a machine is their agents. Whose it is, is kept as a machine's
//! is; its host says `client/<owner>` (`zirc_proto::names::client_host`), by which zircd takes
//! its word as its owner's own, as it never takes a machine's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use tracing::warn;

/// Each machine's account and its owner's handle, lower case.
pub type Owners = BTreeMap<String, String>;

/// The server's machines, as it was told of them: one per sidecar, shared by its handlers.
#[derive(Clone, Debug, Default)]
pub struct Machines(Arc<RwLock<Known>>);

#[derive(Debug, Default)]
struct Known {
    /// None until it's told of any, when it knows of none.
    owners: Option<Owners>,
    /// Where they're kept, if anywhere.
    path: Option<PathBuf>,
    /// Those of `owners` that are their owner's own client, not a machine.
    clients: BTreeSet<String>,
    /// The hosts set since the sidecar started, by account. Ergo keeps them, but a restart
    /// sets them all again, once.
    hosted: BTreeMap<String, String>,
}

impl Machines {
    /// What was kept beside Ergo's `history`, if anything, kept there again as it changes.
    #[must_use]
    pub fn beside(history: &Path) -> Self {
        let path = history.with_file_name("zirc-machines.json");
        let owners = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Owners>(&bytes).ok());
        let clients = std::fs::read(clients_beside(&path))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeSet<String>>(&bytes).ok())
            .unwrap_or_default();
        clients_are(&clients);
        if let Some(owners) = &owners {
            agents_are(owners.keys());
        }
        Self(Arc::new(RwLock::new(Known {
            owners,
            path: Some(path),
            clients,
            hosted: BTreeMap::new(),
        })))
    }

    /// The server's machines are `owners` now, every one.
    pub fn set(&self, owners: &Owners) {
        let owners: Owners = owners
            .iter()
            .map(|(account, owner)| (account.to_lowercase(), owner.to_lowercase()))
            .collect();
        agents_are(owners.keys());
        let mut known = self.write();
        known.owners = Some(owners);
        save(&known);
    }

    /// `account` is one of `owner`'s machines, just made: the server knows it at once rather
    /// than when the controller next says, and goes by what it's told from then on.
    pub fn add(&self, account: &str, owner: &str) {
        let account = account.to_lowercase();
        agents_are([&account]);
        let mut known = self.write();
        known
            .owners
            .get_or_insert_with(Owners::new)
            .insert(account, owner.to_lowercase());
        save(&known);
    }

    /// Those of the server's machines that are their owner's own client are `clients` now,
    /// every one.
    pub fn set_clients(&self, clients: &BTreeSet<String>) {
        let clients: BTreeSet<String> = clients.iter().map(|it| it.to_lowercase()).collect();
        clients_are(&clients);
        let mut known = self.write();
        known.clients = clients;
        save(&known);
    }

    /// `account` is `owner`'s own client under another name, just made.
    pub fn add_client(&self, account: &str, owner: &str) {
        let account = account.to_lowercase();
        clients_are([&account]);
        let mut known = self.write();
        known
            .owners
            .get_or_insert_with(Owners::new)
            .insert(account.clone(), owner.to_lowercase());
        known.clients.insert(account);
        save(&known);
    }

    /// What the server was told, None if never told of any.
    #[must_use]
    pub fn owners(&self) -> Option<Owners> {
        self.read().owners.clone()
    }

    /// The handle of the person whose machine `account` is, if it's a machine's.
    #[must_use]
    pub fn owner(&self, account: &str) -> Option<String> {
        owner_in(self.read().owners.as_ref(), account)
    }

    /// Those of the server's machines that are their owner's own client.
    #[must_use]
    pub fn clients(&self) -> BTreeSet<String> {
        self.read().clients.clone()
    }

    /// The accounts whose host doesn't say what they are and whose yet, as far as the sidecar
    /// has set them since it started: each account and the host it should have,
    /// `machine/<owner>`, or `client/<owner>` for a person's own client.
    #[must_use]
    pub fn unhosted(&self) -> Vec<(String, String)> {
        let known = self.read();
        let Some(owners) = &known.owners else {
            return Vec::new();
        };
        owners
            .iter()
            .map(|(account, owner)| {
                let host = if known.clients.contains(account) {
                    zirc_proto::names::client_host(owner)
                } else {
                    zirc_proto::names::machine_host(owner)
                };
                (account.clone(), host)
            })
            .filter(|(account, host)| known.hosted.get(account) != Some(host))
            .collect()
    }

    /// `account`'s host is `host` now.
    pub fn hosted(&self, account: &str, host: &str) {
        self.write()
            .hosted
            .insert(account.to_lowercase(), host.to_owned());
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Known> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Known> {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Whose machine `account` is, by the machines `known` says (None if never told of any).
#[must_use]
pub fn owner_in(known: Option<&Owners>, account: &str) -> Option<String> {
    known?.get(&account.to_lowercase()).cloned()
}

/// Every machine any sidecar in this process was told of, for telling agents from people
/// ([`is_agent`]) in what's counted. It only grows, so a test's sidecar telling it of its
/// machines changes no other's counts, and a machine's name stays a machine's.
static AGENTS: LazyLock<RwLock<BTreeSet<String>>> = LazyLock::new(RwLock::default);

fn agents_are<'a>(accounts: impl IntoIterator<Item = &'a String>) {
    let mut agents = AGENTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    agents.extend(accounts.into_iter().cloned());
}

/// Every account any sidecar in this process was told is a person's own client: a person,
/// to [`is_agent`], though it's kept among the machines. It only grows, as [`AGENTS`] does.
static CLIENTS: LazyLock<RwLock<BTreeSet<String>>> = LazyLock::new(RwLock::default);

fn clients_are<'a>(accounts: impl IntoIterator<Item = &'a String>) {
    let mut clients = CLIENTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    clients.extend(accounts.into_iter().cloned());
}

/// Whether `account` is an agent's machine rather than a person, for counting them apart: a
/// machine a sidecar here was told of, and not a person's own client.
#[must_use]
pub fn is_agent(account: &str) -> bool {
    let account = account.to_lowercase();
    let among = |set: &RwLock<BTreeSet<String>>| {
        set.read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&account)
    };
    among(&AGENTS) && !among(&CLIENTS)
}

/// The machines the sidecar's tests name, counted as agents as a server told of them would.
#[cfg(test)]
pub(crate) fn the_tests_machines() {
    let accounts = [
        "ada-mbp",
        "ada-ci",
        "acme-ci",
        "alice-mbp",
        "bob-mbp",
        "mattias-mbp",
    ]
    .map(String::from);
    agents_are(&accounts);
}

/// Where the clients among the machines kept at `machines` are kept: a file of their own, so
/// a sidecar from before them reads the machines as it always did.
fn clients_beside(machines: &Path) -> PathBuf {
    machines.with_file_name("zirc-clients.json")
}

fn save(known: &Known) {
    let (Some(path), Some(owners)) = (&known.path, &known.owners) else {
        return;
    };
    let keep = |path: &Path, bytes: serde_json::Result<Vec<u8>>| {
        let saved = bytes.map_err(std::io::Error::other).and_then(|bytes| {
            let partial = path.with_extension("json.partial");
            std::fs::write(&partial, bytes)?;
            std::fs::rename(&partial, path)
        });
        if let Err(error) = saved {
            warn!(%error, path = %path.display(), "couldn't keep whose machines are");
        }
    };
    keep(path, serde_json::to_vec(owners));
    keep(&clients_beside(path), serde_json::to_vec(&known.clients));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(pairs: &[(&str, &str)]) -> Owners {
        pairs
            .iter()
            .map(|(a, o)| ((*a).to_owned(), (*o).to_owned()))
            .collect()
    }

    #[test]
    fn never_told_no_account_is_a_machine_whatever_its_name() {
        assert_eq!(owner_in(None, "ada-mbp"), None);
        assert_eq!(owner_in(None, "ada"), None);
    }

    #[test]
    fn once_told_only_what_it_was_told_is_a_machine() {
        let known = told(&[("m3-mac", "mattias"), ("ada-mbp", "ada"), ("studio", "ada")]);
        assert_eq!(owner_in(Some(&known), "m3-mac").as_deref(), Some("mattias"));
        assert_eq!(owner_in(Some(&known), "ada-mbp").as_deref(), Some("ada"));
        assert_eq!(owner_in(Some(&known), "STUDIO").as_deref(), Some("ada"));
        // Not `m3`'s, whatever its name says, and nobody's when it wasn't told of.
        assert_eq!(owner_in(Some(&known), "m3-other"), None);
        assert_eq!(owner_in(Some(&known), "ada"), None);
    }

    #[test]
    fn what_its_told_is_kept_and_its_hosts_set_once() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("ergo_history.db");
        let machines = Machines::beside(&history);
        assert_eq!(machines.owners(), None);
        machines.set(&told(&[("ada-mbp", "Ada")]));
        machines.add("Studio", "ada");
        assert_eq!(machines.owner("studio").as_deref(), Some("ada"));
        assert!(is_agent("studio") && is_agent("ada-mbp") && !is_agent("bob"));
        // A restart has them, and sets each one's host again.
        let again = Machines::beside(&history);
        assert_eq!(again.owners(), machines.owners());
        assert_eq!(again.unhosted().len(), 2);
        again.hosted("studio", "machine/ada");
        assert_eq!(
            again.unhosted(),
            [("ada-mbp".to_owned(), "machine/ada".to_owned())]
        );
    }

    #[test]
    fn a_persons_own_client_is_kept_as_theirs_and_hosted_as_a_client() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("ergo_history.db");
        let machines = Machines::beside(&history);
        machines.set(&told(&[("lin-mbp", "lin"), ("lin-irc", "lin")]));
        machines.hosted("lin-mbp", "machine/lin");
        machines.hosted("lin-irc", "machine/lin");
        // Told it's her client: its host is due again, as a client's, and it's no agent.
        machines.set_clients(&BTreeSet::from(["Lin-irc".to_owned()]));
        assert_eq!(
            machines.unhosted(),
            [("lin-irc".to_owned(), "client/lin".to_owned())]
        );
        assert_eq!(machines.owner("lin-irc").as_deref(), Some("lin"));
        assert!(is_agent("lin-mbp") && !is_agent("lin-irc"));
        machines.add_client("lin-irc-2", "Lin");
        assert_eq!(machines.owner("lin-irc-2").as_deref(), Some("lin"));
        // A restart has them, each to be hosted as what it is.
        let again = Machines::beside(&history);
        assert_eq!(again.clients(), machines.clients());
        let hosts: Vec<String> = again.unhosted().into_iter().map(|(_, host)| host).collect();
        assert_eq!(hosts, ["client/lin", "client/lin", "machine/lin"]);
    }
}
